// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: validate
// Source: orca@8500fcdcca src/libslic3r/libslic3r.h :: MAX_LINE_WIDTH_MULTIPLIER
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::validate
// Source: prusaslicer@b028299c77 src/libslic3r/Print.cpp :: Print::validate
// Changes: Mica's own preparation checks; the extrusion-width upper bounds follow Orca's config validation (5× the nozzle for line widths including first layer and support, the nozzle for bridges) on resolved settings; the XY-compensation bed check is Mica's; absolute extrusion is refused (not implemented yet) ahead of the upstream "G92 E0" checks, which are ported with their messages (regex_g92e0 matched line by line; Orca's key names under Orca rules).
// Source: prusaslicer@b028299c77 src/libslic3r/BuildVolume.cpp :: BuildVolume::object_state
// Source: orca@8500fcdcca src/slic3r/GUI/PartPlate.cpp :: PartPlate::check_outside
// Changes: Placement tests the vertices against a round or polygonal bed; under Orca rules an object whose convex hull reaches into a bed exclusion area is refused.

//! Input checks at the fallible preparation boundary, before geometry indexing or allocation.
use crate::{config::Config, mesh::TriangleMesh, settings::PrintSettings};

pub fn mesh(mesh: &TriangleMesh) -> Result<(), String> {
    if mesh.vertices.is_empty() || mesh.faces.is_empty() {
        return Err("model has no triangles".into());
    }
    if mesh
        .vertices
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
    {
        return Err(
            "model coordinates must be finite and within ±1,000,000 mm; check model units".into(),
        );
    }
    if mesh
        .faces
        .iter()
        .flatten()
        .any(|&v| v as usize >= mesh.vertices.len())
    {
        return Err("triangle references a missing vertex".into());
    }
    let (min, max) = mesh.bounds();
    if (0..3).any(|i| max[i] - min[i] <= 1e-7) {
        return Err("model has zero thickness/volume".into());
    }
    let has_area = mesh.faces.iter().any(|f| {
        let [a, b, c] = f.map(|i| mesh.vertices[i as usize].map(f64::from));
        let u: [f64; 3] = std::array::from_fn(|i| b[i] - a[i]);
        let v: [f64; 3] = std::array::from_fn(|i| c[i] - a[i]);
        (u[1] * v[2] - u[2] * v[1])
            .hypot(u[2] * v[0] - u[0] * v[2])
            .hypot(u[0] * v[1] - u[1] * v[0])
            > 1e-12
    });
    if !has_area {
        return Err("model contains only degenerate triangles".into());
    }
    Ok(())
}

/// The thinnest layer Mica slices (mm).
pub const MIN_LAYER_HEIGHT: f64 = 0.01;

/// Widest extrusion Mica slices, as a multiple of the nozzle diameter (OrcaSlicer's
/// `MAX_LINE_WIDTH_MULTIPLIER`, libslic3r.h).
pub const MAX_LINE_WIDTH_MULTIPLIER: f64 = 5.;

