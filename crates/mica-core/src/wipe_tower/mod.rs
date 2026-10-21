// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.cpp :: extract_wipe_volumes, plan_toolchange, plan_tower, save_on_last_wipe, generate
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::_make_wipe_tower
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower2.cpp :: plan_toolchange, save_on_last_wipe, generate
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::_make_wipe_tower
// Changes: Independent physical tools only; compact volume/depth plan and lazy per-layer moves instead of whole-print G-code. No MMU or wipe-into-object overrides.
// Changes: Plans only ToolOrdering::tower_layers (upstream has_wipe_tower) with their wipe_tower_layer_height; job layers map to plan layers.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/WipeTower.hpp :: WipeTower::ToolChangeResult
// Changes: TowerVisit::wipe_path is ToolChangeResult::wipe_path in bed coordinates (Prusa WipeTower and Orca WipeTower2 alike).
// Source: orca@8500fcdcca src/libslic3r/GCode/WipeTower.hpp :: WipeTower::ToolChangeResult
// Changes: TowerBlock carries the joined-block index and tool_change_start_pos.
// Source: prusaslicer@b028299c77 src/libslic3r/BuildVolume.cpp :: BuildVolume::all_paths_inside
// Changes: The tower is checked on round and polygonal beds and, under Orca rules, against the bed exclusion areas.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

// Copyright (c) Prusa Research 2017–2023 Lukáš Matěna, Vojtěch Bubník, Enrico Turri.
// Copyright (c) SuperSlicer 2023 Remi Durand; 2020 Paul Arden; 2019 Thomas Moore.
// Upstream is AGPLv3 or higher; this port is AGPL-3.0-only.

//! Prime/wipe towers for independent tool changers. Coordinates and volumes are in mm and mm³.
//! Planning retains only tool changes and depths; [`TowerPlan::visits`] materializes one layer.

pub mod estimate;
mod footprint;
mod paths;
#[cfg(test)]
mod tests;
pub use estimate::TowerShape;

