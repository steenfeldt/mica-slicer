// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::set_extruder, OozePrevention::pre_toolchange, GCodeGenerator::placeholder_parser_process
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/LabelObjects.cpp :: LabelObjects::init, LabelObjects::start_object, LabelObjects::stop_object
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::set_extruder, OozePrevention::pre_toolchange
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::parse_layer_gcode
// Changes: Print-level adapter around the existing path emitter; independent tools only. Shared template state, per-tool settings, instance labels and wipe-tower integration.
// Changes: One Label objects setting (gcode_label_objects): firmware-specific labels are EXCLUDE_OBJECT on Klipper and M486 elsewhere, as PrusaSlicer's LabelObjects does.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/LabelObjects.cpp :: LabelObjects::maybe_change_instance
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeWriter.cpp :: GCodeWriter::set_extruders
// Changes: Label changes as text for the travel insert; PrusaSlicer OctoPrint names and 60-byte "..." cut;
// Changes: multiple_extruders as GCodeWriter defines it (a tool other than the first prints).
// Source: prusaslicer@b028299c77 src/libslic3r/Extruder.cpp :: Extruder::retract
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/CoolingBuffer.cpp :: CoolingBuffer::apply_layer_cooldown
// Changes: Toolchange retraction reads filament-overridden printer values; the new tool's fan is re-emitted after the toolchange (;_TOOLCHANGE_END).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::retract_and_wipe, OozePrevention::post_toolchange, OozePrevention::_get_temp
// Changes: The old tool wipes along its last extrusion during the toolchange retraction (Prusa path); ooze prevention waits (M109) for the new tool after the toolchange.
// Changes: Orca configurations delegate GCode::set_extruder to orca_toolchange (wipe, eager spiral lift, ooze wait, fan resume).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::PlaceholderParserIntegration::update_from_gcodewriter
// Changes: extruded_volume/extruded_weight(_total) also reach single-object Prusa prints (MK4IS/MK3.9/MINI IS before_layer_gcode reads extruded_weight_total).
// Changes: is_extruder_used is padded to max(255, tools) (Orca: max(64, tools)) so out-of-range template reads see false.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

use crate::export::ExportError;
use crate::writer::{Writer, num};
use mica_core::config::Config;
use mica_core::mesh::TriangleMesh;
use mica_core::print_job::{PreparedPrint, tool_config};
use mica_core::settings::PrintSettings;
use mica_core::toolpath::Role;
use mica_template::{Context, Var};
use std::collections::BTreeMap;

/// `is_extruder_used`, padded like upstream: the placeholder parser reads an out-of-range
/// vector index as element 0, so a two-tool template's `is_extruder_used[2]` would otherwise
/// report tool 0's state. PrusaSlicer pads to at least 255 entries, Orca to
/// MAXIMUM_EXTRUDER_NUMBER (64).
pub(crate) fn is_extruder_used(c: &Config, tools: usize, used: impl Fn(usize) -> bool) -> Var {
    let min = if mica_core::rules::Rules::of(c).is_orca() {
        64
    } else {
        255
    };
    Var::bools((0..tools.max(min)).map(|i| i < tools && used(i)))
}

