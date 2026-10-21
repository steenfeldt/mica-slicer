// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/calib.hpp :: Calib_Params, CalibMode
// Source: orca@8500fcdcca src/libslic3r/calib.cpp :: CalibPressureAdvanceLine::print_pa_lines, CalibPressureAdvancePattern::generate_custom_gcodes, CalibPressureAdvance::find_optimal_PA_speed
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer, GCode::interpolate_value_across_layers, GCode::_do_export
// Source: orca@8500fcdcca src/libslic3r/calib.cpp :: CalibPressureAdvancePattern::_refresh_writer
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export
// Source: orca@8500fcdcca src/slic3r/GUI/Plater.cpp :: adjust_settings_for_flowrate_calib, Plater::_calib_pa_tower, Plater::calib_temp, Plater::calib_retraction, Plater::calib_max_vol_speed
// Source: orca@8500fcdcca src/slic3r/Utils/CalibUtils.cpp :: get_calib_mode_name
// Changes: Headless typed API, bounded ranges, procedural meshes and direct PA paths; classic percentage flow passes; no device/GUI state, YOLO, auto calibration or bundled models. See docs/calibration.md.
// Changes: Multi-tool printers print the test as a plate assigned to the chosen tool (single-tool print plan, tool's own vector slots), so templates pick and heat that tool alone.

//! Headless calibration plans. Generate once for a GUI preview, then prepare and export through
//! the ordinary `LayerSource` pipeline. All coordinates are bed coordinates, tools are zero based.

mod geometry;