pub fn settings(s: &PrintSettings) -> Result<(), String> {
    if s.perimeters > 1000
        || s.top_solid_layers > 100_000
        || s.bottom_solid_layers > 100_000
        || s.raft_layers > 1000
    {
        return Err("wall/skin/raft count exceeds the safe preparation limit".into());
    }
    if !s.fill_density.is_finite() || !(0. ..=1.).contains(&s.fill_density) {
        return Err("infill density must be within 0–100%".into());
    }
    s.arachne
        .validate()
        .map_err(|e| format!("invalid Arachne settings: {e}"))?;
    for (name, value) in [
        ("layer height", s.layer_height),
        ("first layer height", s.first_layer_height),
        ("nozzle diameter", s.nozzle_diameter),
        ("filament diameter", s.filament_diameter),
    ] {
        if !value.is_finite() || value <= 0. {
            return Err(format!("{name} must be finite and positive"));
        }
    }
    // G-code carries Z with three decimals: a thinner layer is written as a layer at Z0
    // (or two layers at one height) and cannot be printed.
    for (name, value) in [
        ("layer height", s.layer_height),
        ("first layer height", s.first_layer_height),
    ] {
        if value < MIN_LAYER_HEIGHT {
            return Err(format!(
                "{name} {value} mm is below the {MIN_LAYER_HEIGHT} mm minimum"
            ));
        }
    }
    if s.layer_height > s.nozzle_diameter || s.first_layer_height > s.nozzle_diameter {
        return Err("layer height exceeds nozzle diameter; lower the layer height or select the correct nozzle".into());
    }
    for width in [
        s.perimeter_extrusion_width,
        s.external_perimeter_extrusion_width,
        s.infill_extrusion_width,
        s.solid_infill_extrusion_width,
        s.top_infill_extrusion_width,
        s.first_layer_extrusion_width,
    ] {
        if !width.is_finite()
            || width <= s.layer_height.max(s.first_layer_height) * (1. - std::f64::consts::PI / 4.)
        {
            return Err("extrusion width is too small for the layer height".into());
        }
    }
    // Upper bound, as OrcaSlicer's `validate(FullPrintConfig)`: line widths above
    // MAX_LINE_WIDTH_MULTIPLIER (5) times the nozzle, the first-layer and support widths
    // included, and bridge widths above the nozzle are refused. (PrusaSlicer's
    // `validate_extrusion_width` refuses its main widths from 3 times; the more permissive
    // bound keeps every value either accepts.) A 17 mm first-layer width (GUI monkey) printed a
    // raft base pushing 266 mm of filament through single moves; a 100 mm bridge width kept a
    // slice in uncancellable bridge geometry for most of a minute.
    if let Some(bridge) = s.orca_walls.bridge_line_width {
        let width = bridge.resolve(s.nozzle_diameter, 1.0);
        if !width.is_finite() || width > s.nozzle_diameter + 1e-9 {
            return Err(format!(
                "bridge line width {width} mm exceeds the {} mm nozzle diameter",
                s.nozzle_diameter
            ));
        }
    }
    if s.fill_pattern == crate::fill::Pattern::LockedZag && s.fill_density > 0. {
        for (name, width) in [
            ("skin", s.zag.skin_infill_line_width),
            ("skeleton", s.zag.skeleton_infill_line_width),
        ] {
            let width = crate::fill::locked_zag_line_width(width, s.nozzle_diameter);
            // Below a fifth of the nozzle a line has no cross-section at a layer height as tall
            // as the nozzle (combined infill).
            if !width.is_finite()
                || width > s.nozzle_diameter * MAX_LINE_WIDTH_MULTIPLIER
                || width < 0.25 * s.nozzle_diameter
            {
                return Err(format!(
                    "{name} line width {width} mm must be between a quarter of the {} mm nozzle \
                     and {MAX_LINE_WIDTH_MULTIPLIER}× it",
                    s.nozzle_diameter
                ));
            }
        }
    }
    for (name, width) in [
        ("perimeter", s.perimeter_extrusion_width),
        ("external perimeter", s.external_perimeter_extrusion_width),
        ("infill", s.infill_extrusion_width),
        ("solid infill", s.solid_infill_extrusion_width),
        ("top infill", s.top_infill_extrusion_width),
        ("first layer", s.first_layer_extrusion_width),
        ("support", s.support_material_extrusion_width),
        (
            "first-layer support",
            s.support_material_first_layer_extrusion_width,
        ),
    ] {
        if width > s.nozzle_diameter * MAX_LINE_WIDTH_MULTIPLIER {
            return Err(format!(
                "{name} extrusion width {width} mm exceeds {MAX_LINE_WIDTH_MULTIPLIER}× the \
                 {} mm nozzle and cannot be extruded",
                s.nozzle_diameter
            ));
        }
    }
    Ok(())
}