pub struct ToolSettings {
    pub config: Config,
    pub settings: PrintSettings,
}
pub struct Multi<'a> {
    pub job: &'a PreparedPrint,
    pub tools: Vec<ToolSettings>,
    pub active: usize,
    labels: Vec<Label>,
    object_labels: Vec<usize>,
    current_label: Option<usize>,
    pub(crate) changes: usize,
    filament: BTreeMap<(usize, &'static str), f64>,
    pub(crate) tower_state: crate::wipe_tower::State,
    /// Each tool's part-fan speed for the current layer, restored right after a toolchange.
    pub(crate) layer_fans: Vec<f64>,
}
struct Label {
    name: String,
    display: String,
    /// The OctoPrint comment name: PrusaSlicer's "<name> id:<object> copy <instance>" (0-based),
    /// Orca's `name`.
    comment: String,
    hull: Vec<mica_core::geometry::Point>,
}
impl<'a> Multi<'a> {
    pub fn new(job: &'a PreparedPrint, c: &Config) -> Result<Self, ExportError> {
        if c.bool("single_extruder_multi_material").unwrap_or(false) && job.ordering.used.len() > 1
        {
            return Err(ExportError::Unsupported(
                "multi-tool export requires independent extruders; MMU is not implemented".into(),
            ));
        }
        // The exporter refuses absolute extrusion first; a plate of one tool is no multi-tool
        // print, so the reason is the extrusion mode itself.
        if let Some(problem) = mica_core::safety::extrusion_mode_problem(c) {
            return Err(ExportError::Unsupported(problem));
        }
        let tools = (0..c.vector_len("nozzle_diameter").max(1))
            .map(|i| {
                let config = tool_config(c, i);
                let settings = PrintSettings::from_config(&config)
                    .map_err(|e| ExportError::Unsupported(e.to_string()))?;
                Ok(ToolSettings { config, settings })
            })
            .collect::<Result<Vec<_>, ExportError>>()?;
        let mut keys = Vec::new();
        let mut labels = Vec::new();
        let mut object_labels = Vec::new();
        for o in &job.objects {
            let key = (o.object_id, o.instance_id);
            let index = if let Some(i) = keys.iter().position(|k| *k == key) {
                i
            } else {
                let i = keys.len();
                keys.push(key);
                let mut mesh = TriangleMesh::default();
                for part in &job.objects {
                    if (part.object_id, part.instance_id) == key {
                        mesh.vertices.extend_from_slice(&part.mesh.vertices);
                    }
                }
                let display = display_name(job, o);
                let base = crate::orca::object_name(&display);
                let base = base.strip_suffix("_id_0_copy_0").unwrap_or(&base);
                let name = format!("{base}_id_{}_copy_{}", o.object_id, o.instance_id);
                let comment = if mica_core::rules::Rules::of(c).is_orca() {
                    name.clone()
                } else {
                    format!(
                        "{} id:{} copy {}",
                        label_name(&o.name),
                        o.object_id,
                        o.instance_id
                    )
                };
                labels.push(Label {
                    name,
                    comment,
                    display,
                    hull: crate::orca::object_hull(&mesh),
                });
                i
            };
            object_labels.push(index);
        }
        Ok(Self {
            job,
            tools,
            active: job
                .wipe_tower
                .as_ref()
                .and_then(|p| p.priming_tools.first().copied())
                .unwrap_or(job.ordering.first),
            labels,
            object_labels,
            current_label: None,
            changes: 0,
            filament: BTreeMap::new(),
            tower_state: crate::wipe_tower::State::default(),
            layer_fans: Vec::new(),
        })
    }
    pub fn set_vars(&self, ctx: &mut Context, c: &Config) {
        for key in [
            "initial_tool",
            "initial_extruder",
            "current_extruder",
            "current_hotend",
            "initial_no_support_tool",
            "initial_no_support_extruder",
            "initial_no_support_hotend",
            "most_used_physical_extruder_id",
        ] {
            ctx.set(key, Var::int(self.active as i64));
        }
        ctx.set_current_extruder(self.active);
        ctx.set(
            "is_extruder_used",
            is_extruder_used(c, self.tools.len(), |i| self.job.ordering.used.contains(&i)),
        );
        ctx.set(
            "total_toolchanges",
            Var::int(self.job.ordering.changes as i64),
        );
        for key in [
            "first_tools",
            "first_filaments",
            "first_non_support_tools",
            "first_non_support_filaments",
        ] {
            ctx.set(
                key,
                Var::ints((0..self.tools.len()).map(|i| {
                    if self.job.ordering.used.contains(&i) {
                        i as i64
                    } else {
                        -1
                    }
                })),
            );
        }
        ctx.set(
            "bed_temperature_initial_layer_single",
            Var::int(
                c.int_at("first_layer_bed_temperature", self.active)
                    .unwrap_or(0),
            ),
        );
        // Templates see actual planned geometry, including the single-tool no-tower case.
        for key in ["has_wipe_tower", "wipe_tower", "enable_prime_tower"] {
            ctx.set(key, Var::bool(self.job.wipe_tower.is_some()));
        }
        ctx.set(
            "has_single_extruder_multi_material_priming",
            Var::bool(
                self.job
                    .wipe_tower
                    .as_ref()
                    .is_some_and(|p| !p.priming_tools.is_empty()),
            ),
        );
    }
    /// One Label objects setting: the firmware decides the commands (EXCLUDE_OBJECT on
    /// Klipper, M486 elsewhere); OctoPrint comments are the same everywhere.
    fn style(c: &Config) -> mica_core::merged::LabelObjects {
        mica_core::merged::label_objects(c)
    }
    fn exclude_object(c: &Config) -> bool {
        c.string("gcode_flavor") == Some("klipper")
            && Self::style(c) == mica_core::merged::LabelObjects::Firmware
    }
    fn enabled(c: &Config) -> bool {
        Self::style(c) != mica_core::merged::LabelObjects::Disabled
    }
    pub fn definitions(&self, w: &mut Writer, c: &Config) {
        if !Self::enabled(c) {
            return;
        }
        for (i, l) in self.labels.iter().enumerate() {
            if Self::exclude_object(c) {
                if l.hull.len() >= 3 {
                    w.raw(&crate::orca::object_definition(&l.name, &l.hull));
                }
            } else if c.string("gcode_label_objects") == Some("firmware") {
                w.raw(&format!("M486 S{i}\nM486 A{}\nM486 S-1", l.display));
            }
        }
    }
    pub fn end_label(&mut self, w: &mut Writer, c: &Config) {
        let text = self.label_change(None, c);
        if !text.is_empty() {
            w.raw(&text);
        }
    }
    /// `LabelObjects::has_active_instance`.
    pub(crate) fn has_label(&self) -> bool {
        self.current_label.is_some()
    }
    pub fn begin_label(&mut self, object: usize, w: &mut Writer, c: &Config) {
        let text = self.label_change(Some(object), c);
        if !text.is_empty() {
            w.raw(&text);
        }
    }
    /// `GCodeWriter::multiple_extruders`: a tool other than the first one prints. Label changes
    /// then follow the extrusions instead of the travels.
    pub(crate) fn multiple_extruders(&self) -> bool {
        self.job.ordering.used.iter().any(|&t| t > 0)
    }
    /// `LabelObjects::maybe_change_instance` as text: stop the open label and start `object`'s
    /// (`None`: only stop, as before a skirt or brim); empty when that label is already open.
    pub(crate) fn label_change(&mut self, object: Option<usize>, c: &Config) -> String {
        let target = object.map(|o| self.object_labels[o]);
        if !Self::enabled(c) || target == self.current_label {
            return String::new();
        }
        let mut text = String::new();
        if let Some(i) = self.current_label.take() {
            let l = &self.labels[i];
            text.push_str(&if Self::exclude_object(c) {
                format!("EXCLUDE_OBJECT_END NAME={}\n", l.name)
            } else if c.string("gcode_label_objects") == Some("firmware") {
                "M486 S-1\n".to_string()
            } else {
                format!("; stop printing object {}\n", l.comment)
            });
        }
        if let Some(i) = target {
            let l = &self.labels[i];
            text.push_str(&if Self::exclude_object(c) {
                format!("EXCLUDE_OBJECT_START NAME={}\n", l.name)
            } else if c.string("gcode_label_objects") == Some("firmware") {
                format!("M486 S{i}\n")
            } else {
                format!("; printing object {}\n", l.comment)
            });
            self.current_label = target;
        }
        text
    }
    /// Prusa invokes the toolchange template once with no previous tool after machine startup.
    pub fn initialize(
        &self,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
    ) -> Result<(), ExportError> {
        if mica_core::rules::Rules::of(c).is_orca() {
            return Ok(());
        }
        ctx.set("previous_extruder", Var::int(-1));
        ctx.set("next_extruder", Var::int(self.active as i64));
        ctx.set("toolchange_count", Var::int(0));
        crate::templates::set_layer_vars(ctx, -1, 0., 0.);
        let text = run(
            "toolchange_gcode",
            c.string("toolchange_gcode").unwrap_or(""),
            ctx,
            w,
            c,
        )
        .map_err(ExportError::Template)?;
        let selects = text.lines().any(|l| {
            l.split(';').next().unwrap_or("").split_whitespace().next()
                == Some(&format!("T{}", self.active))
        });
        if !text.is_empty() {
            w.raw(&text);
        }
        if !selects {
            w.raw(&format!("T{}", self.active));
        }
        Ok(())
    }
    pub fn retract_current(&self, w: &mut Writer) {
        let t = &self.tools[self.active];
        let extra = t.config.float("retract_restart_extra").unwrap_or(0.);
        w.retract_with_extra(t.settings.retract_length, t.settings.retract_speed, extra);
    }

