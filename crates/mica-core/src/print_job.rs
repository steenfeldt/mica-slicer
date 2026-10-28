// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::collect_layers_to_print, GCodeGenerator::process_layer
// Source: orca@8500fcdcca src/libslic3r/Support/SupportCommon.cpp :: generate_support_toolpaths
// Changes: A job layer's support islands are the union of its sources' islands at their instance shifts.
// Source: orca@8500fcdcca src/libslic3r/GCode/RetractWhenCrossingPerimeters.cpp :: RetractWhenCrossingPerimeters::travel_inside_internal_regions
// Changes: A job layer's region slices list every source's regions (a single-region source as one).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: sort_print_object_instances
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: translate_support_extruder
// Changes: Don't-care support tools prefer the first non-soluble layer tool under Prusa rules; fixed organic collections survive scheduling and instance shifts.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: PrintConfigDef::init_fff_params
// Source: prusaslicer@b028299c77 src/libslic3r/ShortestPath.cpp :: chain_print_object_instances
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: calc_max_layer_height, fill_wipe_tower_partitions, insert_wipe_tower_extruder
// Changes: Prepared objects/parts/instances (instances differing by an XY shift share one prepared object, paths shifted when scheduled), compact Z schedule, region-indexed path/tool/config annotations, facet seam adapter, plate adhesion and lazy tower plan.
// Changes: Plain objects may carry their mesh as loaded for seam visibility (PrusaSlicer's model frame).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: sort_object_instances_by_model_order, GCodeGenerator::_do_export
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::validate
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export
// Changes: Sequential printing (complete_objects / Orca "by object") as one job of concatenated per-instance layer runs in model order:
// Changes: object-local layer numbers, no tower, PrusaSlicer's skirt and brim with the first object, Orca's brim with each object.
// Changes: Single-tool plans around externally generated layers (calibration tests).
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::ToolOrdering, ToolOrdering::collect_extruders
// Changes: Per-layer custom G-code: MultiAsSingle tool changes override the object extruders from their layer up, and a
// Changes: MultiExtruder colour change makes its extruder print first on its layer.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintApply.cpp :: print_config_diffs
// Changes: Per-tool configurations apply the filament retraction overrides before slicing vectors.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::reorder_extruders
// Changes: toolchange_ordering: Cyclic keeps the sorted tool order; sequential prints and Orca rules always optimize.
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer
// Changes: Orca don't-care support tools: a Default base under an explicit interface avoids the interface tool and soluble
// Changes: filaments; otherwise the layer's first tool gives way to a non-soluble, then a non-support filament.
// Source: orca@8500fcdcca src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::collect_extruders
// Changes: Support-only layers print a Default base under an explicit interface with the cheapest-to-flush used filament
// Changes: (support_interface_not_for_body excludes the interface filament); flush volumes from flush_volumes_matrix.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::object_extruders
// Source: prusaslicer@b028299c77 src/libslic3r/PrintRegion.cpp :: PrintRegion::collect_object_printing_extruders
// Changes: Object extruders from the role tools of the object and its model/modifier volumes; painting counts as several.
// Changes: Print::validate's wipe tower rules for normal supports (soluble: synchronized layers; else Default tools).
// Changes: Per-tool configurations keep every tool's filament_soluble / filament_is_support (SupportParameters reads them by index).
// Changes: With a wipe tower the first layer starts with a soluble tool (reorder_extruders; PrusaSlicer rules).
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::collect_layers_to_print
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: get_extrusions, get_overriden_extrusions
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::_make_wipe_tower
// Changes: Purge into infill/objects: overridable paths drop their tool in the usage prepass (collect_extruders), paths the
// Changes: wiping rules force onto a tool print first with it; tower layers are where ensure_perimeters_infills_order runs.
// Changes: An object whose first layer prints nothing stops the slice with each slicer's own error (collect_layers_to_print).

// Source: prusaslicer@b028299c77 src/libslic3r/Feature/FullSpectrum/VirtualExtruder.cpp :: expand_virtual_extruders_0based
// Changes: Project-aware virtual model assignments; object tool discovery expands recipes into physical tools.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintRegion.cpp :: PrintRegion::flow
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: PrintObject::combine_infill
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::combine_infill
// Changes: Region settings resolve automatic infill widths for the infill tools' nozzles and bound combined infill by
// Changes: the smaller infill nozzle; a mixed color counts the smallest nozzle it alternates through.

//! Print-level scheduling. Geometry remains in independently prepared objects; only a requested
//! batch of finished paths is merged. Parts with different assignments share their instance label.

use crate::config::{Config, ConfigValue};
use crate::geometry::{BoundingBox, ExPolygon, Point};
use crate::mesh::TriangleMesh;
use crate::print::{LayerSource, PreparedObject};
use crate::settings::PrintSettings;
use crate::threemf::Settings;
use crate::tool_ordering::{
    EntityKind, RegionTools, ToolOrdering, WipeInto, WipingExtrusions, entity_kind,
};
use crate::toolpath::{LayerToolpaths, Role};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolAssignments(pub [Option<usize>; 5]);
impl ToolAssignments {
    /// Metadata and CLI use one-based tools. Zero support tools mean the current layer tool.
    pub fn resolve(
        c: &Config,
        object: &Settings,
        part: &Settings,
        default: usize,
        count: usize,
    ) -> Result<Self, String> {
        Self::resolve_with_virtuals(c, object, part, default, count, &Default::default())
    }

    /// Accept a model-role virtual tool only when its validated project recipe exists.
    pub fn resolve_with_virtuals(
        c: &Config,
        object: &Settings,
        part: &Settings,
        default: usize,
        count: usize,
        spectrum: &crate::full_spectrum::FullSpectrum,
    ) -> Result<Self, String> {
        if default >= count {
            return Err(format!(
                "default tool {} exceeds printer's {count} tools",
                default + 1
            ));
        }
        let parse = |key: &str, raw: &str| {
            raw.parse::<usize>()
                .map_err(|_| format!("invalid {key}={raw:?}"))
        };
        let assigned = part
            .get("extruder")
            .or_else(|| object.get("extruder"))
            .map(|v| parse("extruder", v))
            .transpose()?
            .filter(|n| *n != 0);
        let keys = [
            "perimeter_extruder",
            "infill_extruder",
            "solid_infill_extruder",
            "support_material_extruder",
            "support_material_interface_extruder",
        ];
        let mut out = [None; 5];
        for (i, key) in keys.iter().enumerate() {
            let value = part
                .get(*key)
                .or_else(|| object.get(*key))
                .map(|v| parse(key, v))
                .transpose()?
                .unwrap_or_else(|| {
                    if i < 3 {
                        assigned.unwrap_or_else(|| {
                            let n = c.int(key).unwrap_or(1).max(0) as usize;
                            if n == 1 { default + 1 } else { n }
                        })
                    } else {
                        c.int(key).unwrap_or(0).max(0) as usize
                    }
                });
            if value > count && !(i < 3 && spectrum.contains(value)) {
                return Err(format!(
                    "{key} selects tool {value}, printer has {count} tools"
                ));
            }
            out[i] = if value == 0 {
                if i < 3 { Some(default) } else { None }
            } else {
                Some(value - 1)
            };
        }
        Ok(Self(out))
    }
    pub fn tool(&self, role: Role) -> Option<usize> {
        self.0[match role {
            Role::InternalInfill => 1,
            Role::SolidInfill
            | Role::TopSolidInfill
            | Role::BridgeInfill
            | Role::InternalBridgeInfill
            | Role::Ironing => 2,
            Role::SupportMaterial => 3,
            Role::SupportMaterialInterface => 4,
            _ => 0,
        }]
    }
}

/// Select a tool's vector entries for the existing scalar settings resolver. Machine-mode vectors
/// (normal/silent) and geometric point vectors are not per-extruder options. Filament
/// properties that upstream reads from the print config by another tool's index (whether the
/// support or interface filament is soluble or a support material, `SupportParameters`) keep
/// every tool's entry.
pub fn tool_config(c: &Config, tool: usize) -> Config {
    // Filament retraction overrides apply per extruder before the vectors are sliced.
    let c = &crate::filament_overrides::apply(c);
    let mut out = c.clone();
    for (key, value) in c.values() {
        if !key.starts_with("machine_")
            && key != "bed_shape"
            && key != "extruder_offset"
            && key != "small_area_infill_flow_compensation_model"
            && key != "filament_soluble"
            && key != "filament_is_support"
            && let ConfigValue::Vector(v) = value
            && let Some(value) = v.get(tool).or(v.first())
        {
            out.set(key, ConfigValue::Vector(vec![value.clone()]));
        }
    }
    out
}