/// Why the corners `bed` cannot be a bed (`None` when they can): fewer than three corners,
/// a corner listed twice (the sign of a damaged value, such as `0x0,0x0,360x360,0x360` from a
/// bed shape written one point per extruder; GUI monkey test, seed 3004), a value that is not
/// a finite number, or no area.
pub fn bed_shape_problem(bed: &[(f64, f64)]) -> Option<String> {
    if bed.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return Some("a corner is not a number".into());
    }
    // A closed outline (the first corner repeated at the end) is the same polygon.
    let bed = match bed {
        [first, rest @ .., last] if bed.len() > 3 && first == last => &bed[..rest.len() + 1],
        _ => bed,
    };
    if bed.len() < 3 {
        return Some(format!(
            "a bed needs at least three corners, and this one has {}",
            bed.len()
        ));
    }
    for (i, p) in bed.iter().enumerate() {
        if bed[..i].contains(p) {
            return Some(format!("the corner {}x{} is listed twice", p.0, p.1));
        }
    }
    let area = (0..bed.len())
        .map(|i| {
            let (a, b) = (bed[i], bed[(i + 1) % bed.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        / 2.;
    if area.abs() < 1. {
        return Some("its corners enclose no area".into());
    }
    None
}

/// The start of [`placement`]'s warning about a model that reaches below the bed.
pub const BELOW_BED: &str = "model extends below the bed";

/// Cheap placement checks before slicing. The G-code validator checks actual path polygons later.
pub fn placement(mesh: &TriangleMesh, c: &Config) -> Result<Vec<String>, String> {
    self::mesh(mesh)?;
    let (min, max) = mesh.bounds();
    let bed = c.points("bed_shape").ok_or("printer has no bed shape")?;
    if let Some(problem) = bed_shape_problem(&bed) {
        return Err(format!("the printer's bed shape is not a bed: {problem}"));
    }
    let x0 = bed.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let x1 = bed.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let y0 = bed.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let y1 = bed.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    // The bed outline (a delta's circle, a polygon) as PrusaSlicer's object test
    // (`BuildVolume::object_state`), with Mica's 0.02 mm tolerance: every vertex on the bed.
    let volume = crate::build_volume::BuildVolume::from_config(c);
    let off_bed = match &volume {
        Some(v) if v.shape() != crate::build_volume::Shape::Rectangle => mesh
            .vertices
            .iter()
            .any(|p| !v.contains_xy(f64::from(p[0]), f64::from(p[1]), 0.02)),
        _ => {
            f64::from(min[0]) < x0 - 0.02
                || f64::from(max[0]) > x1 + 0.02
                || f64::from(min[1]) < y0 - 0.02
                || f64::from(max[1]) > y1 + 0.02
        }
    };
    if off_bed || f64::from(max[2]) > c.float("max_print_height").unwrap_or(250.) + 0.02 {
        return Err(
            "model is outside the printer's build volume; scale or arrange it on the bed".into(),
        );
    }
    // Orca rules: an object reaching into a bed exclusion area is not printed
    // (`PartPlate::check_outside`: its convex hull against each area); Mica refuses it.
    if let Some(v) = volume.as_ref().filter(|v| !v.zones.is_empty()) {
        let hull = crate::build_volume::convex_hull(
            mesh.vertices
                .iter()
                .map(|p| [f64::from(p[0]), f64::from(p[1])])
                .collect(),
        );
        if let Some(zone) = v.zones.iter().find(|z| z.overlaps_convex(&hull, 0.02)) {
            return Err(format!(
                "model reaches into the excluded area {} of the bed; move it out of the area",
                zone.describe()
            ));
        }
    }
    // Positive XY compensation (Prusa's size, Orca's contour compensation) grows every
    // layer's outline: grown past the bed, the walls are printed outside it. A 1 km value
    // (GUI monkey) also made each layer kilometres wide, minutes of geometry between cancel
    // checks before the job found nothing it could print.
    let growth = ["xy_size_compensation", "xy_contour_compensation"]
        .iter()
        .map(|k| c.float(k).unwrap_or(0.).max(0.))
        .sum::<f64>();
    if !growth.is_finite()
        || f64::from(min[0]) - growth < x0 - 0.02
        || f64::from(max[0]) + growth > x1 + 0.02
        || f64::from(min[1]) - growth < y0 - 0.02
        || f64::from(max[1]) + growth > y1 + 0.02
    {
        return Err(format!(
            "XY compensation of {growth} mm grows the model past the printer's bed; reduce it \
             or move the model inwards"
        ));
    }
    if max[2] <= 0. {
        return Err("model is entirely below the bed".into());
    }
    let mut warnings = Vec::new();
    if min[2] < -0.001 {
        warnings.push(format!("{BELOW_BED}; geometry below Z0 will be clipped"));
    }
    if (0..3).all(|i| max[i] - min[i] < 1.) {
        warnings.push("model is smaller than 1 mm; check imported units".into());
    }
    let mut parents: Vec<usize> = (0..mesh.vertices.len()).collect();
    fn root(parents: &mut [usize], mut i: usize) -> usize {
        while parents[i] != i {
            parents[i] = parents[parents[i]];
            i = parents[i];
        }
        i
    }
    let mut edges = std::collections::HashMap::<(u32, u32), usize>::new();
    for f in &mesh.faces {
        for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[0])] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            let ra = root(&mut parents, a as usize);
            let rb = root(&mut parents, b as usize);
            parents[ra] = rb;
        }
    }
    let open = edges.values().filter(|&&n| n != 2).count();
    if open > 0 {
        warnings.push(format!(
            "model has {open} open/non-manifold edges; inspect repaired contours before printing"
        ));
    }
    let mut shells = std::collections::HashMap::<usize, [[f32; 3]; 2]>::new();
    for (i, &v) in mesh.vertices.iter().enumerate() {
        let bounds = shells.entry(root(&mut parents, i)).or_insert([v, v]);
        for (axis, &value) in v.iter().enumerate() {
            bounds[0][axis] = bounds[0][axis].min(value);
            bounds[1][axis] = bounds[1][axis].max(value);
        }
    }
    let shells: Vec<_> = shells.values().collect();
    if shells.len() > 128 {
        warnings.push(
            "model has more than 128 disconnected shells; inspect overlaps before printing".into(),
        );
    } else if shells.iter().enumerate().any(|(i, a)| {
        shells[i + 1..].iter().any(|b| {
            (0..3).all(|axis| a[0][axis] < b[1][axis] - 1e-5 && b[0][axis] < a[1][axis] - 1e-5)
        })
    }) {
        warnings.push("overlapping shell bounds: possible self-intersection; intersecting shells are unioned during slicing".into());
    }
    Ok(warnings)
}