    /// `GCodeGenerator::set_extruder`. `wipe` is the Prusa wipe path (`m_wipe`): the old tool
    /// wipes along it while retracting for the toolchange, and it is always reset. Orca
    /// configurations use `orca_wipe` in `change_orca` instead.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn change(
        &mut self,
        next: usize,
        layer: usize,
        z: f64,
        wipe: &mut crate::prusa_wipe::Wipe,
        ctx: &mut Context,
        w: &mut Writer,
        c: &Config,
        orca_wipe: &mut crate::orca_motion::Wipe,
    ) -> Result<(), ExportError> {
        if next == self.active {
            return Ok(());
        }
        if crate::orca_motion::enabled(c) {
            return self.change_orca(next, layer, z, ctx, w, c, orca_wipe);
        }
        self.end_label(w, c);
        let prev = self.active;
        let s = &self.tools[prev].settings;
        // GCodeWriter::retract_for_toolchange: `c` carries the filament overrides
        // (filament_retract_length_toolchange, filament_retract_restart_extra_toolchange).
        let length = c
            .float_at("retract_length_toolchange", prev)
            .unwrap_or(s.retract_length);
        // Extruder::retract: the restart extra is replaced only by an actual retraction.
        let retracts = length > w.template_state().1;
        // retract_and_wipe(true): retract_for_toolchange(true) (retract_before_wipe of the
        // toolchange length), wipe along the last extrusion, then top up.
        if let Some(plan) = crate::prusa_wipe::Params::new(&self.tools[prev].config, s)
            .filter(|_| wipe.has_path())
            .zip(w.position())
            .map(|(p, from)| {
                wipe.plan(
                    from,
                    w.retracted(),
                    length,
                    &p.with_toolchange_length(length),
                )
            })
        {
            crate::prusa_wipe::emit(w, &plan, s.retract_speed);
        }
        // "Always reset the extrusion path, even if the tool change retract is set to zero."
        wipe.reset();
        if retracts {
            // A wipe that consumed the whole length leaves only float residue (quantize_e).
            if ((length - w.retracted()) * 1e5).round() > 0. {
                w.retract_to(length, s.retract_speed);
            }
            w.set_restart_extra(
                c.float_at("retract_restart_extra_toolchange", prev)
                    .unwrap_or(0.),
            );
        }
        ctx.set("filament_extruder_id", Var::int(prev as i64));
        let end = run(
            "end_filament_gcode",
            c.string_at("end_filament_gcode", prev).unwrap_or(""),
            ctx,
            w,
            c,
        )
        .map_err(ExportError::Template)?;
        if !end.is_empty() {
            w.raw(&end);
        }
        if c.bool("ooze_prevention").unwrap_or(false) {
            let temp = c
                .float_at(
                    if layer == 0 {
                        "first_layer_temperature"
                    } else {
                        "temperature"
                    },
                    prev,
                )
                .unwrap_or(0.);
            let idle = c
                .float_at("idle_temperature", prev)
                .filter(|v| *v > 0.)
                .unwrap_or(temp + c.float("standby_temperature_delta").unwrap_or(0.));
            w.raw(&format!(
                "M104 S{} T{prev} ; cooldown",
                num(idle.max(0.), 0)
            ));
        }
        self.changes += 1;
        for (key, value) in [
            ("previous_extruder", prev),
            ("next_extruder", next),
            ("toolchange_count", self.changes),
            ("current_extruder", prev),
        ] {
            ctx.set(key, Var::int(value as i64));
        }
        crate::templates::set_layer_vars(ctx, layer as i64, z, z);
        let tpl = c
            .string("toolchange_gcode")
            .filter(|s| !s.is_empty())
            .or_else(|| c.string("change_filament_gcode"))
            .unwrap_or("");
        let text = run("toolchange_gcode", tpl, ctx, w, c).map_err(ExportError::Template)?;
        let selects = text.lines().any(|l| {
            l.split(';').next().unwrap_or("").split_whitespace().next() == Some(&format!("T{next}"))
        });
        if !text.is_empty() {
            w.raw(&text);
        }
        if !selects {
            w.raw(&format!("T{next}"));
        }
        self.active = next;
        w.select_tool(next, c);
        // CoolingBuffer re-emits the new tool's fan at ;_TOOLCHANGE_END, because the custom
        // toolchange G-code (and the XL's separate tool fans) leave it unknown.
        if !mica_core::rules::Rules::of(c).is_orca()
            && let Some(&fan) = self.layer_fans.get(next)
        {
            w.forget_fan();
            w.set_fan(fan);
        }
        ctx.set_current_extruder(next);
        ctx.set("current_extruder", Var::int(next as i64));
        ctx.set("current_hotend", Var::int(next as i64));
        if !crate::templates::sets_temperature(&text, 104, 109).0 {
            let temp = c
                .float_at(
                    if layer == 0 {
                        "first_layer_temperature"
                    } else {
                        "temperature"
                    },
                    next,
                )
                .unwrap_or(0.);
            if temp > 0. {
                w.raw(&format!(
                    "M109 S{} T{next} ; set temperature and wait",
                    num(temp, 0)
                ));
            }
        }
        ctx.set("filament_extruder_id", Var::int(next as i64));
        let start = run(
            "start_filament_gcode",
            c.string_at("start_filament_gcode", next).unwrap_or(""),
            ctx,
            w,
            c,
        )
        .map_err(ExportError::Template)?;
        if !start.is_empty() {
            w.raw(&start);
        }
        // OozePrevention::post_toolchange: wait for the new tool's operating temperature.
        if !mica_core::rules::Rules::of(c).is_orca()
            && c.bool("ooze_prevention").unwrap_or(false)
            && c.float("standby_temperature_delta").unwrap_or(0.) != 0.
        {
            let temperature = c.float_at("temperature", next).unwrap_or(0.);
            let temp = if layer == 0 || temperature == 0. {
                c.float_at("first_layer_temperature", next).unwrap_or(0.)
            } else {
                temperature
            };
            w.raw(&format!(
                "M109 S{} T{next} ; set temperature and wait for it to be reached",
                num(temp, 0)
            ));
        }
        if c.string("gcode_flavor") == Some("klipper")
            && c.bool_at("enable_pressure_advance", next).unwrap_or(false)
        {
            w.set_pressure_advance(c.float_at("pressure_advance", next).unwrap_or(0.));
        }
        Ok(())
    }
    /// Fixed toolchange time for layers without a tower (tower visits account for
    /// their own retraction and change time). Prusa excludes XL load/unload times.
    pub(crate) fn cooling_toolchange_time(&self, layer: usize, c: &Config) -> f64 {
        if self.job.wipe_tower.is_some() {
            return 0.;
        }
        let mut previous = self.active;
        let mut seconds = 0.;
        let xl = c
            .string("printer_notes")
            .is_some_and(|s| s.contains("PRINTER_MODEL_XL"));
        for &tool in &self.job.ordering.layers[layer].extruders {
            if tool == previous {
                continue;
            }
            if !xl && !mica_core::rules::Rules::of(c).is_orca() {
                seconds += c.float_at("filament_unload_time", previous).unwrap_or(0.)
                    + c.float_at("filament_load_time", tool).unwrap_or(0.);
            }
            seconds += c
                .float_at("retract_length_toolchange", previous)
                .unwrap_or(0.)
                / self.tools[previous].settings.retract_speed;
            seconds += c.float_at("retract_length_toolchange", tool).unwrap_or(0.)
                / self.tools[tool].settings.deretract_speed;
            previous = tool;
        }
        seconds
    }
    pub fn record(&mut self, tool: usize, role: Role, mm: f64) {
        *self.filament.entry((tool, role.name())).or_default() += mm;
    }
    pub(crate) fn record_tower(&mut self, role: &'static str, mm: f64) {
        *self.filament.entry((self.active, role)).or_default() += mm;
    }
    pub fn statistics(&self, w: &mut Writer) {
        for (&(tool, role), &mm) in &self.filament {
            w.raw(&format!("; filament used T{tool} [{role}] [mm] = {mm:.5}"));
        }
        for &tool in &self.job.ordering.used {
            // The second pass fills these from explicit G-code, including custom priming.
            w.raw(&format!("; filament used T{tool} [mm] = 0\n; filament used T{tool} [cm3] = 0\n; filament used T{tool} [g] = 0"));
        }
    }
}

