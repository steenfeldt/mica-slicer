// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp :: DynamicPrintConfig::normalize_fdm, validate
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: DynamicPrintConfig::normalize_fdm, validate
// Changes: The spiral vase parts only, on Mica's canonical (PrusaSlicer) option names; Orca's wall_loops, top_shell_layers, sparse_infill_density and retract_when_changing_layer are perimeters, top_solid_layers, fill_density and retract_layer_change here.
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::validate
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::validate
// Source: orca@8500fcdcca src/libslic3r/Layer.cpp :: Layer::is_perimeter_compatible
// Changes: Mica's own wording; the region test compares the resolved wall settings Mica has (tools, wall count, widths, speeds, thin walls) instead of Orca's full option list.
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: slice_volumes
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleMeshSlicer.cpp :: slice_mesh_ex
// Source: prusaslicer@b028299c77 src/libslic3r/ExPolygon.cpp :: keep_largest_contour_only
// Changes: PositiveLargestContour applied to the islands Mica already sliced: every ring counts as a contour (holes close), the union keeps the largest island.
// Source: prusaslicer@b028299c77 src/slic3r/GUI/ConfigManipulation.cpp :: ConfigManipulation::update_print_fff_config
// Source: orca@8500fcdcca src/slic3r/GUI/ConfigManipulation.cpp :: ConfigManipulation::update_print_fff_config
// Changes: The settings both dialogs set when spiral vase is turned on, as edits the app applies in the same step (no dialog); Orca's timelapse and clumping detection have no Mica setting.
// Source: prusaslicer@b028299c77 src/libslic3r/LayerRegion.cpp :: LayerRegion::make_perimeters
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::process_layer
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer
// Changes: The per-layer conditions as functions of the resolved settings; the G-code exporter adds the checks only it can make (one object, no supports or fills on the layer).

//! Spiral vase ("vase mode"): one wall printed as a continuous spiral, solid bottom layers, no
//! top and no infill.
//!
//! The configuration side lives here: the values spiral vase forces ([`normalize`]), the prints
//! it refuses ([`validate_config`], [`validate_objects`]), which layers are sliced to their
//! largest outline ([`slice_from_layer`], [`largest_contour`]), which walls are one plain loop
//! ([`wall_layer`]) and which layers the G-code exporter turns into a spiral ([`spiral_layer`]).
//! The Z ramp itself is `mica-gcode`'s `spiral_vase` post-processor.

use crate::config::Config;
use crate::geometry::clip::union_ex;
use crate::geometry::{ExPolygon, signed_area};
use crate::settings::PrintSettings;

/// PrusaSlicer's `EPSILON`, mm.
const EPSILON: f64 = 1e-4;

/// Whether the configuration prints in spiral vase mode (PrusaSlicer `spiral_vase`, Orca
/// `spiral_mode`).
pub fn enabled(c: &Config) -> bool {
    c.bool("spiral_vase").unwrap_or(false)
}

/// The settings spiral vase forces, as both slicers' `normalize_fdm` sets them before slicing:
/// one wall, no top layers, no infill and no retraction on layer change (also not from a
/// filament override). Returns whether anything was forced (spiral vase is on).
pub fn normalize(c: &mut Config) -> bool {
    if !enabled(c) {
        return false;
    }
    let all_off = |c: &Config, key: &str| vec!["0"; c.vector_len(key).max(1)].join(",");
    let retract = all_off(c, "retract_layer_change");
    c.set_raw("retract_layer_change", &retract)
        .expect("boolean list");
    if c.get("filament_retract_layer_change").is_some() {
        let filament = all_off(c, "filament_retract_layer_change");
        c.set_raw("filament_retract_layer_change", &filament)
            .expect("boolean list");
    }
    c.set_raw("perimeters", "1").expect("integer");
    c.set_raw("top_solid_layers", "0").expect("integer");
    c.set_raw("fill_density", "0%").expect("percent");
    true
}

/// The values [`normalize`] forces, for showing them as fixed while spiral vase is on.
pub const FORCED: [(&str, &str); 3] = [
    ("perimeters", "1"),
    ("top_solid_layers", "0"),
    ("fill_density", "0%"),
];