/// Why Mica refuses absolute extrusion (`use_relative_e_distances = 0`): the writer only
/// writes relative E values.
pub const ABSOLUTE_E_UNSUPPORTED: &str = "Absolute extrusion (M82) isn't supported yet; turn on \
     \"Use relative E distances\" in Printer settings";

/// Whether Mica's writer supports absolute extrusion. Until it does, [`extrusion_mode_problem`]
/// refuses it before the upstream checks.
const ABSOLUTE_E_SUPPORTED: bool = false;

/// PrusaSlicer's `regex_g92e0` (`^[ \t]*[gG]92[ \t]*[eE](0(\.0*)?|\.0+)[ \t]*(;.*)?$`, matched
/// per line): a line that resets the extruder position to zero.
fn resets_extruder(gcode: &str) -> bool {
    gcode.lines().any(|line| {
        let blank = [' ', '\t'];
        let Some(rest) = line.trim_start_matches(blank).strip_prefix(['g', 'G']) else {
            return false;
        };
        let Some(rest) = rest.strip_prefix("92") else {
            return false;
        };
        let Some(rest) = rest.trim_start_matches(blank).strip_prefix(['e', 'E']) else {
            return false;
        };
        let zeros = |s: &str| s.len() - s.trim_start_matches('0').len();
        // `0`, `0.`, `0.00`, or `.0`, `.00`.
        let rest = if let Some(r) = rest.strip_prefix('0') {
            r.strip_prefix('.').map_or(r, |r| &r[zeros(r)..])
        } else if let Some(r) = rest.strip_prefix('.')
            && zeros(r) > 0
        {
            &r[zeros(r)..]
        } else {
            return false;
        };
        let rest = rest.trim_start_matches(blank).trim_end_matches('\r');
        rest.is_empty() || rest.starts_with(';')
    })
}