use crate::{
    config::Config,
    flow::Flow,
    geometry::{BoundingBox, Point},
    mesh::TriangleMesh,
    print::{LayerSource, PreparedObject, prepare_object},
    print_job::{
        PathAssignment, PreparedPrint, PrintObject, ScheduledLayer, ToolAssignments, tool_config,
    },
    settings::PrintSettings,
    toolpath::{ExtrusionPath, LayerToolpaths, Role},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    FlowRate,
    PressureAdvanceLine,
    PressureAdvancePattern,
    PressureAdvanceTower,
    Temperature,
    Retraction,
    MaxVolumetricSpeed,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "flow" | "flow-rate" => Ok(Self::FlowRate),
            "pa-line" => Ok(Self::PressureAdvanceLine),
            "pa-pattern" => Ok(Self::PressureAdvancePattern),
            "pa-tower" => Ok(Self::PressureAdvanceTower),
            "temperature" | "temperature-tower" => Ok(Self::Temperature),
            "retraction" => Ok(Self::Retraction),
            "max-volumetric-speed" => Ok(Self::MaxVolumetricSpeed),
            _ => Err(format!("unknown calibration kind {s}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parameters {
    pub kind: Kind,
    /// Positive step; temperature descends, other ranges ascend. Flow uses `pass` instead.
    pub start: f64,
    pub end: f64,
    /// PA tower: PA/mm; retraction: mm/mm; volumetric: (mm³/s)/mm.
    pub step: f64,
    pub pass: u8,
    /// Fast PA speed, mm/s; None uses Orca's volumetrically limited suggested speed.
    pub speed: Option<f64>,
    /// Zero-based tool that prints the test. On a multi-tool printer the plan is that of a
    /// plate whose objects are all assigned to this tool (Orca/PrusaSlicer): it is the
    /// initial and only used tool, so the start templates pick and heat it alone.
    pub tool: usize,
}
impl Parameters {
    pub fn new(kind: Kind) -> Self {
        let (start, end, step) = match kind {
            Kind::Temperature => (230., 200., 5.),
            Kind::Retraction => (0., 2., 0.1),
            Kind::MaxVolumetricSpeed => (5., 30., 0.5),
            Kind::PressureAdvanceTower => (0., 0.1, 0.002),
            _ => (0., 0.1, 0.005),
        };
        Self {
            kind,
            start,
            end,
            step,
            pass: 1,
            speed: None,
            tool: 0,
        }
    }
    fn validate(&self) -> Result<usize, String> {
        if ![self.start, self.end, self.step]
            .iter()
            .all(|v| v.is_finite())
            || self.start < 0.
            || self.end < 0.
            || self.step <= 0.
        {
            return Err(
                "start/end must be finite and nonnegative; step must be finite and positive".into(),
            );
        }
        if self
            .speed
            .is_some_and(|v| !v.is_finite() || v <= 0. || v > 1000.)
        {
            return Err("speed must be in (0, 1000] mm/s".into());
        }
        if !matches!(self.pass, 1 | 2) {
            return Err("flow pass must be 1 or 2".into());
        }
        if self.kind == Kind::FlowRate {
            return Ok(if self.pass == 1 { 9 } else { 10 });
        }
        let span = if self.kind == Kind::Temperature {
            self.start - self.end
        } else {
            self.end - self.start
        };
        if span < 0. {
            return Err("temperature must descend; other ranges must ascend".into());
        }
        let intervals = span / self.step;
        if !intervals.is_finite() || intervals > 99. {
            return Err("at most 100 calibration samples are allowed".into());
        }
        if self.kind == Kind::Temperature
            && (self.step != 5.
                || self.end < 150.
                || self.start > 350.
                || self.start % 5. != 0.
                || self.end % 5. != 0.)
        {
            return Err("temperature requires 150–350°C, multiples of 5, and --step 5".into());
        }
        if self.kind == Kind::MaxVolumetricSpeed && (self.start <= 0. || self.end > 200.) {
            return Err("volumetric range must be in (0, 200] mm³/s".into());
        }
        if self.kind == Kind::Retraction && self.end > 10. {
            return Err("retraction cannot exceed 10 mm".into());
        }
        if matches!(
            self.kind,
            Kind::PressureAdvanceLine | Kind::PressureAdvancePattern | Kind::PressureAdvanceTower
        ) && self.end > 1.
        {
            return Err("pressure advance cannot exceed 1 second".into());
        }
        // Orca rounds sample count up. Preserve that convention, including a possible overshoot.
        Ok((intervals - 1e-9).ceil() as usize + 1)
    }
}

/// Firmware-independent changes at a path boundary. Absent for ordinary sliced objects.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Control {
    pub pressure_advance: Option<f64>,
    pub temperature: Option<f64>,
    pub retract_length: Option<f64>,
    /// Multiplier relative to the selected filament's existing extrusion multiplier.
    pub flow_ratio: Option<f64>,
    /// Final requested speed, before machine acceleration; calibration disables cooling slowdown.
    pub speed_mm_s: Option<f64>,
    /// Stable value for the G-code comment and reading an unlabelled result.
    pub sample: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Object {
    pub name: String,
    pub mesh: TriangleMesh,
    pub flow_ratio: f64,
}

#[derive(Clone, Debug)]
pub struct Generated {
    pub parameters: Parameters,
    pub objects: Vec<Object>,
    /// Canonical Mica config keys, applied on top of resolved profiles.
    pub overrides: Vec<(String, String)>,
    pub samples: Vec<f64>,
    center: (f64, f64),
    nozzle: f64,
}

/// Generate bed-centred meshes plus overrides; no file I/O or profile discovery.
pub fn generate(p: &Parameters, config: &Config) -> Result<Generated, String> {
    let count = p.validate()?;
    let tools = config.vector_len("nozzle_diameter").max(1);
    if p.tool >= tools {
        return Err(format!(
            "tool {} does not exist; the printer has {tools}",
            p.tool + 1
        ));
    }
    // The tested tool's nozzle, filament and limits (its vector entries).
    let tool_config = tool_config(config, p.tool);
    let config = &tool_config;
    let s = PrintSettings::from_config(config).map_err(|e| e.to_string())?;
    crate::safety::settings(&s)?;
    if p.kind == Kind::Temperature {
        let minimum = config
            .float("nozzle_temperature_range_low")
            .unwrap_or(s.temperature.min(s.first_layer_temperature) - 30.);
        let maximum = config
            .float("nozzle_temperature_range_high")
            .unwrap_or(s.temperature.max(s.first_layer_temperature) + 30.);
        if p.end < minimum || p.start > maximum {
            return Err(format!(
                "temperature range must stay within selected filament limits ({minimum:.0}–{maximum:.0}°C)"
            ));
        }
    }
    let bed = config
        .points("bed_shape")
        .ok_or("printer has no bed_shape")?;
    if bed.len() != 4 {
        return Err("calibration currently requires a rectangular bed".into());
    }
    let minx = bed.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let miny = bed.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let maxx = bed.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let maxy = bed.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if ![minx, miny, maxx, maxy].iter().all(|v| v.is_finite())
        || maxx - minx < 50.
        || maxy - miny < 50.
    {
        return Err("invalid or too small calibration bed".into());
    }
    for corner in [(minx, miny), (minx, maxy), (maxx, miny), (maxx, maxy)] {
        if !bed.contains(&corner) {
            return Err("calibration currently requires an axis-aligned rectangular bed".into());
        }
    }
    let center = ((minx + maxx) / 2., (miny + maxy) / 2.);
    let samples = if p.kind == Kind::FlowRate {
        if p.pass == 1 {
            (-4..=4).map(|i| f64::from(i) * 5.).collect()
        } else {
            (-9..=0).map(f64::from).collect()
        }
    } else {
        (0..count)
            .map(|i| {
                p.start + if p.kind == Kind::Temperature { -1. } else { 1. } * i as f64 * p.step
            })
            .collect()
    };
    let mut g = Generated {
        parameters: p.clone(),
        objects: Vec::new(),
        overrides: Vec::new(),
        samples,
        center,
        nozzle: s.nozzle_diameter,
    };
    for (k, v) in [
        ("skirts", "0"),
        ("brim_width", "0"),
        ("support_material", "0"),
        ("raft_layers", "0"),
        ("cooling", "0"),
        ("slowdown_below_layer_time", "0"),
        ("slow_down_layer_time", "0"),
        ("use_firmware_retraction", "0"),
        ("wipe", "0"),
        ("retract_on_layer_change", "0"),
        ("seam_slope_type", "none"),
        ("scarf_seam_placement", "nowhere"),
        ("xy_hole_compensation", "0"),
        ("xy_contour_compensation", "0"),
        ("hole_to_polyhole", "0"),
        ("fuzzy_skin", "none"),
        ("spiral_vase", "0"),
        ("ironing", "0"),
        ("small_area_infill_flow_compensation", "0"),
        ("small_perimeter_speed", "100%"),
        ("elefant_foot_compensation", "0"),
        ("xy_size_compensation", "0"),
        ("perimeter_generator", "classic"),
        ("gcode_label_objects", "disabled"),
    ] {
        g.set(k, v);
    }
    let h = match p.kind {
        Kind::MaxVolumetricSpeed => s.nozzle_diameter * 0.8,
        _ => s.nozzle_diameter / 2.,
    };
    g.set("layer_height", h);
    g.set("first_layer_height", h);
    g.set("top_solid_min_thickness", 0);
    g.set("bottom_solid_min_thickness", 0);
    g.set("fill_density", "0%");
    g.set("top_solid_layers", 0);
    g.set("bottom_solid_layers", 0);
    g.set("perimeters", 2);
    let height = match p.kind {
        Kind::FlowRate => 10. * h,
        Kind::PressureAdvanceLine => h,
        Kind::PressureAdvancePattern => 4. * h,
        Kind::Temperature => count as f64 * 10. * s.nozzle_diameter / 0.4,
        Kind::PressureAdvanceTower => count as f64,
        Kind::Retraction => 1.4 + (p.end - p.start) / p.step - 1e-5,
        Kind::MaxVolumetricSpeed => (p.end - p.start + 1.) / p.step,
    };
    if height > config.float("max_print_height").unwrap_or(250.) || height / h > 5000. {
        return Err("calibration exceeds printer height or 5000 layers; narrow the range".into());
    }
    match p.kind {
        Kind::FlowRate => {
            g.set("perimeters", 1);
            g.set("fill_density", "35%");
            g.set("bottom_solid_layers", 2);
            g.set("top_solid_layers", 5);
            g.set("thick_internal_bridges", "0");
            g.set("fill_pattern", "rectilinear");
            g.set("top_fill_pattern", "monotonic");
            g.set("fill_angle", 45);
            g.set("solid_infill_extrusion_width", s.nozzle_diameter * 1.2);
            g.set("top_infill_extrusion_width", s.nozzle_diameter * 1.2);
            for i in 0..count {
                let x = center.0 + (i % 3) as f64 * 36. - 36.;
                let rows = count.div_ceil(3);
                let y = center.1 + (i / 3) as f64 * 31. - (rows - 1) as f64 * 15.5;
                let mut mesh = TriangleMesh::default();
                geometry::cuboid(
                    &mut mesh,
                    [x - 15., y - 12., 0.],
                    [x + 15., y + 12., height],
                );
                g.objects.push(Object {
                    name: format!("flow {:+}%", g.samples[i]),
                    mesh,
                    flow_ratio: 1. + g.samples[i] / 100.,
                });
            }
        }
        Kind::PressureAdvanceLine | Kind::PressureAdvancePattern => {
            // The meshes represent the actual direct paths for preview; no bundled dummy STL.
            let layers = g.pa_layers(&s)?;
            let mut mesh = TriangleMesh::default();
            for (layer, _) in layers {
                for path in layer.paths {
                    geometry::ribbon(
                        &mut mesh,
                        &path.points,
                        path.flow.width,
                        layer.print_z - layer.height,
                        layer.print_z,
                    );
                }
            }
            g.objects.push(Object {
                name: "pressure advance".into(),
                mesh,
                flow_ratio: 1.,
            });
        }
        Kind::Temperature => {
            g.set("temperature", p.start);
            g.set("first_layer_temperature", p.start);
            // Orca templates also read source option names, retained alongside canonical keys.
            // `config` (above) holds the tested tool's entries; `Generated::config` puts the
            // value into that tool's slot only.
            for key in ["nozzle_temperature_initial_layer", "nozzle_temperature"] {
                if config.get(key).is_some() {
                    g.set(key, format!("{:.0}", p.start));
                }
            }
            g.set("bottom_solid_layers", 3);
            g.set("top_solid_layers", 3);
            let mut mesh = TriangleMesh::default();
            let scale = s.nozzle_diameter / 0.4;
            // Two posts with a 20 mm bridge and embossed numeric labels on the front face.
            for i in 0..count {
                let z = i as f64 * 10. * scale;
                for x in [-18., 10.] {
                    geometry::cuboid(
                        &mut mesh,
                        [x * scale, -6. * scale, z],
                        [(x + 8.) * scale, 6. * scale, z + 10. * scale],
                    );
                }
                geometry::cuboid(
                    &mut mesh,
                    [-18. * scale, -6. * scale, z + 8. * scale],
                    [18. * scale, 6. * scale, z + 10. * scale],
                );
                geometry::label(
                    &mut mesh,
                    &format!("{:.0}", g.samples[i]),
                    [-17. * scale, -6. * scale, z + 2. * scale],
                    scale,
                );
            }
            geometry::cuboid(
                &mut mesh,
                [-20. * scale, -8. * scale, 0.],
                [20. * scale, 8. * scale, h],
            );
            mesh.translate([center.0 as f32, center.1 as f32, 0.]);
            g.objects.push(Object {
                name: "temperature tower".into(),
                mesh,
                flow_ratio: 1.,
            });
        }
        Kind::Retraction => {
            g.set("bottom_solid_layers", 3);
            g.set("retract_before_travel", 1);
            let mut mesh = TriangleMesh::default();
            for x in [-20., 14.] {
                geometry::cuboid(&mut mesh, [x, -3., 0.], [x + 6., 3., height]);
            }
            geometry::cuboid(&mut mesh, [-23., -6., 0.], [23., 6., 2. * h]);
            mesh.translate([center.0 as f32, center.1 as f32, 0.]);
            g.objects.push(Object {
                name: "retraction towers".into(),
                mesh,
                flow_ratio: 1.,
            });
        }
        Kind::PressureAdvanceTower | Kind::MaxVolumetricSpeed => {
            let width = if p.kind == Kind::MaxVolumetricSpeed {
                (maxx - minx - 30.).min(160.)
            } else {
                40.
            };
            if p.kind == Kind::MaxVolumetricSpeed {
                g.set("perimeters", 1);
                g.set(
                    "external_perimeter_extrusion_width",
                    s.nozzle_diameter * 1.75,
                );
                g.set("first_layer_extrusion_width", s.nozzle_diameter * 1.75);
                g.set("filament_max_volumetric_speed", 0);
                g.set("max_volumetric_speed", 0);
            }
            let mut mesh = TriangleMesh::default();
            geometry::cuboid(
                &mut mesh,
                [center.0 - width / 2., center.1 - 10., 0.],
                [center.0 + width / 2., center.1 + 10., height],
            );
            g.objects.push(Object {
                name: "calibration tower".into(),
                mesh,
                flow_ratio: 1.,
            });
        }
    }
    // Include extrusion and a margin in this preflight, before exporting anything.
    for obj in &g.objects {
        let (a, b) = obj.mesh.bounds();
        if f64::from(a[0]) < minx + 3.
            || f64::from(a[1]) < miny + 3.
            || f64::from(b[0]) > maxx - 3.
            || f64::from(b[1]) > maxy - 3.
        {
            return Err("calibration does not fit the bed; narrow the range".into());
        }
    }
    Ok(g)
}

impl Generated {
    fn set(&mut self, key: &str, value: impl ToString) {
        self.overrides.push((key.into(), value.to_string()));
    }
    /// `base` with the overrides. On a multi-tool printer a per-tool (vector) option changes
    /// in the tested tool's slot only; the other tools keep their values.
    pub fn config(&self, base: &Config) -> Result<Config, String> {
        let mut c = base.clone();
        let tools = base.vector_len("nozzle_diameter").max(1);
        let tool = self.parameters.tool;
        for (k, v) in &self.overrides {
            let old = (tools > 1).then(|| (c.get(k).cloned(), c.extra(k).map(str::to_owned)));
            c.set_raw(k, v).map_err(|e| e.to_string())?;
            // Imported Orca source keys can be typed without belonging to the canonical
            // schema. Replace their typed values as well as the retained raw text.
            use crate::config::{OptType, parse_value};
            let ty = match k.as_str() {
                "nozzle_temperature_initial_layer" | "nozzle_temperature" => Some(OptType::Ints),
                "slow_down_layer_time" => Some(OptType::Floats),
                "thick_internal_bridges" | "small_area_infill_flow_compensation" => {
                    Some(OptType::Bool)
                }
                "seam_slope_type" => Some(OptType::Enum),
                _ => None,
            };
            if let Some(ty) = ty {
                c.set(k, parse_value(ty, v).map_err(|e| format!("{k}: {e}"))?);
            }
            if let Some((typed, text)) = old {
                splice_tool(&mut c, k, typed, text, tool, tools);
            }
        }
        Ok(c)
    }
    pub fn combined_mesh(&self) -> TriangleMesh {
        let mut mesh = TriangleMesh::default();
        for object in &self.objects {
            geometry::append(&mut mesh, &object.mesh);
        }
        mesh
    }
    /// Prepare bounded calibration toolpaths. Export with the returned config/settings; the
    /// source supplies controls at the original path boundaries, before cooling and emission.
    ///
    /// On a printer with more than one tool the test is printed by `parameters.tool` through
    /// the print-level plan of a plate whose objects are all assigned to that tool (the way
    /// Orca and PrusaSlicer print a single-tool plate): the start templates select, pick up
    /// and heat that tool alone and the others stay parked and cold. A single-tool printer
    /// keeps the plain single-object export.
    pub fn prepare(&self, base: &Config) -> Result<CalibrationPrint, String> {
        let config = self.config(base)?;
        let tool = self.parameters.tool;
        let multi = config.vector_len("nozzle_diameter") > 1;
        let settings =
            PrintSettings::from_config(&tool_config(&config, tool)).map_err(|e| e.to_string())?;
        let assigned = ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]);
        let mut layers: Vec<LayerToolpaths> = Vec::new();
        let mut controls: Vec<Vec<Control>> = Vec::new();
        // Per layer and path: the index of the job object it belongs to (multi-tool only).
        let mut owners: Vec<Vec<usize>> = Vec::new();
        let mut job_objects: Vec<PrintObject> = Vec::new();
        if matches!(
            self.parameters.kind,
            Kind::PressureAdvanceLine | Kind::PressureAdvancePattern
        ) {
            for (layer, control) in self.pa_layers(&settings)? {
                owners.push(vec![0; layer.paths.len()]);
                layers.push(layer);
                controls.push(control);
            }
            if multi {
                // The direct paths need no slicing; the job object only carries the label,
                // footprint and per-tool settings: a box around the paths.
                let bbox = BoundingBox::from_points(
                    layers
                        .iter()
                        .flat_map(|l| l.paths.iter().flat_map(|p| &p.points)),
                )
                .ok_or("calibration has no paths")?;
                let top = layers.last().map_or(settings.layer_height, |l| l.print_z);
                let mut mesh = TriangleMesh::default();
                let mm = |v| crate::geometry::unscale(v);
                geometry::cuboid(
                    &mut mesh,
                    [mm(bbox.min.x), mm(bbox.min.y), 0.],
                    [mm(bbox.max.x), mm(bbox.max.y), top],
                );
                job_objects.push(PrintObject::prepare(
                    self.objects
                        .first()
                        .map_or_else(|| "pressure advance".into(), |o| o.name.clone()),
                    0,
                    0,
                    mesh,
                    assigned.clone(),
                    &config,
                )?);
            }
        } else {
            for (oi, object) in self.objects.iter().enumerate() {
                let (single, job_object) = if multi {
                    let o = PrintObject::prepare(
                        object.name.clone(),
                        oi,
                        0,
                        object.mesh.clone(),
                        assigned.clone(),
                        &config,
                    )?;
                    (None, Some(o))
                } else {
                    let prepared =
                        prepare_object(&object.mesh, &settings).map_err(|e| e.to_string())?;
                    (Some(prepared), None)
                };
                let prepared: &PreparedObject = match (&single, &job_object) {
                    (Some(p), _) => p,
                    (_, Some(o)) => &o.prepared,
                    _ => unreachable!(),
                };
                let n = prepared.layer_count();
                for mut layer in prepared.toolpaths(0..n) {
                    let i = layer.index;
                    if layers.len() <= i {
                        layers.push(LayerToolpaths {
                            paths: Vec::new(),
                            ..layer.clone()
                        });
                        controls.push(Vec::new());
                        owners.push(Vec::new());
                    }
                    owners[i].extend(std::iter::repeat_n(oi, layer.paths.len()));
                    let count = layer.paths.len();
                    for pi in 0..count {
                        let mut c = self.control(layer.print_z, i, n);
                        if self.parameters.kind == Kind::FlowRate {
                            c.flow_ratio = Some(object.flow_ratio);
                            c.sample = (pi == 0).then_some(self.samples[oi]);
                            // Orca limits calibration skin speed for the largest pass multiplier.
                            let max_ratio = if self.parameters.pass == 1 { 1.2 } else { 1. };
                            let cap = settings.filament_max_volumetric_speed;
                            if cap > 0. {
                                let path = &layer.paths[pi];
                                let nominal = if i == 0 {
                                    settings.first_layer_speed
                                } else {
                                    match path.role {
                                        Role::TopSolidInfill => settings.top_solid_infill_speed,
                                        Role::SolidInfill => settings.solid_infill_speed,
                                        Role::InternalInfill => settings.infill_speed,
                                        Role::ExternalPerimeter => {
                                            settings.external_perimeter_speed
                                        }
                                        Role::BridgeInfill
                                        | Role::InternalBridgeInfill
                                        | Role::OverhangPerimeter => settings.bridge_speed,
                                        _ => settings.perimeter_speed,
                                    }
                                };
                                c.speed_mm_s = Some(
                                    nominal
                                        .min(
                                            cap / (path.flow.mm3_per_mm()
                                                * settings.extrusion_multiplier
                                                * max_ratio),
                                        )
                                        .floor()
                                        .max(1.),
                                );
                            }
                        } else {
                            if self.parameters.kind == Kind::MaxVolumetricSpeed {
                                let area = layer.paths[pi].flow.mm3_per_mm()
                                    * settings.extrusion_multiplier;
                                c.speed_mm_s = Some((c.sample.unwrap() / area).round());
                            }
                            if self.parameters.kind == Kind::PressureAdvanceTower
                                && self.parameters.speed.is_none()
                                && i > 0
                            {
                                let cap = settings.filament_max_volumetric_speed;
                                let speed = settings.external_perimeter_speed.max(100.);
                                c.speed_mm_s = Some(if cap > 0. {
                                    speed.min(cap / layer.paths[pi].flow.mm3_per_mm()).floor()
                                } else {
                                    speed
                                });
                            }
                            if pi > 0 {
                                c.pressure_advance = None;
                                c.temperature = None;
                                c.sample = None;
                            }
                        }
                        controls[i].push(c);
                    }
                    layers[i].paths.append(&mut layer.paths);
                }
                job_objects.extend(job_object);
            }
        }
        let bbox = layers
            .first()
            .and_then(|l| BoundingBox::from_points(l.paths.iter().flat_map(|p| &p.points)));
        let job = multi.then(|| {
            let z: Vec<_> = layers.iter().map(|l| (l.print_z, l.height)).collect();
            PreparedPrint::single_tool(job_objects, &z, tool)
        });
        Ok(CalibrationPrint {
            config,
            settings,
            layers,
            controls,
            owners,
            job,
            tool,
            bbox,
        })
    }
    fn control(&self, z: f64, index: usize, count: usize) -> Control {
        let p = &self.parameters;
        let mut c = Control::default();
        match p.kind {
            Kind::PressureAdvanceTower => {
                let v = p.start + z.floor() * p.step;
                c.pressure_advance = Some(v);
                c.sample = Some(v);
                c.speed_mm_s = p.speed;
            }
            Kind::Temperature => {
                // Exact f32/truncation policy in Orca GCode::interpolate_value_across_layers.
                let v = if index <= 1 {
                    p.start
                } else {
                    let start = p.start as f32 + 5.;
                    let end = p.end as f32;
                    let value = start + index as f32 / (count as f32 - 1.) * (end - start);
                    f64::from((value / 5.).trunc() * 5.)
                };
                c.temperature = Some(v);
                c.sample = Some(v);
            }
            Kind::Retraction => {
                let v = p.start + (z - 0.4).max(0.).floor() * p.step;
                c.retract_length = Some(v);
                c.sample = Some(v);
            }
            Kind::MaxVolumetricSpeed => {
                let v = p.start + z * p.step;
                let area = Flow::new(self.nozzle * 1.75, self.nozzle * 0.8).mm3_per_mm();
                c.speed_mm_s = Some((v / area).round());
                c.sample = Some(v);
            }
            _ => {}
        }
        c
    }
    fn pa_layers(&self, s: &PrintSettings) -> Result<Vec<(LayerToolpaths, Vec<Control>)>, String> {
        let p = &self.parameters;
        let line = p.kind == Kind::PressureAdvanceLine;
        let width = if line {
            if self.nozzle < 0.51 {
                self.nozzle * 1.5
            } else {
                self.nozzle * 1.05
            }
        } else {
            self.nozzle * 1.125
        };
        let flow = Flow::new(width, self.nozzle / 2.);
        let volume = s.filament_max_volumetric_speed;
        let fast = p.speed.unwrap_or_else(|| {
            s.external_perimeter_speed
                .max(100.)
                .min(if volume > 0. {
                    volume / flow.mm3_per_mm()
                } else {
                    f64::INFINITY
                })
                .floor()
        });
        let slow = (fast / 10.).max(10.);
        let fast = fast.max(slow + 5.);
        let n = self.samples.len();
        let mut out = Vec::new();
        for li in 0..if line { 1 } else { 4 } {
            let h = self.nozzle / 2.;
            let mut layer = LayerToolpaths {
                support_groups: Vec::new(),
                index: li,
                print_z: (li + 1) as f64 * h,
                height: h,
                paths: Vec::new(),
            };
            let mut controls = Vec::new();
            let mut push = |pts: &[(f64, f64)], speed, pa, sample| {
                layer.paths.push(ExtrusionPath {
                    role: Role::ExternalPerimeter,
                    perimeter_index: Some(0),
                    points: pts.iter().map(|&(x, y)| Point::from_mm(x, y)).collect(),
                    flow,
                    seam: None,
                    overhang: None,
                    brick: None,
                });
                controls.push(Control {
                    pressure_advance: pa,
                    speed_mm_s: Some(speed),
                    sample,
                    ..Default::default()
                });
            };
            if line {
                let x = self.center.0 - 40.;
                let y = self.center.1 - (n - 1) as f64 * 3.5 / 2.;
                push(&[(x, y + n as f64 * 3.5), (x, y)], slow, Some(0.), None);
                for (i, &value) in self.samples.iter().enumerate() {
                    let y = y + i as f64 * 3.5;
                    push(&[(x, y), (x + 20., y)], slow, Some(value), Some(value));
                    push(&[(x + 20., y), (x + 60., y)], fast, None, None);
                    push(&[(x + 60., y), (x + 80., y)], slow, None, None);
                }
                push(
                    &[(x + 80., y), (x + 80., y + n as f64 * 3.5)],
                    slow,
                    Some(0.),
                    None,
                );
            } else {
                let spacing = flow.spacing();
                let shift = 2. * spacing * 2_f64.sqrt() + 2. + width;
                let start = self.center.0 - ((n - 1) as f64 * shift + 30. / 2_f64.sqrt()) / 2.;
                let y = self.center.1 - 30. / 2_f64.sqrt();
                // Four layers, three nested 90° chevrons, 30 mm sides, 2 mm between samples.
                if li == 0 {
                    let end = start + (n - 1) as f64 * shift + 30. / 2_f64.sqrt() + 3. * spacing;
                    push(
                        &[
                            (start - spacing, y - spacing),
                            (end, y - spacing),
                            (end, y + 30. * 2_f64.sqrt() + spacing),
                            (start - spacing, y + 30. * 2_f64.sqrt() + spacing),
                            (start - spacing, y - spacing),
                        ],
                        30.,
                        Some(p.start),
                        None,
                    );
                }
                for (i, &value) in self.samples.iter().enumerate() {
                    for wall in 0..3 {
                        let x = start + i as f64 * shift + wall as f64 * spacing * 2_f64.sqrt();
                        let side = 30. / 2_f64.sqrt();
                        push(
                            &[(x, y), (x + side, y + side), (x, y + 2. * side)],
                            if li == 0 { 30. } else { fast },
                            Some(value),
                            (wall == 0).then_some(value),
                        );
                    }
                }
            }
            out.push((layer, controls));
        }
        Ok(out)
    }
}