/// Settings of a region whose walls print with `tools.0[0]` (zero-based physical tools), with
/// the infill roles resolved for their own tools' nozzles as upstream `PrintRegion::flow` does:
/// automatic sparse infill widths from the sparse infill tool, solid and top ones from the solid
/// infill tool. Combined infill stays within the smaller nozzle of the two infill tools
/// (`PrintObject::combine_infill`); `logical` are the same roles before mixed colors resolve
/// to physical tools, so a mix bounds it by the smallest nozzle it alternates through.
pub(crate) fn region_settings(
    c: &Config,
    tools: &ToolAssignments,
    logical: &ToolAssignments,
    spectrum: &crate::full_spectrum::FullSpectrum,
) -> Result<PrintSettings, String> {
    let resolve =
        |tool: usize| PrintSettings::from_config(&tool_config(c, tool)).map_err(|e| e.to_string());
    let wall = tools.0[0].unwrap_or(0);
    let mut s = resolve(wall)?;
    let nozzle = |tool: usize| c.float_at("nozzle_diameter", tool);
    let other = |tool: Option<usize>| {
        tool.filter(|t| *t != wall && nozzle(*t) != nozzle(wall))
            .map(resolve)
            .transpose()
    };
    if let Some(r) = other(tools.0[1])? {
        s.infill_extrusion_width = r.infill_extrusion_width;
    }
    if let Some(r) = other(tools.0[2])? {
        s.solid_infill_extrusion_width = r.solid_infill_extrusion_width;
        s.top_infill_extrusion_width = r.top_infill_extrusion_width;
    }
    // Upstream identifies a mixed region by its wall tool and prints every role with the mix.
    let mix = logical.0[0]
        .map(|t| t + 1)
        .filter(|id| spectrum.contains(*id));
    s.infill_combine_nozzle = logical.0[1..3]
        .iter()
        .flatten()
        .flat_map(|t| spectrum.expand(mix.unwrap_or(t + 1)))
        .chain(tools.0[1..3].iter().flatten().map(|t| t + 1))
        .filter_map(|id| nozzle(id - 1))
        .fold(f64::INFINITY, f64::min);
    if !s.infill_combine_nozzle.is_finite() {
        s.infill_combine_nozzle = s.nozzle_diameter;
    }
    Ok(s)
}

/// Whether each layer starts with the extruder that ended the layer below
/// (`ToolOrdering::reorder_extruders`). PrusaSlicer's Cyclic ordering keeps the sorted tool order
/// (1, 2, 3, ...) on every layer, but only for a print-wide ordering: a sequential print orders
/// each object without the print config, which always optimizes. OrcaSlicer has no
/// `toolchange_ordering` (it orders tools to minimize flushing); Orca rules ignore it.
pub fn optimized_tool_ordering(c: &Config) -> bool {
    c.string("toolchange_ordering") != Some("cyclic")
        || crate::sequential::enabled(c)
        || crate::sequential::orca_rules(c)
}

pub(crate) fn resolved_tools(c: &Config) -> Result<Vec<(Config, PrintSettings)>, String> {
    (0..c.vector_len("nozzle_diameter").max(1))
        .map(|tool| {
            let config = tool_config(c, tool);
            let settings = PrintSettings::from_config(&config).map_err(|e| e.to_string())?;
            Ok((config, settings))
        })
        .collect()
}

pub struct PrintObject {
    pub name: String,
    pub object_id: usize,
    pub instance_id: usize,
    /// This instance's placed model volumes (labels, bounds, ordering).
    pub mesh: TriangleMesh,
    pub tools: ToolAssignments,
    /// Slices, walls, fills and supports, shared by every instance that differs from it only
    /// by an XY translation (upstream's `PrintObject` with several `PrintInstance`s).
    pub prepared: Arc<PreparedObject>,
    /// Scaled XY offset of this instance from `prepared`'s coordinates; zero for the instance
    /// that was prepared (upstream's `PrintInstance::shift`, relative to the first instance).
    pub shift: Point,
    pub config: Config,
    by_tool: Vec<(Config, PrintSettings)>,
}
impl PrintObject {
    pub fn prepare(
        name: String,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        tools: ToolAssignments,
        c: &Config,
    ) -> Result<Self, String> {
        Self::prepare_observed(
            name,
            object_id,
            instance_id,
            mesh,
            tools,
            c,
            &crate::progress::Silent,
        )
    }