/// `extruded_volume[_total]` and `extruded_weight[_total]`, which PrusaSlicer provides to
/// every custom G-code block (PlaceholderParserIntegration::update_from_gcodewriter).
pub(crate) fn set_extruded(ctx: &mut Context, w: &Writer, c: &Config, states: &[(f64, f64)]) {
    let volumes: Vec<_> = states
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (w.filament_for_tool(i) + s.0)
                * std::f64::consts::PI
                * (c.float_at("filament_diameter", i).unwrap_or(1.75) / 2.).powi(2)
        })
        .collect();
    let weights: Vec<_> = volumes
        .iter()
        .enumerate()
        .map(|(i, v)| v * c.float_at("filament_density", i).unwrap_or(0.) / 1000.)
        .collect();
    ctx.set("extruded_volume_total", Var::float(volumes.iter().sum()));
    ctx.set("extruded_weight_total", Var::float(weights.iter().sum()));
    ctx.set("extruded_volume", Var::floats(volumes));
    ctx.set("extruded_weight", Var::floats(weights));
}

/// Read and write every physical extruder's state, including changes made by XL startup priming.
pub(crate) fn run(
    name: &str,
    tpl: &str,
    ctx: &mut Context,
    w: &mut Writer,
    c: &Config,
) -> Result<String, String> {
    let n = c.vector_len("nozzle_diameter").max(1);
    let (position, _, _, zhop) = w.template_state();
    let states = w.tool_states(n);
    ctx.set_output("position", Var::floats(position));
    ctx.set_output("e_retracted", Var::floats(states.iter().map(|s| s.0)));
    ctx.set_output("e_restart_extra", Var::floats(states.iter().map(|s| s.1)));
    ctx.set("zhop", Var::float(zhop));
    set_extruded(ctx, w, c, &states);
    let result = crate::templates::run(name, tpl, ctx)?;
    let output = |key: &str, len: usize| -> Result<Vec<f64>, String> {
        ctx.output(key)
            .and_then(Var::as_vector)
            .filter(|v| v.len() == len)
            .ok_or_else(|| format!("{name}: invalid {key} length"))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("{name}: {key} must be finite"))
            })
            .collect()
    };
    let pos = output("position", 3)?;
    let e = output("e_retracted", n)?;
    let extra = output("e_restart_extra", n)?;
    if e.iter().any(|v| *v < 0.) {
        return Err(format!("{name}: negative e_retracted"));
    }
    w.apply_tool_states(e.into_iter().zip(extra).collect());
    let states = w.tool_states(n);
    let active = w.active_tool();
    let pos = [pos[0], pos[1], pos[2]];
    w.apply_template_state(
        (pos != position).then_some(pos),
        states[active].0,
        states[active].1,
    );
    Ok(result)
}