/// Puts the new single value of `key` into slot `tool` of its previous per-tool vector
/// (typed and option text), so the other tools keep theirs.
fn splice_tool(
    c: &mut Config,
    key: &str,
    typed: Option<crate::config::ConfigValue>,
    text: Option<String>,
    tool: usize,
    tools: usize,
) {
    use crate::config::ConfigValue;
    if let (Some(ConfigValue::Vector(old)), Some(ConfigValue::Vector(new))) = (typed, c.get(key))
        && new.len() == 1
        && !old.is_empty()
    {
        let value = new[0].clone();
        let mut merged: Vec<_> = (0..tools.max(old.len()))
            .map(|i| old.get(i).unwrap_or(&old[0]).clone())
            .collect();
        if let Some(slot) = merged.get_mut(tool) {
            *slot = value;
            c.set(key, ConfigValue::Vector(merged));
        }
    }
    if let (Some(old), Some(new)) = (text, c.extra(key).map(str::to_owned)) {
        let mut values: Vec<&str> = old.split(',').collect();
        if values.len() > 1 && !new.contains(',') {
            values.resize(tools.max(values.len()), values[0]);
            if let Some(slot) = values.get_mut(tool) {
                *slot = &new;
                let joined = values.join(",");
                let _ = c.set_raw(key, &joined);
            }
        }
    }
}