    /// [`Self::prepare`] reporting the preparation stages to `observer`, which may cancel it.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_observed(
        name: String,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        tools: ToolAssignments,
        c: &Config,
        observer: &dyn crate::progress::Observer,
    ) -> Result<Self, String> {
        Self::prepare_in_frame_observed(
            name,
            object_id,
            instance_id,
            mesh,
            tools,
            c,
            None,
            observer,
        )
    }

    /// [`Self::prepare_observed`] for a plain object with its model frame (see
    /// [`crate::input::InputObject::frame`]): `mesh` is the frame placed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_in_frame_observed(
        name: String,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        tools: ToolAssignments,
        c: &Config,
        frame: Option<&crate::input::Frame>,
        observer: &dyn crate::progress::Observer,
    ) -> Result<Self, String> {
        let mut s = region_settings(c, &tools, &tools, &Default::default())?;
        if let Some(tree) = s.tree3d.as_mut() {
            tree.set_object_extruders(&object_extruders(&tools, None, c));
        }
        let prepared = crate::print::prepare_with_source(
            &mesh,
            &s,
            &crate::seam::painting::NoPainting,
            None,
            frame,
            observer,
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            name,
            object_id,
            instance_id,
            mesh,
            tools,
            prepared: Arc::new(prepared),
            shift: Point::default(),
            config: c.clone(),
            by_tool: resolved_tools(c)?,
        })
    }
    /// Prepare all model, modifier, negative and support volumes on one object layer grid.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_regions(
        name: String,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        model: &crate::regions::RegionModel,
        tools: ToolAssignments,
        c: &Config,
    ) -> Result<Self, String> {
        Self::prepare_regions_observed(
            name,
            object_id,
            instance_id,
            mesh,
            model,
            tools,
            c,
            &crate::progress::Silent,
        )
    }

    /// [`Self::prepare_regions`] reporting the stages to `observer`, which may cancel it.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_regions_observed(
        name: String,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        model: &crate::regions::RegionModel,
        tools: ToolAssignments,
        c: &Config,
        observer: &dyn crate::progress::Observer,
    ) -> Result<Self, String> {
        model.validate()?;
        let logical = tools;
        let tools = ToolAssignments(
            logical
                .0
                .map(|t| t.map(|t| model.full_spectrum.resolve(t + 1, 0) - 1)),
        );
        let mut s = region_settings(c, &tools, &logical, &model.full_spectrum)?;
        if let Some(tree) = s.tree3d.as_mut() {
            tree.set_object_extruders(&object_extruders(&tools, Some(model), c));
        }
        let painting = crate::painting::query::PaintedQuery::new(model, false);
        let prepared =
            crate::print::prepare_with_regions(&mesh, &s, &painting, Some((model, c)), observer)
                .map_err(|e| e.to_string())?;
        Ok(Self {
            name,
            object_id,
            instance_id,
            mesh,
            tools,
            prepared: Arc::new(prepared),
            shift: Point::default(),
            config: c.clone(),
            by_tool: resolved_tools(c)?,
        })
    }

    /// Another instance of this prepared object, `shift` (scaled XY) away from this one, with
    /// its own identity and placed mesh. The caller guarantees that the instance's geometry,
    /// settings, tools, modifiers and painting equal this object's moved by `shift`
    /// ([`crate::instances::group`]). Shared objects print in a plate, whose plan supersedes
    /// the object's own skirt and brim, so that plan is dropped here.
    pub fn instance(
        &mut self,
        name: String,
        object_id: usize,
        instance_id: usize,
        mesh: TriangleMesh,
        shift: Point,
    ) -> Self {
        if let Some(prepared) = Arc::get_mut(&mut self.prepared) {
            prepared.adhesion = crate::brim::AdhesionPlan::default();
        }
        Self {
            name,
            object_id,
            instance_id,
            mesh,
            tools: self.tools.clone(),
            prepared: self.prepared.clone(),
            shift: Point::new(self.shift.x + shift.x, self.shift.y + shift.y),
            config: self.config.clone(),
            by_tool: self.by_tool.clone(),
        }
    }

    /// Whether the object's first layer (its lowest one with a slice or a path) prints
    /// anything: its own extrusions or supports.
    pub fn first_layer_prints(prepared: &PreparedObject) -> bool {
        let n = prepared.layer_count();
        let Some(sliced) =
            (0..n).find(|&i| prepared.layer_outlines(i).is_some_and(|o| !o.is_empty()))
        else {
            // No layer with a slice (an XY compensation larger than the object, a model too
            // thin for any layer): as upstream, whose first object layer then has no
            // extrusions, it prints only if its first layer has paths of its own (none at all:
            // Orca-rule objects drop the empty layers at the top, so all of them).
            return n > 0 && prepared.toolpaths(0..1).iter().any(|l| !l.paths.is_empty());
        };
        prepared
            .toolpaths(0..sliced + 1)
            .iter()
            .any(|l| !l.paths.is_empty())
    }

    /// The prepared object of a single, unshared instance (the CLI's single-object source).
    pub fn into_prepared(self) -> Result<PreparedObject, String> {
        if self.shift != Point::default() {
            return Err("a shifted instance has no geometry of its own".into());
        }
        Arc::try_unwrap(self.prepared)
            .map_err(|_| "the prepared object is shared by other instances".to_string())
    }

    /// Outlines of object layer `index`, at this instance's position.
    pub fn layer_outlines(&self, index: usize) -> Option<std::borrow::Cow<'_, [ExPolygon]>> {
        use std::borrow::Cow;
        let outlines = self.prepared.layer_outlines(index)?;
        Some(if self.shift == Point::default() {
            Cow::Borrowed(outlines)
        } else {
            Cow::Owned(crate::instances::shifted_polygons(outlines, self.shift))
        })
    }

    pub fn path_config(&self, region: Option<usize>, tool: usize) -> (&Config, &PrintSettings) {
        let table = region
            .and_then(|r| self.prepared.region(r))
            .map_or(&self.by_tool, |r| &r.by_tool);
        let (config, settings) = &table[tool];
        (config, settings)
    }
    fn path_tool(&self, region: Option<usize>, role: Role) -> Option<usize> {
        region
            .and_then(|r| self.prepared.region(r))
            .map_or(&self.tools, |r| &r.tools)
            .tool(role)
    }
    /// [`Self::path_tool`] under a per-layer tool switch, which replaces the wall and infill
    /// tools (supports keep theirs, as upstream `collect_extruders`).
    fn path_tool_at(
        &self,
        region: Option<usize>,
        role: Role,
        switch: Option<usize>,
    ) -> Option<usize> {
        match switch {
            Some(t) if !matches!(role, Role::SupportMaterial | Role::SupportMaterialInterface) => {
                Some(t)
            }
            _ => self.path_tool(region, role),
        }
    }
    /// The purge-absorbing options of a region: PrusaSlicer reads `wipe_into_infill` per region
    /// (a modifier may set it) and `wipe_into_objects` per object. OrcaSlicer's
    /// `flush_into_infill` / `flush_into_objects` (read into the same keys) are object settings,
    /// which every region of the object inherits; Mica also lets a modifier set the first one
    /// on Orca printers.
    pub(crate) fn wipe_into(&self, region: Option<usize>) -> WipeInto {
        let flag = |c: &Config, key: &str| c.bool(key).unwrap_or(false);
        WipeInto {
            infill: flag(self.path_config(region, 0).0, "wipe_into_infill"),
            objects: flag(&self.config, "wipe_into_objects"),
        }
    }
    /// Whether any region of the object may absorb a purge.
    fn wipes(&self) -> bool {
        self.wipe_into(None).any()
            || (0..self.prepared.region_count()).any(|r| self.wipe_into(Some(r)).any())
    }
    /// A region's wall, solid and sparse infill tools under a per-layer tool switch.
    fn region_tools(&self, region: Option<usize>, switch: Option<usize>) -> Option<RegionTools> {
        Some(RegionTools {
            perimeter: self.path_tool_at(region, Role::Perimeter, switch)?,
            solid: self.path_tool_at(region, Role::SolidInfill, switch)?,
            infill: self.path_tool_at(region, Role::InternalInfill, switch)?,
        })
    }
    /// Upstream `collect_extruders` for an object layer whose regions may absorb purges: the
    /// tools its paths need. Overridable walls and fills do not ask for their own tool; a layer
    /// left with no tool at all asks for "don't care" (`None`, the current tool).
    fn wiping_layer_tools(
        &self,
        w: &WipingExtrusions,
        paths: &[crate::toolpath::ExtrusionPath],
        regions: &[Option<usize>],
        switch: Option<usize>,
    ) -> Vec<Option<usize>> {
        let mut ts = Vec::new();
        let mut by_region: BTreeMap<Option<usize>, Vec<Role>> = BTreeMap::new();
        for (pi, p) in paths.iter().enumerate() {
            let region = regions.get(pi).copied().flatten();
            if entity_kind(p.role) != EntityKind::Other
                && self.region_tools(region, switch).is_some()
            {
                by_region.entry(region).or_default().push(p.role);
            } else {
                let t = self.path_tool_at(region, p.role, switch);
                if !ts.contains(&t) {
                    ts.push(t);
                }
            }
        }
        for (region, roles) in by_region {
            if let Some(tools) = self.region_tools(region, switch) {
                w.collect_region(roles, tools, self.wipe_into(region), &mut ts);
            }
        }
        if ts.is_empty() && !paths.is_empty() {
            ts.push(None);
        }
        ts
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathAssignment {
    /// Index into PreparedPrint::objects (parts may share the same object/instance identity).
    pub object: usize,
    pub tool: usize,
    pub region: Option<usize>,
}
#[derive(Clone, Default)]
pub struct ScheduledLayer {
    pub layer: LayerToolpaths,
    /// Empty for a legacy single-object source, otherwise one entry per path.
    pub assignments: Vec<PathAssignment>,
}

/// `PrintObject::object_extruders`: the zero-based tools printing the object's walls, sparse
/// and solid infill, over its model and modifier volumes. Painted material makes it more than
/// one tool (painting always adds the painted tools to the object's regions).
pub(crate) fn object_extruders(
    tools: &ToolAssignments,
    model: Option<&crate::regions::RegionModel>,
    c: &Config,
) -> Vec<usize> {
    use crate::threemf::PartKind;
    let spectrum = model
        .map(|m| &m.full_spectrum)
        .filter(|s| !s.extruders().is_empty());
    let mut out: Vec<usize> = tools.0[..3].iter().flatten().copied().collect();
    if let Some(spectrum) = spectrum {
        for key in [
            "perimeter_extruder",
            "infill_extruder",
            "solid_infill_extruder",
        ] {
            if let Some(id) = c.int(key).filter(|id| *id > 0) {
                out.extend(spectrum.expand(id as usize).into_iter().map(|id| id - 1));
            }
        }
    }
    let count = c.vector_len("nozzle_diameter").max(1);
    for volume in model.map_or(&[][..], |m| &m.volumes[..]) {
        if !matches!(volume.kind, PartKind::Model | PartKind::Modifier) {
            continue;
        }
        if !volume.painting.material.is_empty() {
            out.push(usize::MAX);
        }
        if volume.config.values().next().is_none() {
            continue;
        }
        let config = crate::regions::apply_config(c, &volume.config);
        let default = tools.0[0].unwrap_or(0);
        if let Ok(t) = ToolAssignments::resolve_with_virtuals(
            &config,
            &Default::default(),
            &Default::default(),
            default.min(count - 1),
            count,
            spectrum.unwrap_or(&Default::default()),
        ) {
            for tool in t.0[..3].iter().flatten() {
                if let Some(spectrum) = spectrum {
                    out.extend(spectrum.expand(tool + 1).into_iter().map(|id| id - 1));
                } else {
                    out.push(*tool);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A per-filament boolean, also when it is kept as Orca text among the extras
/// (`filament_is_support` has no PrusaSlicer definition).
pub(crate) fn filament_flag(config: &Config, key: &str, tool: usize) -> bool {
    config.bool_at(key, tool).unwrap_or_else(|| {
        config.extra(key).is_some_and(|raw| {
            let values: Vec<&str> = raw.split(',').collect();
            values
                .get(tool)
                .or(values.first())
                .is_some_and(|v| matches!(v.trim().trim_matches('"'), "1" | "true"))
        })
    })
}

/// `translate_support_extruder` for a configured zero ("don't care"). Explicit tools
/// have already been resolved by ToolAssignments; `assigned` holds the object's (or region's)
/// base and interface tools. PrusaSlicer takes the layer's first non-soluble tool. Orca
/// (`GCode::process_layer`) prints a don't-care base under an explicit interface with the
/// layer's first non-soluble tool other than the interface's; otherwise it starts from the
/// layer's first tool and moves on to a non-soluble, then a non-support filament.
fn dont_care_support_tool(
    config: &Config,
    tools: &[usize],
    role: Role,
    assigned: (Option<usize>, Option<usize>),
) -> usize {
    let soluble = |t: usize| config.bool_at("filament_soluble", t).unwrap_or(false);
    if !crate::rules::Rules::of(config).is_orca() {
        return tools
            .iter()
            .copied()
            .find(|&tool| !soluble(tool))
            .unwrap_or(tools[0]);
    }
    if let (Role::SupportMaterial, (None, Some(interface))) = (role, assigned) {
        return tools
            .iter()
            .copied()
            .find(|&t| !soluble(t) && t != interface)
            .unwrap_or(tools[0]);
    }
    let support = |t: usize| filament_flag(config, "filament_is_support", t);
    let mut tool = tools[0];
    if soluble(tool)
        && let Some(t) = tools.iter().copied().find(|&t| !soluble(t))
    {
        tool = t;
    }
    if support(tool)
        && let Some(t) = tools.iter().copied().find(|&t| !support(t))
    {
        tool = t;
    }
    tool
}
struct JobLayer {
    z: f64,
    height: f64,
    sources: Vec<(usize, usize)>,
    outlines: Vec<ExPolygon>,
    /// Top fill surfaces of every source at this Z, placed like `outlines`.
    tops: Vec<ExPolygon>,
    skirt: Option<(usize, f64)>,
    /// Sequential printing: which instance this layer belongs to and its layer number within
    /// that instance (layer-by-layer printing has none).
    sequence: Option<Sequence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sequence {
    /// Position of the instance in the print order (0 for the first object printed).
    ordinal: usize,
    /// Layer number within the instance; 0 is its first layer, on the bed.
    local: usize,
    /// The instance's first object (part group) in `PreparedPrint::objects`.
    object: usize,
}

/// The start of an object printed sequentially (one object after another).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectStart {
    /// Objects printed before this one.
    pub ordinal: usize,
    /// The instance's first object in [`PreparedPrint::objects`].
    pub object: usize,
}

pub struct PreparedPrint {
    pub objects: Vec<PrintObject>,
    pub ordering: ToolOrdering,
    pub adhesion: crate::brim::AdhesionPlan,
    pub wipe_tower: Option<crate::wipe_tower::TowerPlan>,
    /// Problems that do not stop the print (PrusaSlicer's sequential collision notice).
    pub warnings: Vec<String>,
    layers: Vec<JobLayer>,
    object_order: Vec<usize>,
    /// Per object, the index of its shared prepared object in `leaders`.
    group_of: Vec<usize>,
    /// Per shared prepared object, the first object (instance) that uses it.
    leaders: Vec<usize>,
    /// Per shared prepared object, the layer toolpaths the tool-usage prepass already generated,
    /// keyed by object layer index. Export batches take them instead of generating the same
    /// paths again; a layer that is missing (or requested twice) is generated on demand exactly
    /// as before. Instances move their copy of a path by their shift.
    prepass: Vec<Mutex<RetainedLayers>>,
    /// The plate's per-layer custom G-code (colour changes, pauses, tool switches).
    pub custom_gcode: crate::custom_gcode::Info,
    /// Tool switches of a single-tool plate on a multi-tool printer (from `custom_gcode`).
    switches: crate::custom_gcode::Switches,
    /// Purging into infill or objects, when some object asks for it.
    wiping: Option<WipingExtrusions>,
}

/// How many bytes of generated toolpaths the tool-usage prepass keeps for export, over all
/// objects. Typical plates fit whole; a 1,916-layer painted plate held 1.2 GB here before
/// this bound.
pub const PREPASS_RETAIN_BYTES: usize = 256 << 20;

/// Generated layer toolpaths with their per-path region ids, by object layer index.
type RetainedLayers = BTreeMap<usize, (LayerToolpaths, Vec<Option<usize>>)>;
impl PreparedPrint {
    pub fn new(objects: Vec<PrintObject>, c: &Config) -> Result<Self, String> {
        Self::with_custom_gcode(objects, c, crate::custom_gcode::Info::default())
    }

    /// Distinct tools the objects print with (upstream `Print::object_extruders`): walls and
    /// infill of every object and region, plus explicit support tools.
    pub fn object_tool_count(objects: &[PrintObject]) -> usize {
        let mut tools = std::collections::BTreeSet::new();
        for o in objects {
            tools.extend(o.tools.0[..3].iter().flatten().copied());
            for r in 0..o.prepared.region_count() {
                if let Some(region) = o.prepared.region(r) {
                    tools.extend(region.tools.0[..3].iter().flatten().copied());
                }
            }
        }
        tools.len()
    }

    /// [`Self::new`] with a plate's per-layer custom G-code: per-layer tool switches and the
    /// tools colour changes need take part in the tool ordering.
    pub fn with_custom_gcode(
        mut objects: Vec<PrintObject>,
        c: &Config,
        mut custom_gcode: crate::custom_gcode::Info,
    ) -> Result<Self, String> {
        let mut warnings = Vec::new();
        // Neither PrusaSlicer nor Orca applies per-layer custom G-code when objects print one
        // after another (GCode.cpp: assign_custom_gcodes runs for layer-by-layer prints only).
        if crate::sequential::enabled(c) && !custom_gcode.is_empty() {
            warnings.push(
                "Color changes, pauses and other layer G-code are not used when objects print \
                 one after another (as in PrusaSlicer and OrcaSlicer)."
                    .into(),
            );
            custom_gcode = crate::custom_gcode::Info::default();
        }
        let tool_count = c.vector_len("nozzle_diameter").max(1);
        validate_tower_supports(&objects, c)?;
        let object_tools = Self::object_tool_count(&objects);
        let switches = custom_gcode.extruder_switches(tool_count, object_tools);
        let picked = custom_gcode.picked_color_changes(
            tool_count,
            c.bool("single_extruder_multi_material").unwrap_or(false),
        );
        // Brick layers v1 prints with one tool only: with several tools, a tool's paths could
        // print next to another tool's raised walls, and the wipe tower travels at layer height.
        let brick_tools: std::collections::BTreeSet<usize> = objects
            .iter()
            .flat_map(|o| {
                let regions = (0..o.prepared.region_count())
                    .filter_map(|r| o.prepared.region(r))
                    .flat_map(|r| r.tools.0.iter().flatten().copied());
                o.tools.0.iter().flatten().copied().chain(regions)
            })
            .collect();
        let painted = objects.iter().any(|o| {
            (0..o.prepared.layer_count()).any(|i| {
                o.prepared
                    .layer_regions(i)
                    .is_some_and(|r| r.iter().any(|r| r.painted_tool.is_some()))
            })
        });
        let mut brick_suppressed = false;
        if brick_tools.len() > 1 || painted || !switches.is_empty() {
            for o in &objects {
                // Every object, not only up to the first one that raises walls.
                brick_suppressed |= o.prepared.suppress_brick_layers();
            }
        }
        if brick_suppressed {
            warnings.push(
                "Brick layers are off for this plate: it prints with more than one tool.".into(),
            );
        }
        // Child plans are superseded by the plate plan. Their paths must not influence tool usage.
        // A shared prepared object already dropped its plan when it was shared.
        for o in &mut objects {
            if let Some(prepared) = Arc::get_mut(&mut o.prepared) {
                prepared.adhesion = crate::brim::AdhesionPlan::default();
            }
        }
        // Instances sharing one prepared object generate their paths once (upstream's
        // PrintObject with several PrintInstances).
        let mut leaders: Vec<usize> = Vec::new();
        let group_of: Vec<usize> = (0..objects.len())
            .map(|oi| {
                leaders
                    .iter()
                    .position(|&l| Arc::ptr_eq(&objects[l].prepared, &objects[oi].prepared))
                    .unwrap_or_else(|| {
                        leaders.push(oi);
                        leaders.len() - 1
                    })
            })
            .collect();
        // Compact layer/tool records drive the schedule; the generated paths themselves are
        // retained for the export batches, which previously regenerated every layer.
        // Indexed collection and the serial fold preserve object, layer and tool order.
        let prepass_timer = crate::timing::Span::new(crate::timing::Stage::ToolUsage);
        // Retention is bounded: each object keeps its lowest layers up to an equal share of
        // PREPASS_RETAIN_BYTES and lets export regenerate the rest, so a tall or dense plate
        // does not hold the paths of every layer at once (deterministic per object, whatever
        // the thread schedule; regenerated layers are identical).
        let share = PREPASS_RETAIN_BYTES / leaders.len().max(1);
        // Purging into infill or objects (upstream WipingExtrusions). Objects printed one after
        // another get their own tool ordering upstream, without the print config, so no wiping.
        let wiping = {
            let orca = crate::rules::Rules::of(c).is_orca();
            (!crate::sequential::enabled(c) && objects.iter().any(|o| o.wipes())).then(|| {
                WipingExtrusions::new(
                    orca,
                    (0..tool_count)
                        .map(|t| c.bool_at("filament_soluble", t).unwrap_or(false))
                        .collect(),
                    (0..tool_count)
                        .map(|t| filament_flag(c, "filament_is_support", t))
                        .collect(),
                )
            })
        };
        let (usage, prepass): (Vec<_>, Vec<_>) = leaders
            .par_iter()
            .map(|&leader| {
                // Instances share the tools of the object they were made from.
                let o = &objects[leader];
                let mut usage = Vec::new();
                let mut retained = BTreeMap::new();
                let mut kept = 0usize;
                let object_wiping = wiping.as_ref().filter(|_| o.wipes());
                for begin in (0..o.prepared.layer_count()).step_by(32) {
                    for (layer, regions) in o
                        .prepared
                        .regional_toolpaths(begin..(begin + 32).min(o.prepared.layer_count()))
                    {
                        let mut ts = Vec::new();
                        let switch = switches.at(layer.print_z);
                        if let Some(w) = object_wiping {
                            ts = o.wiping_layer_tools(w, &layer.paths, &regions, switch);
                        } else {
                            for (pi, p) in layer.paths.iter().enumerate() {
                                let t = o.path_tool_at(
                                    regions.get(pi).copied().flatten(),
                                    p.role,
                                    switch,
                                );
                                if !ts.contains(&t) {
                                    ts.push(t);
                                }
                            }
                        }
                        usage.push((layer.index, layer.print_z, layer.height, ts));
                        // Once over the share, every later layer is dropped: the retained
                        // set stays a prefix and export regenerates one contiguous range.
                        if kept < share {
                            kept += layer.heap_bytes()
                                + regions.capacity() * std::mem::size_of::<Option<usize>>();
                            retained.insert(layer.index, (layer, regions));
                        }
                    }
                }
                (usage, Mutex::new(retained))
            })
            .unzip();
        drop(prepass_timer);
        for o in &objects {
            o.prepared.drop_warm_layers();
        }
        // collect_layers_to_print: an object whose first layer has no extrusions cannot be
        // printed (its second layer and brim would be laid in the air). Both slicers stop
        // the slice. GUI monkey test, seed 3019: a sub-millimeter object printed its first
        // extrusions on layer 2, at a temperature the hotend was never waited for. An object's
        // first layer is its lowest one with a slice or a path (upstream objects rest on the
        // bed; a part or object placed higher starts where its geometry does).
        for group in instance_groups(&objects) {
            let layers = || {
                group.iter().filter_map(|&oi| {
                    usage[group_of[oi]].iter().find(|u| {
                        !u.3.is_empty()
                            || objects[oi]
                                .prepared
                                .layer_outlines(u.0)
                                .is_some_and(|o| !o.is_empty())
                    })
                })
            };
            let Some(first) = layers().map(|u| u.1).min_by(f64::total_cmp) else {
                // Nothing on any layer (an XY compensation larger than the object): upstream's
                // first object layer has no extrusions either. Without this the start G-code
                // failed on its undefined first-layer placeholders (CLI fuzz, 2026-10-21). With
                // Orca rules the empty layers at the top are dropped, so it has none at all.
                return Err(empty_first_layer(&objects[group[0]].name, c));
            };
            if !layers().any(|u| u.1 <= first + 1e-4 && !u.3.is_empty()) {
                return Err(empty_first_layer(&objects[group[0]].name, c));
            }
        }
        // Sequential printing (PrusaSlicer's complete_objects, Orca's "by object"): each
        // instance prints all its layers before the next one starts, in model order
        // (sort_object_instances_by_model_order). Parts of one instance print together.
        let instances = instance_groups(&objects);
        let mut sequential = crate::sequential::enabled(c) && instances.len() > 1;
        let orca = crate::sequential::orca_rules(c);
        let tools_of = |group: &[usize]| {
            let mut used: Vec<usize> = group
                .iter()
                .flat_map(|&oi| &usage[group_of[oi]])
                .flat_map(|(.., ts)| ts.iter().flatten().copied())
                .collect();
            used.sort_unstable();
            used.dedup();
            used
        };
        if sequential {
            let nozzles = c.vector_len("nozzle_diameter").max(1);
            let all: Vec<usize> = instances.iter().flatten().copied().collect();
            let tower = c.bool("wipe_tower").unwrap_or(false) && nozzles > 1;
            if orca {
                // GCode::_do_export prints by object only without a prime tower; the tool
                // ordering it asks is the last object's.
                if tower && instances.last().is_some_and(|g| tools_of(g).len() > 1) {
                    sequential = false;
                    warnings.push(
                        "Objects print layer by layer: OrcaSlicer prints one object after \
                         another only without a prime tower (turn the prime tower off)."
                            .into(),
                    );
                }
            } else if tower && tools_of(&all).len() > 1 {
                return Err(
                    "The Wipe Tower is currently not supported for multimaterial sequential prints."
                        .into(),
                );
            }
        }
        if sequential {
            let meshes: Vec<Vec<&TriangleMesh>> = instances
                .iter()
                .map(|g| g.iter().map(|&oi| &objects[oi].mesh).collect())
                .collect();
            let footprints: Vec<_> = instances
                .iter()
                .zip(&meshes)
                .map(|(g, parts)| crate::sequential::Instance {
                    name: &objects[g[0]].name,
                    parts,
                })
                .collect();
            let report = crate::sequential::check(&footprints, c);
            if !report.errors.is_empty() {
                return Err(report.errors.join("\n"));
            }
            warnings.extend(report.warnings);
        }
        for o in &objects {
            for w in o
                .prepared
                .texture_warnings()
                .into_iter()
                .chain(o.prepared.brick_warnings())
            {
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
        }
        let groups: Vec<Vec<usize>> = if sequential {
            instances
        } else {
            vec![(0..objects.len()).collect()]
        };
        let mut layers: Vec<JobLayer> = Vec::new();
        let mut layer_tools: Vec<Vec<Option<usize>>> = Vec::new();
        for (ordinal, group) in groups.iter().enumerate() {
            let (merged, tools) = merge_layers(&objects, &group_of, &usage, group);
            for (local, mut layer) in merged.into_iter().enumerate() {
                if sequential {
                    layer.sequence = Some(Sequence {
                        ordinal,
                        local,
                        object: group[0],
                    });
                }
                layers.push(layer);
            }
            layer_tools.extend(tools);
        }
        if orca {
            orca_support_only_base_tools(&objects, &layers, &mut layer_tools, c);
        }
        // A sequential print has no tower (upstream refuses or falls back above).
        if !sequential {
            // Preserve the original schedule when there is no multi-tool tower. For a tower,
            // fill raft/empty Z gaps and insert an explicitly selected structural tool.
            if c.bool("wipe_tower").unwrap_or(false) {
                let provisional = ToolOrdering::new(
                    layers
                        .iter()
                        .zip(&layer_tools)
                        .map(|(l, t)| (l.z, t.clone()))
                        .collect(),
                    optimized_tool_ordering(c),
                );
                if provisional.used.len() > 1 {
                    let mut active = provisional.first;
                    let mut top = 0.;
                    for l in &provisional.layers {
                        for &t in &l.extruders {
                            if t != active {
                                top = l.print_z;
                            }
                            active = t;
                        }
                    }
                    let max_height = crate::tool_ordering::max_layer_height(c);
                    let forced = c.int("wipe_tower_extruder").unwrap_or(0).max(0) as usize;
                    if forced > c.vector_len("nozzle_diameter") {
                        return Err("wipe_tower_extruder exceeds printer tool count".into());
                    }
                    let mut z = 0.;
                    let mut expanded = Vec::new();
                    let mut expanded_tools = Vec::new();
                    for (l, mut ts) in layers.into_iter().zip(layer_tools) {
                        if l.z <= top + 1e-4 {
                            let count = ((l.z - z) / max_height - 1e-6).ceil().max(1.) as usize;
                            let step = (l.z - z) / count as f64;
                            for n in 1..count {
                                expanded.push(JobLayer {
                                    z: z + n as f64 * step,
                                    height: step,
                                    sources: vec![],
                                    outlines: vec![],
                                    tops: vec![],
                                    skirt: None,
                                    sequence: None,
                                });
                                expanded_tools.push(if forced > 0 {
                                    vec![Some(forced - 1)]
                                } else {
                                    vec![None]
                                });
                            }
                            if forced > 0 {
                                ts.push(Some(forced - 1));
                            }
                        }
                        z = l.z;
                        expanded.push(l);
                        expanded_tools.push(ts);
                    }
                    layers = expanded;
                    layer_tools = expanded_tools;
                }
            }
        }
        // A colour change on a layer some object prints needs its extruder first there.
        let needed: Vec<Option<usize>> = layers
            .iter()
            .map(|l| {
                l.sources.iter().find_map(|&(oi, li)| {
                    crate::custom_gcode::Info::needed_extruder(
                        &picked,
                        objects[oi].prepared.layer_z(li).0,
                    )
                })
            })
            .collect();
        // PrusaSlicer's reorder_extruders starts a tower print's first layer with a soluble
        // tool; Orca orders by flush volume instead.
        let soluble: Vec<bool> = if crate::rules::Rules::of(c).is_orca() {
            vec![]
        } else {
            (0..c.vector_len("nozzle_diameter").max(1))
                .map(|t| c.bool_at("filament_soluble", t).unwrap_or(false))
                .collect()
        };
        let mut ordering = ToolOrdering::with_options(
            layers
                .iter()
                .zip(layer_tools)
                .map(|(l, mut ts)| {
                    ts.sort_unstable();
                    (l.z, ts)
                })
                .collect(),
            optimized_tool_ordering(c),
            &needed,
            c.bool("wipe_tower").unwrap_or(false),
            &soluble,
        );
        // Support-only layers (independent support layer heights) carry no object extrusions,
        // so the tower skips them as upstream does. Gap filler layers exist for the tower.
        for (lt, l) in ordering.layers.iter_mut().zip(&layers) {
            lt.has_object = l.sources.is_empty()
                || l.sources
                    .iter()
                    .any(|&(oi, li)| objects[oi].prepared.print_layers[li].object.is_some());
        }
        let wipe_tower = if sequential {
            None
        } else {
            crate::wipe_tower::TowerPlan::new(c, &ordering)?
        };
        let adhesion =
            crate::brim::plate::generate_plate(&objects, c, &ordering, wipe_tower.as_ref())?;
        let mut skirt_index = 0;
        let mut skirt_z = 0.;
        for layer in &mut layers {
            // Sequentially, the one skirt around the plate prints with the first object
            // (GCode.cpp's skirt_done is never reset between objects).
            if layer.sequence.is_some_and(|s| s.ordinal > 0) {
                break;
            }
            if layer
                .sources
                .iter()
                .any(|&(oi, li)| crate::brim::plate::skirt_layer(&objects[oi].prepared, li))
            {
                layer.skirt = Some((skirt_index, layer.z - skirt_z));
                skirt_index += 1;
                skirt_z = layer.z;
            }
        }
        // Stable nearest-neighbour instance chain, reused for every tool and layer as upstream.
        let centers: Vec<_> = objects
            .iter()
            .map(|o| {
                let (a, b) = o.mesh.bounds();
                ((a[0] + b[0]) as f64 / 2., (a[1] + b[1]) as f64 / 2.)
            })
            .collect();
        let mut pending: Vec<_> = (0..objects.len()).collect();
        let mut object_order = Vec::new();
        let mut pos = centers.first().copied().unwrap_or_default();
        while !pending.is_empty() {
            let index = (0..pending.len())
                .min_by(|&a, &b| {
                    let dist = |i: usize| {
                        (centers[pending[i]].0 - pos.0).hypot(centers[pending[i]].1 - pos.1)
                    };
                    dist(a).total_cmp(&dist(b)).then(a.cmp(&b))
                })
                .unwrap();
            let oi = pending.remove(index);
            pos = centers[oi];
            object_order.push(oi);
        }
        Ok(Self {
            objects,
            ordering,
            wipe_tower,
            adhesion,
            warnings,
            layers,
            object_order,
            group_of,
            leaders,
            prepass,
            custom_gcode,
            switches,
            wiping,
        })
    }

    /// Whether objects print one after another (each instance completed before the next).
    pub fn is_sequential(&self) -> bool {
        self.layers.first().is_some_and(|l| l.sequence.is_some())
    }

    /// The first layer of an object printed sequentially.
    pub fn object_start(&self, i: usize) -> Option<ObjectStart> {
        self.layers[i]
            .sequence
            .filter(|s| s.local == 0)
            .map(|s| ObjectStart {
                ordinal: s.ordinal,
                object: s.object,
            })
    }

    /// The sequentially printed object that layer `i` belongs to (its position in the print
    /// order and first part group).
    pub fn sequential_object(&self, i: usize) -> Option<ObjectStart> {
        self.layers[i].sequence.map(|s| ObjectStart {
            ordinal: s.ordinal,
            object: s.object,
        })
    }

    /// See [`LayerSource::previous_layer`].
    pub fn previous_layer(&self, i: usize) -> Option<usize> {
        if self.layers.get(i)?.sequence.is_some_and(|s| s.local == 0) {
            None
        } else {
            i.checked_sub(1)
        }
    }

    /// See [`LayerSource::next_layer`].
    pub fn next_layer(&self, i: usize) -> Option<usize> {
        let next = self.layers.get(i + 1)?;
        (!next.sequence.is_some_and(|s| s.local == 0)).then_some(i + 1)
    }

    /// Highest layer Z of the print.
    pub fn max_print_z(&self) -> f64 {
        self.layers.iter().map(|l| l.z).fold(0., f64::max)
    }

    /// XY centre (mm) of the bounding box of all parts of `object`'s instance: where
    /// PrusaSlicer and Orca travel ("move to origin position for next object") before a
    /// sequential object starts.
    pub fn instance_center(&self, object: usize) -> (f64, f64) {
        let key = (
            self.objects[object].object_id,
            self.objects[object].instance_id,
        );
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for o in self
            .objects
            .iter()
            .filter(|o| (o.object_id, o.instance_id) == key)
        {
            let (a, b) = o.mesh.bounds();
            for k in 0..2 {
                min[k] = min[k].min(a[k]);
                max[k] = max[k].max(b[k]);
            }
        }
        (
            f64::from(min[0] + max[0]) / 2.,
            f64::from(min[1] + max[1]) / 2.,
        )
    }

    /// The plan of a source that generates its own layers (the calibration tests): `objects`
    /// supply labels, first-layer footprints and per-tool settings only, and every layer at
    /// `layers` (print Z, height) is printed by `tool` alone, without skirt, brim or tower.
    /// This is the tool ordering of a plate whose only objects are assigned to `tool`, so
    /// the start templates see it as the initial and only used tool. The source returns its
    /// own `scheduled_toolpaths`; [`Self::scheduled`] has no paths for such a plan.
    pub fn single_tool(objects: Vec<PrintObject>, layers: &[(f64, f64)], tool: usize) -> Self {
        let ordering = ToolOrdering::new(
            layers.iter().map(|&(z, _)| (z, vec![Some(tool)])).collect(),
            true,
        );
        Self {
            prepass: objects.iter().map(|_| Mutex::default()).collect(),
            object_order: (0..objects.len()).collect(),
            // Every object is its own group: the source supplies the paths.
            group_of: (0..objects.len()).collect(),
            leaders: (0..objects.len()).collect(),
            layers: layers
                .iter()
                .map(|&(z, height)| JobLayer {
                    z,
                    height,
                    sources: vec![],
                    outlines: vec![],
                    tops: vec![],
                    skirt: None,
                    sequence: None,
                })
                .collect(),
            objects,
            ordering,
            adhesion: crate::brim::AdhesionPlan::default(),
            wipe_tower: None,
            custom_gcode: crate::custom_gcode::Info::default(),
            switches: crate::custom_gcode::Switches::default(),
            wiping: None,
            warnings: Vec::new(),
        }
    }

    pub fn object_first_layer_bbox(&self) -> Option<BoundingBox> {
        crate::brim::plate::object_bbox(&self.objects)
    }

    pub fn scheduled(&self, range: Range<usize>) -> Vec<ScheduledLayer> {
        let batches: Vec<_> = (0..self.leaders.len())
            .into_par_iter()
            .map(|group| {
                let oi = self.leaders[group];
                // Instances of one prepared object sit on the same job layers.
                let mut indices: Vec<_> = self.layers[range.clone()]
                    .iter()
                    .flat_map(|l| &l.sources)
                    .filter(|(o, _)| self.group_of[*o] == group)
                    .map(|(_, l)| *l)
                    .collect();
                // Sequential objects sharing a prepared object reuse its layer indices.
                indices.sort_unstable();
                indices.dedup();
                // Layers the prepass retained are moved out; any other layer is generated as before.
                let mut out = Vec::new();
                let mut missing = Vec::new();
                {
                    let mut retained = self.prepass[group].lock().unwrap();
                    for &l in &indices {
                        match retained.remove(&l) {
                            Some(entry) => out.push(((group, l), entry)),
                            None => missing.push(l),
                        }
                    }
                }
                // Contiguous runs (one run unless sequential copies wrap around in a batch).
                let mut runs: Vec<Range<usize>> = Vec::new();
                for l in missing {
                    match runs.last_mut() {
                        Some(run) if run.end == l => run.end = l + 1,
                        _ => runs.push(l..l + 1),
                    }
                }
                for run in runs {
                    out.extend(
                        self.objects[oi]
                            .prepared
                            .regional_toolpaths(run)
                            .into_iter()
                            .map(|(layer, regions)| ((group, layer.index), (layer, regions))),
                    );
                }
                out
            })
            .collect();
        let cache: BTreeMap<_, _> = batches.into_iter().flatten().collect();
        range
            .map(|i| {
                let l = &self.layers[i];
                let mut out = ScheduledLayer {
                    layer: LayerToolpaths {
                        support_groups: Vec::new(),
                        // Sequential objects each start again at their own first layer.
                        index: l.sequence.map_or(i, |s| s.local),
                        print_z: l.z,
                        height: l.height,
                        paths: vec![],
                    },
                    assignments: vec![],
                };
                let tools = &self.ordering.layers[i].extruders;
                for &tool in tools {
                    if let Some((index, height)) = l.skirt {
                        let mut paths =
                            self.adhesion
                                .paths_for_tool(index, height, tools, tool, usize::MAX);
                        let owner = l.sequence.map_or_else(
                            || {
                                self.object_order
                                    .iter()
                                    .copied()
                                    .find(|&oi| self.objects[oi].tools.0[0] == Some(tool))
                                    .unwrap_or(0)
                            },
                            |s| s.object,
                        );
                        out.assignments.extend(std::iter::repeat_n(
                            PathAssignment {
                                object: owner,
                                tool,
                                region: None,
                            },
                            paths.len(),
                        ));
                        out.layer.paths.append(&mut paths);
                    }
                    // PrusaSlicer prints the whole brim once, with the first layer of the first
                    // object (m_brim_done); Orca keeps each object's brim with that object.
                    let orca_sequence = l.sequence.filter(|_| self.adhesion.orca);
                    if orca_sequence.map_or(i == 0, |s| s.local == 0) {
                        for (bi, path) in self.adhesion.brim.iter().enumerate() {
                            let owner =
                                self.adhesion.brim_objects.get(bi).copied().unwrap_or(0) as usize;
                            if let Some(s) = orca_sequence {
                                let (a, b) = (&self.objects[owner], &self.objects[s.object]);
                                if (a.object_id, a.instance_id) != (b.object_id, b.instance_id) {
                                    continue;
                                }
                            }
                            let brim_tool = if self.adhesion.orca {
                                self.objects[owner]
                                    .tools
                                    .tool(Role::Brim)
                                    .unwrap_or(tools[0])
                            } else {
                                tools[0]
                            };
                            if tool == brim_tool {
                                out.layer.paths.push(path.clone());
                                out.assignments.push(PathAssignment {
                                    object: owner,
                                    tool,
                                    region: None,
                                });
                            }
                        }
                    }
                    // Upstream prints the extrusions forced to this tool by the wiping rules
                    // first (every instance), then the normal ones.
                    let passes: &[bool] = if self.wiping.is_some() {
                        &[true, false]
                    } else {
                        &[false]
                    };
                    for &overridden_pass in passes {
                        for &oi in &self.object_order {
                            for &(o, li) in &l.sources {
                                if o != oi {
                                    continue;
                                }
                                let (layer, regions) = &cache[&(self.group_of[oi], li)];
                                let shift = self.objects[oi].shift;
                                let switch = self.switches.at(layer.print_z);
                                for (pi, path) in layer.paths.iter().enumerate() {
                                    let region = regions.get(pi).copied().flatten();
                                    let object = &self.objects[oi];
                                    let (path_tool, overridden) = self
                                        .wipe_route(i, object, region, path.role, switch)
                                        .unwrap_or_else(|| {
                                            let t = object
                                                .path_tool_at(region, path.role, switch)
                                                .unwrap_or_else(|| {
                                                    let assigned = object
                                                        .path_tool(region, Role::SupportMaterial);
                                                    let interface = object.path_tool(
                                                        region,
                                                        Role::SupportMaterialInterface,
                                                    );
                                                    dont_care_support_tool(
                                                        &object.config,
                                                        tools,
                                                        path.role,
                                                        (assigned, interface),
                                                    )
                                                });
                                            (t, false)
                                        });
                                    if path_tool == tool && overridden == overridden_pass {
                                        let mut path = path.clone();
                                        crate::instances::shift_path(&mut path, shift);
                                        if let Some(group) =
                                            layer.support_groups.iter().find(|g| g.start == pi)
                                        {
                                            let start = out.layer.paths.len();
                                            out.layer
                                                .support_groups
                                                .push(start..start + group.len());
                                        }
                                        out.layer.paths.push(path);
                                        out.assignments.push(PathAssignment {
                                            object: oi,
                                            tool,
                                            region,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                out
            })
            .collect()
    }

    /// The tool printing a path that may absorb a purge, and whether the wiping rules forced it
    /// there (upstream `get_extruder_id`): on a layer the wipe tower plans,
    /// `ensure_perimeters_infills_order` may force it to the layer's first or last non-soluble
    /// tool; otherwise it keeps its tool, or takes the layer's last tool when its own was
    /// dropped from the layer. `None` for every other path.
    fn wipe_route(
        &self,
        layer: usize,
        object: &PrintObject,
        region: Option<usize>,
        role: Role,
        switch: Option<usize>,
    ) -> Option<(usize, bool)> {
        let w = self.wiping.as_ref()?;
        let into = object.wipe_into(region);
        if !into.any() {
            return None;
        }
        let own = object.path_tool_at(region, role, switch)?;
        if !w.is_overridable(role, own, into) {
            return None;
        }
        let tools = object.region_tools(region, switch)?;
        let extruders = &self.ordering.layers.get(layer)?.extruders;
        let tower = self
            .wipe_tower
            .as_ref()
            .is_some_and(|t| t.job_layers.get(layer).copied().flatten().is_some());
        if tower {
            // The island order Mica prints (PrusaSlicer's `infill_first`; Orca's regions read
            // `is_infill_first`, which Mica does not implement: walls first).
            let infill_first = object.path_config(region, own).1.infill_first;
            if let Some(t) = w.ensure_order(extruders, role, tools, into, infill_first) {
                return Some((t, true));
            }
        }
        Some((
            crate::tool_ordering::layer_tool_or_last(extruders, own),
            false,
        ))
    }
}
/// Orca `ToolOrdering::collect_extruders`: on a layer without object extrusions (support
/// layers between object layers, independent support layer heights), a don't-care support
/// base under an explicitly assigned interface prints with the used filament that is cheapest
/// to flush to from the interface filament and not soluble. `support_interface_not_for_body`
/// (Orca's default) excludes the interface filament itself, which otherwise wins with its zero
/// flush. The layer's base then prints with that tool (`dont_care_support_tool`).
fn orca_support_only_base_tools(
    objects: &[PrintObject],
    layers: &[JobLayer],
    layer_tools: &mut [Vec<Option<usize>>],
    c: &Config,
) {
    let mut used: Vec<usize> = layer_tools.iter().flatten().flatten().copied().collect();
    used.sort_unstable();
    used.dedup();
    let count = c.vector_len("nozzle_diameter").max(1);
    let matrix: Vec<f64> = c
        .raw("flush_volumes_matrix")
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    let flush = |from: usize, to: usize| matrix.get(from * count + to).copied().unwrap_or(0.);
    for (layer, tools) in layers.iter().zip(layer_tools.iter_mut()) {
        let has_object = layer
            .sources
            .iter()
            .any(|&(oi, li)| objects[oi].prepared.print_layers[li].object.is_some());
        if has_object || !tools.contains(&None) {
            continue;
        }
        // Only a layer printing both the base and the interface (`has_support && has_interface`).
        let Some((oi, interface)) = layer.sources.iter().find_map(|&(oi, _)| {
            let t = &objects[oi].tools;
            t.tool(Role::SupportMaterial)
                .is_none()
                .then(|| t.tool(Role::SupportMaterialInterface))
                .flatten()
                .filter(|&interface| tools.contains(&Some(interface)))
                .map(|interface| (oi, interface))
        }) else {
            continue;
        };
        let not_for_body = objects[oi]
            .config
            .raw("support_interface_not_for_body")
            .is_none_or(|v| !matches!(v.trim(), "0" | "false"));
        let current = not_for_body.then_some(interface);
        let mut next = None;
        let mut best = f64::MAX;
        for &id in &used {
            if c.bool_at("filament_soluble", id).unwrap_or(false) || Some(id) == current {
                continue;
            }
            let volume = flush(interface, id);
            if volume < best {
                next = Some(id);
                best = volume;
            }
        }
        // Upstream pushes `next + 1`; when nothing qualifies that is the excluded interface.
        let next = next.unwrap_or(interface);
        for t in tools.iter_mut().filter(|t| t.is_none()) {
            *t = Some(next);
        }
    }
}

/// PrusaSlicer's `Print::validate` for a wipe tower with normal (not organic) supports: soluble
/// supports (a zero top contact distance) need support layers synchronized with the object
/// layers, and other supports must print with the current tool (both support tools "Default").
/// OrcaSlicer has no such check (its prime tower synchronizes the support layers itself).
pub fn validate_tower_supports(objects: &[PrintObject], c: &Config) -> Result<(), String> {
    if crate::rules::Rules::of(c).is_orca()
        || !c.bool("wipe_tower").unwrap_or(false)
        || c.bool("spiral_vase").unwrap_or(false)
        || c.vector_len("nozzle_diameter") < 2
    {
        return Ok(());
    }
    for o in objects {
        let oc = &o.config;
        let has_support = oc.bool("support_material").unwrap_or(false)
            || oc.int("support_material_enforce_layers").unwrap_or(0) > 0
            || oc.int("raft_layers").unwrap_or(0) > 0;
        if !has_support || oc.string("support_material_style") == Some("organic") {
            continue;
        }
        if oc.float("support_material_contact_distance").unwrap_or(0.2) == 0. {
            if !oc
                .bool("support_material_synchronize_layers")
                .unwrap_or(false)
            {
                return Err(format!(
                    "{}: for the wipe tower to work with soluble supports, the support layers \
                     need to be synchronized with the object layers (turn on \
                     support_material_synchronize_layers).",
                    o.name
                ));
            }
        } else if oc.int("support_material_extruder").unwrap_or(0) != 0
            || oc.int("support_material_interface_extruder").unwrap_or(0) != 0
        {
            return Err(format!(
                "{}: the wipe tower supports non-soluble supports only if they print with the \
                 current tool without a tool change: set both support tools \
                 (support_material_extruder and support_material_interface_extruder) to \
                 Default (0), or use a top contact distance of 0 for soluble supports.",
                o.name
            ));
        }
    }
    Ok(())
}

/// collect_layers_to_print's error for an object whose first layer prints nothing, in the
/// words of the slicer whose rules apply.
pub fn empty_first_layer(name: &str, c: &Config) -> String {
    if crate::rules::Rules::of(c).is_orca() {
        format!(
            "{name}: One object has an empty first layer and can't be printed. Please Cut the \
             bottom or enable supports."
        )
    } else {
        format!("There is an object with no extrusions in the first layer.\nObject name: {name}")
    }
}

/// Per-object layer usage from the tool prepass: (object layer index, print_z, height, tools).
type Usage = Vec<(usize, f64, f64, Vec<Option<usize>>)>;

/// Instances (parts with the same object and instance ids) in model order: by object id,
/// then instance id, parts in their original order.
fn instance_groups(objects: &[PrintObject]) -> Vec<Vec<usize>> {
    let mut keys: Vec<(usize, usize)> = objects
        .iter()
        .map(|o| (o.object_id, o.instance_id))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys.iter()
        .map(|k| {
            (0..objects.len())
                .filter(|&oi| (objects[oi].object_id, objects[oi].instance_id) == *k)
                .collect()
        })
        .collect()
}

/// The job layers of `members` (object indices), merged by print Z, with their tools.
fn merge_layers(
    objects: &[PrintObject],
    group_of: &[usize],
    usage: &[Usage],
    members: &[usize],
) -> (Vec<JobLayer>, Vec<Vec<Option<usize>>>) {
    let mut by_z: BTreeMap<i64, JobLayer> = BTreeMap::new();
    let mut tools: BTreeMap<i64, Vec<Option<usize>>> = BTreeMap::new();
    for &oi in members {
        let group = group_of[oi];
        for (index, z, height, ts) in usage[group].iter().cloned() {
            let key = (z * 1e6).round() as i64;
            let entry = by_z.entry(key).or_insert_with(|| JobLayer {
                z,
                height,
                sources: vec![],
                outlines: vec![],
                tops: vec![],
                skirt: None,
                sequence: None,
            });
            entry.sources.push((oi, index));
            entry.height = entry.height.min(height);
            if let Some(outlines) = objects[oi].layer_outlines(index) {
                entry.outlines.extend_from_slice(&outlines);
            }
            if let Some(tops) = objects[oi].prepared.layer_top_surfaces(index) {
                if objects[oi].shift == Point::default() {
                    entry.tops.extend_from_slice(tops);
                } else {
                    entry
                        .tops
                        .extend(crate::instances::shifted_polygons(tops, objects[oi].shift));
                }
            }
            let tools = tools.entry(key).or_default();
            for t in ts {
                if !tools.contains(&t) {
                    tools.push(t);
                }
            }
        }
    }
    // Upstream groups against the first Z in a group (not a transitive epsilon chain),
    // then prints at the midpoint of the group's extreme Z values.
    let mut layers: Vec<JobLayer> = Vec::new();
    let mut layer_tools: Vec<Vec<Option<usize>>> = Vec::new();
    let mut first_z = f64::NEG_INFINITY;
    for (key, mut layer) in by_z {
        let ts = tools.remove(&key).unwrap_or_default();
        if layer.z <= first_z + 1e-4
            && let Some(last) = layers.last_mut()
        {
            last.z = 0.5 * (first_z + layer.z);
            last.height = last.height.min(layer.height);
            last.sources.append(&mut layer.sources);
            last.outlines.append(&mut layer.outlines);
            last.tops.append(&mut layer.tops);
            layer_tools.last_mut().unwrap().extend(ts);
        } else {
            first_z = layer.z;
            layers.push(layer);
            layer_tools.push(ts);
        }
    }
    (layers, layer_tools)
}

impl LayerSource for PreparedPrint {
    fn print_job(&self) -> Option<&PreparedPrint> {
        Some(self)
    }
    fn custom_gcode(&self) -> Option<&crate::custom_gcode::Info> {
        (!self.custom_gcode.is_empty()).then_some(&self.custom_gcode)
    }
    fn scheduled_toolpaths(&self, range: Range<usize>) -> Vec<ScheduledLayer> {
        self.scheduled(range)
    }
    fn layer_count(&self) -> usize {
        self.layers.len()
    }
    fn layer_z(&self, i: usize) -> (f64, f64) {
        (self.layers[i].z, self.layers[i].height)
    }
    fn layer_outlines(&self, i: usize) -> Option<&[ExPolygon]> {
        Some(&self.layers[i].outlines)
    }
    fn layer_top_surfaces(&self, i: usize) -> Option<&[ExPolygon]> {
        Some(&self.layers[i].tops)
    }
    fn support_islands(&self, i: usize) -> Option<Vec<ExPolygon>> {
        let mut islands = Vec::new();
        for &(oi, index) in &self.layers.get(i)?.sources {
            let object = &self.objects[oi];
            if let Some(areas) = object.prepared.support_islands(index) {
                if object.shift == Point::default() {
                    islands.extend(areas);
                } else {
                    islands.extend(crate::instances::shifted_polygons(&areas, object.shift));
                }
            }
        }
        (!islands.is_empty()).then(|| crate::geometry::clip::union_ex(&islands))
    }
    fn region_slices(&self, i: usize) -> Option<Vec<Vec<ExPolygon>>> {
        let sources = &self.layers.get(i)?.sources;
        let split: Vec<_> = sources
            .iter()
            .map(|&(oi, index)| self.objects[oi].prepared.region_slices(index))
            .collect();
        if split.iter().all(Option::is_none) {
            return None;
        }
        let mut out = Vec::new();
        for (&(oi, index), regions) in sources.iter().zip(split) {
            let object = &self.objects[oi];
            let regions = regions.or_else(|| {
                object
                    .prepared
                    .layer_outlines(index)
                    .map(|outlines| vec![outlines.to_vec()])
            });
            for slices in regions.unwrap_or_default() {
                out.push(if object.shift == Point::default() {
                    slices
                } else {
                    crate::instances::shifted_polygons(&slices, object.shift)
                });
            }
        }
        Some(out)
    }
    fn object_origin(&self, object: usize) -> Option<Point> {
        self.objects.get(object).map(|o| {
            Point::new(
                o.prepared.origin.x + o.shift.x,
                o.prepared.origin.y + o.shift.y,
            )
        })
    }
    fn first_layer_hull(&self) -> Option<&[Point]> {
        (!self.adhesion.first_layer_hull.is_empty()).then_some(&self.adhesion.first_layer_hull)
    }
    fn first_layer_bbox(&self) -> Option<BoundingBox> {
        self.adhesion.first_layer_bbox()
    }
    fn toolpaths(&self, range: Range<usize>) -> Vec<LayerToolpaths> {
        self.scheduled(range).into_iter().map(|l| l.layer).collect()
    }
}

#[cfg(test)]
mod support_tool_tests {
    use super::*;

    #[test]
    fn prusa_dont_care_support_prefers_non_soluble_orca_keeps_first() {
        let base = Role::SupportMaterial;
        let interface = Role::SupportMaterialInterface;
        let mut config = Config::defaults();
        config.set_raw("filament_soluble", "1,0,0").unwrap();
        assert_eq!(
            dont_care_support_tool(&config, &[0, 1], base, (None, None)),
            1
        );
        assert_eq!(dont_care_support_tool(&config, &[0], base, (None, None)), 0);
        // PrusaSlicer ignores the interface tool and support filaments.
        config.set_raw("filament_is_support", "0,1,0").unwrap();
        assert_eq!(
            dont_care_support_tool(&config, &[0, 1, 2], base, (None, Some(1))),
            1
        );
        config.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
        // Orca: a soluble first tool gives way to a non-soluble one, a support filament to the
        // layer's first body filament (upstream checks that one for solubility no more).
        assert_eq!(
            dont_care_support_tool(&config, &[0, 1, 2], base, (None, None)),
            0
        );
        assert_eq!(
            dont_care_support_tool(&config, &[1, 2], base, (None, None)),
            2
        );
        assert_eq!(
            dont_care_support_tool(&config, &[1, 2], interface, (Some(1), None)),
            2
        );
        // A don't-care base under an explicit interface avoids the interface tool.
        config.set_raw("filament_soluble", "0,0,0").unwrap();
        config.set_raw("filament_is_support", "0,0,0").unwrap();
        assert_eq!(
            dont_care_support_tool(&config, &[1, 0], base, (None, Some(1))),
            0
        );
        assert_eq!(
            dont_care_support_tool(&config, &[1], base, (None, Some(1))),
            1
        );
        assert_eq!(
            dont_care_support_tool(&config, &[1, 0], base, (None, None)),
            1
        );
    }
}