/// What turning spiral vase on also sets, as PrusaSlicer's and OrcaSlicer's settings dialogs
/// do when asked: one wall, no top layers, no infill, no supports and no thin-wall detection
/// (Orca also no reversed overhang walls). Only the values that differ from `c` are returned.
pub fn companions(c: &Config) -> Vec<(String, String)> {
    let mut wanted = vec![
        ("perimeters", "1"),
        ("top_solid_layers", "0"),
        ("fill_density", "0%"),
        ("support_material", "0"),
        ("support_material_enforce_layers", "0"),
        ("thin_walls", "0"),
    ];
    if crate::rules::Rules::of(c).is_orca() && c.get("overhang_reverse").is_some() {
        wanted.push(("overhang_reverse", "0"));
    }
    wanted
        .into_iter()
        .filter(|(k, v)| {
            let mut probe = Config::new();
            probe.set_raw(k, v).is_ok() && probe.raw(k) != c.raw(k)
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The configuration checks of both slicers' `validate` for spiral vase that [`normalize`]
/// does not settle: supports.
pub fn validate_config(c: &Config) -> Result<(), String> {
    if !enabled(c) {
        return Ok(());
    }
    if c.bool("support_material").unwrap_or(false)
        || c.int("support_material_enforce_layers").unwrap_or(0) > 0
    {
        return Err(
            "Spiral vase can't print supports: turn supports off (support_material and \
             support_material_enforce_layers) or turn spiral vase off"
                .into(),
        );
    }
    Ok(())
}

/// What [`validate_objects`] needs to know of one prepared object instance.
pub struct ObjectInfo<'a> {
    pub name: &'a str,
    /// The object's own configuration (supports may be turned on per object).
    pub config: &'a Config,
    /// Its print regions' wall settings and wall tool (one entry without regions); a region
    /// painted with several filaments counts once per filament.
    pub regions: Vec<(&'a PrintSettings, Option<usize>)>,
}

/// `Print::validate` for spiral vase: one object instance (or objects printed one after
/// another), printed with one set of wall settings and one filament.
pub fn validate_objects(c: &Config, objects: &[ObjectInfo<'_>]) -> Result<(), String> {
    if !enabled(c) {
        return Ok(());
    }
    let orca = crate::rules::Rules::of(c).is_orca();
    if objects.len() > 1 && !crate::sequential::enabled(c) {
        return Err(if orca {
            "Spiral vase prints one object at a time: remove the other objects and copies, or \
             set the print sequence to \"By object\""
        } else {
            "Spiral vase prints one object at a time: remove the other objects and copies, or \
             print them one after another (complete_objects)"
        }
        .into());
    }
    for o in objects {
        validate_config(o.config).map_err(|e| format!("{}: {e}", o.name))?;
        let Some((first, rest)) = o.regions.split_first() else {
            continue;
        };
        // PrusaSlicer refuses any second region; Orca only regions whose walls differ
        // (`Layer::is_perimeter_compatible`), as it prints compatible regions' walls as one.
        let incompatible = rest
            .iter()
            .any(|other| !orca || other.1 != first.1 || !same_walls(first.0, other.0));
        if incompatible {
            return Err(format!(
                "{}: spiral vase prints an object with one filament and one set of wall \
                 settings; its parts or modifiers change them",
                o.name
            ));
        }
    }
    Ok(())
}

/// The resolved wall settings Orca's `Layer::is_perimeter_compatible` compares.
fn same_walls(a: &PrintSettings, b: &PrintSettings) -> bool {
    a.perimeters == b.perimeters
        && a.external_perimeters_first == b.external_perimeters_first
        && a.infill_first == b.infill_first
        && a.perimeter_speed == b.perimeter_speed
        && a.external_perimeter_speed == b.external_perimeter_speed
        && a.gap_fill_speed == b.gap_fill_speed
        && a.perimeter_extrusion_width == b.perimeter_extrusion_width
        && a.external_perimeter_extrusion_width == b.external_perimeter_extrusion_width
        && a.thin_walls == b.thin_walls
        && a.overhangs == b.overhangs
}

/// `slice_volumes`' `slicing_mode_normal_below_layer`: the first slice plane (of `zs`, the
/// planes the layers are sliced at) from which the object is sliced to its largest outline.
/// The bottom layers below it are sliced normally (holes stay, every island stays).
pub fn slice_from_layer(s: &PrintSettings, zs: &[f64]) -> usize {
    first_above(s.bottom_solid_layers, s.bottom_solid_min_thickness, zs)
}

/// [`slice_from_layer`] from a configuration (a model part's region settings).
pub fn slice_from_layer_config(c: &Config, zs: &[f64]) -> usize {
    first_above(
        c.int("bottom_solid_layers").unwrap_or(3).max(0) as usize,
        c.float("bottom_solid_min_thickness").unwrap_or(0.),
        zs,
    )
}

fn first_above(bottom_layers: usize, min_thickness: f64, zs: &[f64]) -> usize {
    let mut from = bottom_layers;
    while from < zs.len() && zs[from] < min_thickness - EPSILON {
        from += 1;
    }
    from
}

/// `MeshSlicingParams::SlicingMode::PositiveLargestContour` on a sliced layer: every ring is a
/// contour (holes close), and of the united islands only the one with the largest outline stays.
pub fn largest_contour(islands: &[ExPolygon]) -> Vec<ExPolygon> {
    if islands.is_empty() {
        return Vec::new();
    }
    let rings: Vec<ExPolygon> = islands
        .iter()
        .flat_map(|ex| ex.polygons())
        .filter(|ring| ring.len() >= 3)
        .map(|ring| {
            let mut contour = ring.clone();
            if signed_area(&contour) < 0. {
                contour.reverse();
            }
            ExPolygon {
                contour,
                holes: Vec::new(),
            }
        })
        .collect();
    let mut united = union_ex(&rings);
    keep_largest_contour_only(&mut united);
    united
}

/// `keep_largest_contour_only`: of several islands, the one whose outline encloses the largest
/// area.
pub fn keep_largest_contour_only(islands: &mut Vec<ExPolygon>) {
    if islands.len() > 1 {
        let mut best = 0;
        let mut max_area = 0.;
        for (i, ex) in islands.iter().enumerate() {
            let area = signed_area(&ex.contour);
            if area > max_area {
                max_area = area;
                best = i;
            }
        }
        let keep = islands.swap_remove(best);
        islands.clear();
        islands.push(keep);
    }
}

/// `LayerRegion::make_perimeters`' spiral vase condition: layer `index` (top at `print_z`) is
/// above the solid bottom, so its wall is one classic loop around its largest outline.
pub fn wall_layer(s: &PrintSettings, index: usize, print_z: f64) -> bool {
    s.spiral_vase
        && index >= s.bottom_solid_layers
        && print_z >= s.bottom_solid_min_thickness - EPSILON
}

/// The part of `process_layer`'s spiral vase condition the settings decide: not the first
/// layer of a print with a brim, not a skirt layer, no draft shield, above the solid bottom
/// layers. The exporter also requires one object and nothing but its wall on the layer.
pub fn spiral_layer(s: &PrintSettings, index: usize) -> bool {
    let adhesion = &s.adhesion;
    let infinite_skirt =
        adhesion.draft_shield == crate::brim::DraftShield::Enabled && adhesion.skirts > 0;
    // PrintObject::has_brim: a brim type with a width (Orca's auto brim decides its own).
    let brim = &adhesion.brim;
    let has_brim = brim.kind != crate::brim::BrimType::None
        && (brim.width > 0. || brim.kind == crate::brim::BrimType::Auto);
    s.spiral_vase
        && (index > 0 || !has_brim)
        && index >= adhesion.skirt_height
        && !infinite_skirt
        && index >= s.bottom_solid_layers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;

    fn square(x0: f64, y0: f64, size: f64) -> Vec<Point> {
        vec![
            Point::from_mm(x0, y0),
            Point::from_mm(x0 + size, y0),
            Point::from_mm(x0 + size, y0 + size),
            Point::from_mm(x0, y0 + size),
        ]
    }

    #[test]
    fn normalize_forces_one_wall_no_top_no_infill_no_layer_change_retraction() {
        let mut c = Config::defaults();
        c.set_raw("retract_layer_change", "1,1").unwrap();
        c.set_raw("filament_retract_layer_change", "1,nil").unwrap();
        assert!(!normalize(&mut c.clone()));
        c.set_raw("spiral_vase", "1").unwrap();
        assert!(normalize(&mut c));
        assert_eq!(c.int("perimeters"), Some(1));
        assert_eq!(c.int("top_solid_layers"), Some(0));
        assert_eq!(c.float("fill_density"), Some(0.));
        assert_eq!(c.raw("retract_layer_change").as_deref(), Some("0,0"));
        assert_eq!(
            c.raw("filament_retract_layer_change").as_deref(),
            Some("0,0")
        );
    }

    #[test]
    fn turning_it_on_sets_one_wall_no_top_no_infill_no_supports() {
        let mut c = Config::defaults();
        c.set_raw("perimeters", "2").unwrap();
        c.set_raw("top_solid_layers", "5").unwrap();
        c.set_raw("fill_density", "15%").unwrap();
        c.set_raw("support_material", "1").unwrap();
        c.set_raw("thin_walls", "0").unwrap();
        let keys: Vec<String> = companions(&c).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            [
                "perimeters",
                "top_solid_layers",
                "fill_density",
                "support_material"
            ]
        );
        for (k, v) in companions(&c) {
            c.set_raw(&k, &v).unwrap();
        }
        assert!(companions(&c).is_empty());
    }

    #[test]
    fn supports_and_several_objects_are_refused() {
        let mut c = Config::defaults();
        c.set_raw("spiral_vase", "1").unwrap();
        c.set_raw("support_material", "1").unwrap();
        assert!(validate_config(&c).unwrap_err().contains("supports"));
        c.set_raw("support_material", "0").unwrap();
        assert!(validate_config(&c).is_ok());
        let s = PrintSettings::core_one_hf04_speed_pla();
        let object = |name| ObjectInfo {
            name,
            config: &c,
            regions: vec![(&s, Some(0))],
        };
        assert!(validate_objects(&c, &[object("a")]).is_ok());
        assert!(
            validate_objects(&c, &[object("a"), object("b")])
                .unwrap_err()
                .contains("one object at a time")
        );
        let mut sequential = c.clone();
        sequential.set_raw("complete_objects", "1").unwrap();
        assert!(validate_objects(&sequential, &[object("a"), object("b")]).is_ok());
        let two_regions = ObjectInfo {
            name: "a",
            config: &c,
            regions: vec![(&s, Some(0)), (&s, Some(0))],
        };
        assert!(validate_objects(&c, &[two_regions]).is_err());
    }

    #[test]
    fn largest_contour_closes_holes_and_drops_smaller_islands() {
        let outer = ExPolygon {
            contour: square(0., 0., 20.),
            holes: vec![{
                let mut h = square(5., 5., 10.);
                h.reverse();
                h
            }],
        };
        let small = ExPolygon {
            contour: square(30., 0., 5.),
            holes: vec![],
        };
        let kept = largest_contour(&[small, outer]);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].holes.is_empty());
        assert!((kept[0].area_mm2() - 400.).abs() < 1e-6);
    }

    #[test]
    fn spiral_starts_above_the_bottom_layers_and_their_minimum_thickness() {
        let mut s = PrintSettings {
            spiral_vase: true,
            bottom_solid_layers: 3,
            bottom_solid_min_thickness: 0.75,
            ..PrintSettings::core_one_hf04_speed_pla()
        };
        // Slice planes of 0.2 mm layers: 0.1, 0.3, 0.5, 0.7, 0.9 …
        let zs: Vec<f64> = (0..10).map(|i| 0.1 + 0.2 * i as f64).collect();
        assert_eq!(slice_from_layer(&s, &zs), 4);
        assert!(!wall_layer(&s, 2, 0.6));
        assert!(wall_layer(&s, 3, 0.8));
        assert!(spiral_layer(&s, 3));
        assert!(!spiral_layer(&s, 2));
        s.spiral_vase = false;
        assert!(!wall_layer(&s, 3, 0.8) && !spiral_layer(&s, 3));
    }
}