pub struct CalibrationPrint {
    pub config: Config,
    /// The tested tool's settings (its vector entries of `config`).
    pub settings: PrintSettings,
    layers: Vec<LayerToolpaths>,
    controls: Vec<Vec<Control>>,
    owners: Vec<Vec<usize>>,
    /// The single-tool plate plan on a multi-tool printer; `None` on a single-tool printer.
    job: Option<PreparedPrint>,
    tool: usize,
    bbox: Option<BoundingBox>,
}
impl CalibrationPrint {
    /// The zero-based tool that prints the test.
    pub fn tool(&self) -> usize {
        self.tool
    }
}
impl LayerSource for CalibrationPrint {
    fn print_job(&self) -> Option<&PreparedPrint> {
        self.job.as_ref()
    }
    fn scheduled_toolpaths(&self, range: std::ops::Range<usize>) -> Vec<ScheduledLayer> {
        range
            .map(|i| ScheduledLayer {
                layer: self.layers[i].clone(),
                assignments: if self.job.is_some() {
                    self.owners[i]
                        .iter()
                        .map(|&object| PathAssignment {
                            object,
                            tool: self.tool,
                            region: None,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
            })
            .collect()
    }
    fn calibration_control(&self, layer: usize, path: usize) -> Option<Control> {
        self.controls.get(layer)?.get(path).copied()
    }
    fn layer_count(&self) -> usize {
        self.layers.len()
    }
    fn layer_z(&self, i: usize) -> (f64, f64) {
        (self.layers[i].print_z, self.layers[i].height)
    }
    fn first_layer_bbox(&self) -> Option<BoundingBox> {
        self.bbox
    }
    fn toolpaths(&self, range: std::ops::Range<usize>) -> Vec<LayerToolpaths> {
        self.layers[range].to_vec()
    }
}

#[cfg(test)]
mod tests;