/// Firmware metadata lists each cancellable instance once, including all of its parts.
pub fn objects_info(job: &PreparedPrint) -> String {
    let mut keys = Vec::new();
    let mut entries = Vec::new();
    for o in &job.objects {
        let key = (o.object_id, o.instance_id);
        if keys.contains(&key) {
            continue;
        }
        keys.push(key);
        let mut mesh = TriangleMesh::default();
        for part in &job.objects {
            if (part.object_id, part.instance_id) == key {
                mesh.vertices.extend_from_slice(&part.mesh.vertices);
            }
        }
        let name = display_name(job, o);
        let info = crate::prusa_metadata::objects_info(&name, &mesh);
        entries.push(
            info.trim_start_matches("{\"objects\":[")
                .trim_end_matches("]}")
                .to_string(),
        );
    }
    format!("{{\"objects\":[{}]}}", entries.join(","))
}

fn display_name(job: &PreparedPrint, o: &mica_core::print_job::PrintObject) -> String {
    let mut name = label_name(&o.name);
    if job
        .objects
        .iter()
        .any(|other| other.object_id == o.object_id && other.instance_id != o.instance_id)
    {
        name.push_str(&format!(" (Instance {})", o.instance_id + 1));
    }
    name
}

/// `LabelObjects::init`: an object name of more than 60 bytes is cut there (on a UTF-8 boundary)
/// and gets "..."; control characters and `;` (which would end the command) are dropped.
pub(crate) fn label_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != ';')
        .collect();
    if clean.len() <= 60 {
        return clean;
    }
    let mut cut = 60;
    while !clean.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &clean[..cut])
}