use crate::config::Config;
use crate::geometry::{BoundingBox, Point};
use crate::tool_ordering::ToolOrdering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TowerKind {
    Prusa,
    Orca,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wall {
    Rectangle,
    Cone,
    Rib,
}
#[derive(Clone, Debug)]
pub struct TowerSettings {
    pub kind: TowerKind,
    pub position: [f64; 2],
    pub rotation: f64,
    pub width: f64,
    pub line_width: f64,
    pub brim: f64,
    pub cone_angle: f64,
    pub extra_flow: f64,
    pub extra_spacing: f64,
    pub bridging: f64,
    pub no_sparse: bool,
    pub first_speed: f64,
    pub infill_speed: f64,
    pub perimeter_speed: f64,
    pub travel_speed: f64,
    pub wall: Wall,
    pub rib_width: f64,
    pub rib_extra: f64,
    pub fillet: bool,
    pub prime_volume: f64,
    pub enable_ramming: bool,
    pub tools: Vec<TowerTool>,
}
#[derive(Clone, Debug)]
pub struct TowerTool {
    pub min_purge: f64,
    pub soluble: bool,
    pub ram: bool,
    pub ram_volume: f64,
    pub ram_flow: f64,
}
/// Read typed Orca extras or a CLI override of a model-owned Prusa tower coordinate.
/// A tower setting: an explicit scalar text value (a project's or user's edit of an option the
/// schema does not type, such as Orca's `wipe_tower_x`) first, then the typed value, then the
/// first element of a retained vector text.
pub fn number(c: &Config, key: &str, default: f64) -> f64 {
    c.extra(key)
        .filter(|s| !s.contains(','))
        .and_then(|s| s.trim().trim_end_matches('%').parse().ok())
        .or_else(|| c.float(key))
        .or_else(|| {
            c.raw(key)?
                .split(',')
                .next()?
                .trim()
                .trim_end_matches('%')
                .parse()
                .ok()
        })
        .unwrap_or(default)
}
impl TowerSettings {
    pub fn from_config(c: &Config) -> Result<Self, String> {
        let orca = crate::rules::Rules::of(c).is_orca();
        if orca && c.string("wipe_tower_type").is_some_and(|v| v != "type2") {
            return Err("only Orca WipeTower2 is supported for independent tools".into());
        }
        let n = c.vector_len("nozzle_diameter").max(1);
        let nozzle = c.float("nozzle_diameter").unwrap_or(0.4);
        if (0..n)
            .any(|i| (c.float_at("nozzle_diameter", i).unwrap_or(nozzle) - nozzle).abs() > 1e-6)
        {
            return Err("wipe tower requires equal nozzle diameters".into());
        }
        let forced = c.int("wipe_tower_extruder").unwrap_or(0);
        let speed = |key, default| {
            let v = number(c, key, default);
            if v > 0. { v } else { default }
        };
        let max_speed = if orca {
            speed("wipe_tower_max_purge_speed", 90.)
        } else {
            f64::INFINITY
        };
        let first = c.abs_value("first_layer_speed", 60.).unwrap_or(30.);
        let out = Self {
            kind: if orca {
                TowerKind::Orca
            } else {
                TowerKind::Prusa
            },
            position: [
                number(c, "wipe_tower_x", if orca { 15. } else { 180. }),
                number(c, "wipe_tower_y", if orca { 220. } else { 140. }),
            ],
            rotation: number(c, "wipe_tower_rotation_angle", 0.),
            width: number(c, "wipe_tower_width", 60.),
            line_width: 1.25 * nozzle,
            brim: number(c, "wipe_tower_brim_width", 2.),
            cone_angle: number(c, "wipe_tower_cone_angle", 0.),
            extra_flow: number(c, "wipe_tower_extra_flow", 100.) / 100.,
            extra_spacing: number(c, "wipe_tower_extra_spacing", 100.) / 100.,
            bridging: number(c, "wipe_tower_bridging", 10.),
            no_sparse: c.bool("wipe_tower_no_sparse_layers").unwrap_or(false),
            first_speed: if first > 0. { first } else { 30. },
            infill_speed: speed("infill_speed", 80.).min(max_speed),
            perimeter_speed: speed("perimeter_speed", 80.).min(max_speed),
            travel_speed: speed("travel_speed", 120.),
            wall: if !orca {
                Wall::Cone
            } else {
                match c.string("wipe_tower_wall_type").unwrap_or("rib") {
                    "cone" => Wall::Cone,
                    "rectangle" => Wall::Rectangle,
                    _ => Wall::Rib,
                }
            },
            rib_width: number(c, "wipe_tower_rib_width", 8.),
            rib_extra: number(c, "wipe_tower_extra_rib_length", 0.),
            fillet: c.bool("wipe_tower_fillet_wall").unwrap_or(true),
            prime_volume: number(c, "prime_volume", 30.),
            enable_ramming: !orca || c.bool("enable_filament_ramming").unwrap_or(false),
            tools: (0..n)
                .map(|i| TowerTool {
                    min_purge: c
                        .float_at("filament_minimal_purge_on_wipe_tower", i)
                        .unwrap_or(15.),
                    soluble: if forced > 0 {
                        i + 1 != forced as usize
                    } else {
                        c.bool_at("filament_soluble", i).unwrap_or(false)
                    },
                    ram: c.bool_at("filament_multitool_ramming", i).unwrap_or(false),
                    ram_volume: c
                        .float_at("filament_multitool_ramming_volume", i)
                        .unwrap_or(10.),
                    ram_flow: c
                        .float_at("filament_multitool_ramming_flow", i)
                        .unwrap_or(10.),
                })
                .collect(),
        };
        for (key, value) in [
            ("width", out.width),
            ("line width", out.line_width),
            ("bridging", out.bridging),
            ("extra flow", out.extra_flow),
            ("extra spacing", out.extra_spacing),
        ] {
            if !value.is_finite() || value <= 0. {
                return Err(format!("wipe tower {key} must be finite and positive"));
            }
        }
        if out.width <= 6. * out.line_width
            || !out.cone_angle.is_finite()
            || !(0. ..90.).contains(&out.cone_angle)
            || !out.rotation.is_finite()
            || out.position.iter().any(|v| !v.is_finite())
            || !out.brim.is_finite()
            || out.brim < 0.
            || !out.prime_volume.is_finite()
            || out.prime_volume < 0.
            || !out.rib_extra.is_finite()
            || out.rib_extra < 0.
            || !out.rib_width.is_finite()
            || out.rib_width < 0.
        {
            return Err("invalid wipe tower dimensions, angle or purge volume".into());
        }
        for t in &out.tools {
            if [t.min_purge, t.ram_volume, t.ram_flow]
                .iter()
                .any(|v| !v.is_finite() || *v < 0.)
                || (t.ram && t.ram_flow <= 0.)
            {
                return Err("invalid wipe tower filament purge/ramming parameters".into());
            }
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct TowerChange {
    pub old_tool: usize,
    pub new_tool: usize,
    pub required_depth: f64,
    pub ramming_depth: f64,
    pub first_wipe_line: f64,
    pub wipe_volume: f64,
    pub total_volume: f64,
}
#[derive(Clone, Debug)]
pub struct TowerLayer {
    pub print_z: f64,
    pub height: f64,
    pub tower_z: f64,
    pub depth: f64,
    pub changes: Vec<TowerChange>,
    pub start_tool: usize,
    pub skipped: bool,
}
impl TowerLayer {
    pub fn changes_depth(&self) -> f64 {
        self.changes.iter().map(|c| c.required_depth).sum()
    }
}
#[derive(Clone, Debug)]
pub struct TowerPlan {
    pub settings: TowerSettings,
    pub layers: Vec<TowerLayer>,
    pub depth: f64,
    pub height: f64,
    /// Printing tools in priming order; empty when the printer's startup template primes them.
    pub priming_tools: Vec<usize>,
    pub bed_bounds: [[f64; 2]; 2],
    /// Plan layer of each job layer; `None` for layers without a tower (support-only layers
    /// that keep the current tool, and layers above the last tool change).
    pub job_layers: Vec<Option<usize>>,
}
/// A move's extrusion is a volume, independent of the active filament diameter and multiplier.
#[derive(Clone, Debug)]
pub struct TowerMove {
    pub xy: [f64; 2],
    pub volume: f64,
    pub speed: f64,
    pub width: f64,
    pub ramming: bool,
}
#[derive(Clone, Debug, Default)]
pub struct TowerBlock {
    pub start: [f64; 2],
    pub moves: Vec<TowerMove>,
    /// Index into `moves` where a joined block (`merge_tcr`'s second result) begins.
    pub joined: Option<usize>,
    /// Orca `tool_change_start_pos`: the toolchange's initial tower position, which the
    /// post-processed G-code travels to after the custom toolchange G-code.
    pub entry: Option<[f64; 2]>,
}
#[derive(Clone, Debug)]
pub struct TowerVisit {
    pub old_tool: usize,
    pub tool: usize,
    pub z: f64,
    pub height: f64,
    pub before: Option<TowerBlock>,
    pub after: TowerBlock,
    pub change: bool,
    /// `ToolChangeResult::wipe_path` in bed coordinates: where the next retraction wipes
    /// after leaving the tower (empty for none).
    pub wipe_path: Vec<[f64; 2]>,
}
impl TowerPlan {
    /// Plans the tower of a job and checks that it (brim, cone and ribs included) stays on
    /// the bed; `None` when the job prints no tower.
    pub fn new(c: &Config, ordering: &ToolOrdering) -> Result<Option<Self>, String> {
        let Some(out) = Self::plan(c, ordering)? else {
            return Ok(None);
        };
        if let Some(bb) = out.first_layer_bbox() {
            let (min, max) = (
                [
                    crate::geometry::unscale(bb.min.x),
                    crate::geometry::unscale(bb.min.y),
                ],
                [
                    crate::geometry::unscale(bb.max.x),
                    crate::geometry::unscale(bb.max.y),
                ],
            );
            if (0..2).any(|i| {
                min[i] < out.bed_bounds[0][i] - 1e-4 || max[i] > out.bed_bounds[1][i] + 1e-4
            }) {
                return Err(format!(
                    "wipe tower (including brim/cone/ribs) at X {:.1}..{:.1}, Y {:.1}..{:.1} is outside the print bed; set wipe_tower_x/y or rotation",
                    min[0], max[0], min[1], max[1]
                ));
            }
            // A round or polygonal bed (its corners on the outline) and, under Orca rules, the
            // bed's exclusion areas: the tower's paths are toolpaths like any other
            // (`BuildVolume::all_paths_inside`).
            if let Some(volume) = crate::build_volume::BuildVolume::from_config(c) {
                let corners = [min, [max[0], min[1]], max, [min[0], max[1]]];
                let off = volume.shape() != crate::build_volume::Shape::Rectangle
                    && corners
                        .iter()
                        .any(|p| !volume.contains_xy(p[0], p[1], crate::build_volume::BED_EPSILON));
                if off {
                    return Err(format!(
                        "wipe tower (including brim/cone/ribs) at X {:.1}..{:.1}, Y {:.1}..{:.1} is outside the print bed; set wipe_tower_x/y or rotation",
                        min[0], max[0], min[1], max[1]
                    ));
                }
                if let Some(zone) = volume
                    .zones
                    .iter()
                    .find(|z| z.overlaps_convex(&corners, crate::build_volume::BED_EPSILON))
                {
                    return Err(format!(
                        "wipe tower (including brim/cone/ribs) at X {:.1}..{:.1}, Y {:.1}..{:.1} reaches into the excluded area {} of the bed; set wipe_tower_x/y",
                        min[0],
                        max[0],
                        min[1],
                        max[1],
                        zone.describe()
                    ));
                }
            }
        }
        Ok(Some(out))
    }
    /// The plan without the bed check: what [`estimate::estimate`] uses to size a tower that
    /// may still be moved onto the bed.
    pub fn plan(c: &Config, ordering: &ToolOrdering) -> Result<Option<Self>, String> {
        if !c.bool("wipe_tower").unwrap_or(false) || ordering.used.len() < 2 {
            return Ok(None);
        }
        if c.bool("single_extruder_multi_material").unwrap_or(false) {
            return Err("wipe tower supports independent tools, not MMU".into());
        }
        let s = TowerSettings::from_config(c)?;
        // A tower or brim wider than the whole bed can never be placed on it. Refused before
        // any geometry: a 1 km brim was built in full first (GUI monkey test, seed 605: 5 GB
        // on the UI thread for the tower estimate; `mica slice`: 55 s and 6 GB, then refused).
        let span = c.points("bed_shape").unwrap_or_default().iter().fold(
            [
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ],
            |b, &(x, y)| [b[0].min(x), b[1].max(x), b[2].min(y), b[3].max(y)],
        );
        let largest = (span[1] - span[0]).max(span[3] - span[2]);
        if largest.is_finite() && (s.brim > largest || s.width > largest) {
            return Err(format!(
                "wipe tower {} ({:.0} mm) is wider than the print bed ({largest:.0} mm)",
                if s.brim > largest { "brim" } else { "width" },
                s.brim.max(s.width)
            ));
        }
        let mut last = ordering.first;
        let mut last_change = None;
        for (i, l) in ordering.layers.iter().enumerate() {
            for &t in &l.extruders {
                if t != last {
                    last_change = Some(i);
                }
                last = t;
            }
        }
        let Some(last_change) = last_change else {
            return Ok(None);
        };
        let has_tower = ordering.tower_layers(crate::tool_ordering::max_layer_height(c));
        let mut layers = Vec::new();
        let mut job_layers = vec![None; ordering.layers.len()];
        let mut z = 0.;
        let mut tower_z = 0.;
        last = ordering.first;
        for (j, l) in ordering.layers[..=last_change].iter().enumerate() {
            // A tool change always prints the tower, even where upstream's rule would not.
            if !has_tower[j] && l.extruders.iter().all(|&t| t == last) {
                continue;
            }
            let i = layers.len();
            job_layers[j] = Some(i);
            let h = l.print_z - z;
            if h <= 0. || !h.is_finite() || h >= s.line_width * 2. {
                return Err("invalid wipe tower layer height".into());
            }
            let mut layer = TowerLayer {
                print_z: l.print_z,
                height: h,
                tower_z: 0.,
                depth: 0.,
                changes: vec![],
                start_tool: last,
                skipped: false,
            };
            for &t in &l.extruders {
                if t != last {
                    let volume = if s.kind == TowerKind::Prusa {
                        s.tools[t].min_purge
                    } else {
                        s.prime_volume
                    };
                    let width = s.width - 3. * s.line_width;
                    let length = 0.25 * s.tools[last].ram_flow / area(2. * s.line_width, h);
                    let ram_depth = if s.enable_ramming {
                        ((length / width).floor() + 1.) * 2. * s.line_width * s.extra_spacing
                    } else {
                        0.
                    };
                    let first_line = width * (1. - (length / width).fract());
                    let spacing = if i == 0 && s.kind == TowerKind::Orca {
                        s.extra_flow
                    } else {
                        s.extra_flow * s.extra_spacing
                    };
                    let wipe_depth = wipe_depth(
                        volume - first_line * area(s.line_width * s.extra_flow, h),
                        h,
                        &s,
                        spacing,
                    );
                    layer.changes.push(TowerChange {
                        old_tool: last,
                        new_tool: t,
                        required_depth: ram_depth + wipe_depth,
                        ramming_depth: ram_depth,
                        first_wipe_line: first_line,
                        wipe_volume: volume,
                        total_volume: volume,
                    });
                }
                last = t;
            }
            layer.skipped = s.no_sparse && i > 0 && layer.changes.is_empty();
            if !layer.skipped {
                tower_z += h;
            }
            layer.tower_z = if s.no_sparse { tower_z } else { l.print_z };
            layers.push(layer);
            z = l.print_z;
        }
        let mut out = Self {
            settings: s,
            layers,
            depth: 0.,
            height: if c.bool("wipe_tower_no_sparse_layers").unwrap_or(false) {
                tower_z
            } else {
                z
            },
            priming_tools: if c
                .bool("single_extruder_multi_material_priming")
                .unwrap_or(false)
            {
                ordering
                    .used
                    .iter()
                    .copied()
                    .filter(|&t| t != ordering.first)
                    .chain(std::iter::once(ordering.first))
                    .collect()
            } else {
                vec![]
            },
            job_layers,
            bed_bounds: c.points("bed_shape").unwrap_or_default().iter().fold(
                [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
                |mut b, &(x, y)| {
                    b[0][0] = b[0][0].min(x);
                    b[0][1] = b[0][1].min(y);
                    b[1][0] = b[1][0].max(x);
                    b[1][1] = b[1][1].max(y);
                    b
                },
            ),
        };
        out.plan_depths();
        for _ in 0..5 {
            for i in 0..out.layers.len() {
                let Some(idx) = out.layers[i]
                    .changes
                    .iter()
                    .position(|t| !out.settings.tools[t.new_tool].soluble)
                else {
                    continue;
                };
                let volume_saved = out.finish_volume(i);
                let s = &out.settings;
                let l = &mut out.layers[i];
                let t = &mut l.changes[idx];
                t.wipe_volume = s.tools[t.new_tool]
                    .min_purge
                    .max(t.total_volume - volume_saved);
                let volume = (t.wipe_volume
                    - t.first_wipe_line * area(s.line_width * s.extra_flow, l.height))
                .max(0.);
                let spacing = if i == 0 && s.kind == TowerKind::Orca {
                    s.extra_flow
                } else {
                    s.extra_flow * s.extra_spacing
                };
                t.required_depth = t.ramming_depth + wipe_depth(volume, l.height, s, spacing);
            }
            out.plan_depths();
        }
        Ok(Some(out))
    }
    fn plan_depths(&mut self) {
        for l in &mut self.layers {
            l.depth = 0.;
        }
        self.depth = 0.;
        for i in (0..self.layers.len()).rev() {
            let d = self.layers[i].depth.max(self.layers[i].changes_depth());
            self.layers[i].depth = d;
            self.depth = self.depth.max(d + self.settings.line_width);
            for l in &mut self.layers[..i] {
                if l.depth - d < 2. * self.settings.line_width {
                    l.depth = d;
                }
            }
        }
    }
    pub fn first_layer_bbox(&self) -> Option<BoundingBox> {
        let mut pts = Vec::new();
        for v in paths::visits(self, 0) {
            for b in v.before.iter().chain(std::iter::once(&v.after)) {
                for m in &b.moves {
                    if m.volume > 0. {
                        pts.push(Point::from_mm(m.xy[0], m.xy[1]));
                    }
                }
            }
        }
        BoundingBox::from_points(&pts)
    }
    /// The tower visits of job layer `layer` (none when that layer has no tower).
    pub fn visits(&self, layer: usize) -> Vec<TowerVisit> {
        match self.job_layers.get(layer) {
            Some(&Some(i)) => paths::visits(self, i),
            _ => vec![],
        }
    }
    pub fn priming_visits(&self) -> Vec<TowerVisit> {
        paths::prime(self)
    }
    pub fn final_ram(&self, tool: usize, z: f64, height: f64) -> Option<TowerVisit> {
        paths::final_ram(self, tool, z, height)
    }
    fn finish_volume(&self, layer: usize) -> f64 {
        paths::finish_volume(self, layer)
    }
}
fn area(width: f64, height: f64) -> f64 {
    height * (width - height * (1. - std::f64::consts::FRAC_PI_4))
}
fn wipe_depth(volume: f64, h: f64, s: &TowerSettings, spacing: f64) -> f64 {
    let length = (volume / area(s.line_width, h)).max(0.) / s.extra_flow;
    ((length / (s.width - 3. * s.line_width)).floor() + 1.) * s.line_width * spacing
}