/// The extrusion mode check of both slicers' `Print::validate`, for absolute extrusion: Mica
/// does not write absolute E yet ([`ABSOLUTE_E_UNSUPPORTED`]); upstream refuses a "G92 E0" in
/// the before-layer or layer-change G-code, which would reset the absolute position every
/// layer. `None` for relative extrusion.
pub fn extrusion_mode_problem(c: &Config) -> Option<String> {
    if c.bool("use_relative_e_distances").unwrap_or(true) {
        return None;
    }
    if !ABSOLUTE_E_SUPPORTED {
        return Some(ABSOLUTE_E_UNSUPPORTED.into());
    }
    absolute_e_reset_problem(c)
}

/// PrusaSlicer's (and Orca's) refusal of "G92 E0" in layer G-code with absolute extrusion, in
/// their words; under Orca rules with Orca's key names.
fn absolute_e_reset_problem(c: &Config) -> Option<String> {
    let orca = crate::rules::Rules::of(c).is_orca();
    for (key, orca_key) in [
        ("before_layer_gcode", "before_layer_change_gcode"),
        ("layer_gcode", "layer_change_gcode"),
    ] {
        if c.string(key).is_some_and(resets_extruder) {
            return Some(format!(
                "\"G92 E0\" was found in {}, which is incompatible with absolute extruder \
                 addressing.",
                if orca { orca_key } else { key }
            ));
        }
    }
    None
}

/// Settings which are valid but likely unintended are surfaced by both job front ends.
pub fn config_warnings(c: &Config) -> Vec<String> {
    let mut warnings = Vec::new();
    if c.int("perimeters") == Some(0) && c.float("fill_density") == Some(0.) {
        warnings.push("0 walls and 0 infill: only top/bottom skins and supports will print".into());
    }
    warnings.extend(unsupported_settings(c));
    warnings
}

/// Settings a profile or project turns on that Mica does not apply yet, one sentence each:
/// the slice goes on without them (the GUI hides them, `mica-gui`'s `settings::unimplemented`).
pub fn unsupported_settings(c: &Config) -> Vec<String> {
    let orca = crate::rules::Rules::of(c).is_orca();
    // Text lists (several scripts, several substitutions): any non-blank entry.
    let any_text = |key: &str| {
        (0..c.vector_len(key).max(1))
            .any(|i| c.string_at(key, i).is_some_and(|s| !s.trim().is_empty()))
    };
    // Per-tool values: any tool's.
    let any_nonzero = |key: &str| {
        c.raw(key).is_some_and(|raw| {
            raw.split(',').any(|v| {
                v.trim()
                    .trim_end_matches('%')
                    .parse::<f64>()
                    .is_ok_and(|v| v != 0.)
            })
        })
    };
    let mut out = Vec::new();
    if any_text("post_process") {
        out.push("Post-processing scripts aren't run yet; the G-code is saved without them".into());
    }
    // A PrusaSlicer option: OrcaSlicer has none, so its rules ignore it as Mica does.
    if !orca && any_text("gcode_substitutions") {
        out.push("G-code substitutions aren't applied yet; the G-code is saved as sliced".into());
    }
    if any_nonzero("filament_shrinkage_compensation_xy")
        || any_nonzero("filament_shrinkage_compensation_z")
    {
        out.push(
            "Filament shrinkage compensation isn't applied yet; the model prints at its own size"
                .into(),
        );
    }
    // PrusaSlicer's first object layer above a raft.
    if !orca
        && c.int("raft_layers").unwrap_or(0) > 0
        && (c
            .raw("first_layer_speed_over_raft")
            .is_some_and(|v| v.trim() != "30")
            || c.float("first_layer_acceleration_over_raft")
                .is_some_and(|v| v != 0.))
    {
        out.push("First layer speed and acceleration over raft aren't applied yet".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GUI monkey test, seed 3004: a bed written one point per extruder repeats a corner.
    #[test]
    fn g92_e0_lines_are_recognized_as_upstream_does() {
        for yes in [
            "G92 E0",
            "G92 E0.0",
            "  g92\te.000 ; reset",
            "M107\nG92 E0.\n",
            "G92 E.0",
            "G92E0",
        ] {
            assert!(resets_extruder(yes), "{yes:?}");
        }
        for no in [
            "G92 E1",
            "G92 E",
            "G92 E.",
            "; G92 E0",
            "G92 E0 X1",
            "G1 E0",
            "G920 E0",
        ] {
            assert!(!resets_extruder(no), "{no:?}");
        }
    }

    #[test]
    fn absolute_extrusion_is_refused_with_the_upstream_reasons_behind() {
        let mut c = Config::defaults();
        c.set_raw("use_relative_e_distances", "1").unwrap();
        assert_eq!(extrusion_mode_problem(&c), None);
        c.set_raw("use_relative_e_distances", "0").unwrap();
        assert_eq!(
            extrusion_mode_problem(&c).as_deref(),
            Some(ABSOLUTE_E_UNSUPPORTED)
        );
        // Once absolute E is written, PrusaSlicer's own check applies (the CORE One's
        // before-layer G-code resets E with "G92 E0.0").
        c.set_raw(
            "before_layer_gcode",
            ";BEFORE_LAYER_CHANGE\nG92 E0.0\n;[layer_z]",
        )
        .unwrap();
        assert_eq!(
            absolute_e_reset_problem(&c).as_deref(),
            Some(
                "\"G92 E0\" was found in before_layer_gcode, which is incompatible with \
                 absolute extruder addressing."
            )
        );
        c.set_raw("before_layer_gcode", "").unwrap();
        assert_eq!(absolute_e_reset_problem(&c), None);
        c.set_raw("layer_gcode", "G92 E0").unwrap();
        crate::rules::Rules::OrcaSlicer.set(&mut c);
        assert!(
            absolute_e_reset_problem(&c)
                .unwrap()
                .starts_with("\"G92 E0\" was found in layer_change_gcode,")
        );
    }

    #[test]
    fn a_bed_needs_three_distinct_corners_and_an_area() {
        let sq = [(0., 0.), (250., 0.), (250., 210.), (0., 210.)];
        assert_eq!(bed_shape_problem(&sq), None);
        // A closed outline is the same bed.
        assert_eq!(
            bed_shape_problem(&[sq[0], sq[1], sq[2], sq[3], sq[0]]),
            None
        );
        let damaged = [(0., 0.), (0., 0.), (360., 360.), (0., 360.)];
        assert!(
            bed_shape_problem(&damaged)
                .unwrap()
                .contains("listed twice")
        );
        assert!(bed_shape_problem(&[(0., 0.)]).is_some());
        assert!(bed_shape_problem(&[(0., 0.), (10., 0.), (20., 0.)]).is_some());
        assert!(bed_shape_problem(&[(0., 0.), (f64::NAN, 0.), (1., 1.)]).is_some());
        let mut c = Config::defaults();
        c.set_raw("bed_shape", "0x0,0x0,360x360,0x360").unwrap();
        let mesh = crate::mesh::tests::box_mesh([170., 170., 0.], [190., 190., 20.]);
        assert!(placement(&mesh, &c).unwrap_err().contains("bed shape"));
    }
}
