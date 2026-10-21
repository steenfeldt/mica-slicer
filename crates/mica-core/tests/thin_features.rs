// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Gap fill and thin walls on `tests/models/thin_features.stl` (a block with 0.5 / 0.8 / 1.2 mm slots and a
//! free-standing 0.6 mm wall), compared with PrusaSlicer 2.9.6.
//!
//! Reference: PrusaSlicer 2.9.6, CORE One HF0.4 / 0.20mm SPEED / Prusament PLA (profiles 2.5.12) with
//! `--perimeter-generator classic` (the profile uses Arachne), centred at 125,110; filament mm per layer
//! and `;TYPE:` read from its G-code (including G2/G3 extrusion). Layer 0 uses 0.2 mm elephant-foot
//! compensation; additional first-layer oracles cover the cube, hole block and Benchy.

use mica_core::geometry::{BoundingBox, Point, SCALE, polyline_length};
use mica_core::mesh::TriangleMesh;
use mica_core::print::{LayerSource, SlicedObject, prepare_object, slice_object};
use mica_core::settings::PrintSettings;
use mica_core::toolpath::Role;

fn model() -> TriangleMesh {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
    let mut m = TriangleMesh::load_stl(format!("{dir}thin_features.stl")).unwrap();
    m.place_on_bed(125.0, 110.0);
    m
}

fn filament_mm(obj: &SlicedObject, s: &PrintSettings, layer: usize, role: Role) -> f64 {
    let area = std::f64::consts::PI * (s.filament_diameter / 2.0).powi(2);
    obj.layers[layer]
        .paths
        .iter()
        .filter(|p| p.role == role)
        .map(|p| polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm() / area)
        .sum()
}

fn close(got: f64, want: f64, tol: f64) -> bool {
    (got - want).abs() <= want * tol
}

#[test]
fn gap_fill_matches_prusaslicer() {
    let s = classic_settings();
    let obj = slice_object(&model(), &s).unwrap();
    assert_eq!(obj.layers.len(), 25);
    for layer in [1, 5, 10, 20, 24] {
        let gap = filament_mm(&obj, &s, layer, Role::GapFill);
        assert!(close(gap, 0.605, 0.02), "layer {layer}: gap fill {gap}");
        let ext = filament_mm(&obj, &s, layer, Role::ExternalPerimeter);
        assert!(close(ext, 6.895, 0.02), "layer {layer}: external {ext}");
        let per = filament_mm(&obj, &s, layer, Role::Perimeter);
        assert!(close(per, 5.005, 0.02), "layer {layer}: perimeter {per}");
    }
    // Gap fill is subtracted from the infill area.
    let sparse = filament_mm(&obj, &s, 10, Role::InternalInfill);
    assert!(close(sparse, 3.822, 0.02), "sparse {sparse}");
    // Printed after the walls of its island, before its infill.
    let roles: Vec<Role> = obj.layers[10].paths.iter().map(|p| p.role).collect();
    let first_gap = roles.iter().position(|&r| r == Role::GapFill).unwrap();
    let last_gap = roles.iter().rposition(|&r| r == Role::GapFill).unwrap();
    assert!(roles[..first_gap].contains(&Role::Perimeter));
    assert!(!roles[..last_gap].contains(&Role::InternalInfill));
}

#[test]
fn no_gap_fill_when_disabled() {
    let s = PrintSettings {
        gap_fill_speed: 0.0,
        ..classic_settings()
    };
    let obj = slice_object(&model(), &s).unwrap();
    assert!(
        obj.layers
            .iter()
            .flat_map(|l| &l.paths)
            .all(|p| p.role != Role::GapFill)
    );
}

#[test]
fn thin_walls_match_prusaslicer() {
    // Same, with `--thin-walls`: the 0.6 mm wall becomes a single variable-width external perimeter
    // instead of a squashed loop.
    let s = PrintSettings {
        thin_walls: true,
        ..classic_settings()
    };
    let obj = slice_object(&model(), &s).unwrap();
    for layer in [1, 10, 24] {
        let ext = filament_mm(&obj, &s, layer, Role::ExternalPerimeter);
        assert!(close(ext, 6.632, 0.02), "layer {layer}: external {ext}");
        let gap = filament_mm(&obj, &s, layer, Role::GapFill);
        assert!(close(gap, 0.605, 0.02), "layer {layer}: gap fill {gap}");
    }
}

fn load_model(name: &str) -> TriangleMesh {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(format!("{name}.stl"));
    let mut mesh = TriangleMesh::load_stl(path).unwrap();
    mesh.place_on_bed(125.0, 110.0);
    mesh
}

fn layer_filament(
    layer: &mica_core::toolpath::LayerToolpaths,
    settings: &PrintSettings,
    role: Role,
) -> f64 {
    let filament_area = std::f64::consts::PI * (settings.filament_diameter / 2.0).powi(2);
    layer
        .paths
        .iter()
        .filter(|p| p.role == role)
        .map(|p| polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm() / filament_area)
        .sum()
}

#[test]
fn compensated_first_layers_match_prusaslicer() {
    // PrusaSlicer 2.9.6, supplied oracle command, measured 2026-10-15.
    // Reference datadir uses vendor 2.5.11 (first-layer settings equal 2.5.12).
    let settings = classic_settings();
    for (model, external, perimeter, solid) in [
        ("thin_features", 7.80548, 5.70751, 16.35912),
        ("hole_block", 3.96149, 3.93176, 19.88549),
        ("cube20", 2.90140, 2.76244, 26.51720),
    ] {
        let object = prepare_object(&load_model(model), &settings).unwrap();
        let layer = object.toolpaths(0..1).remove(0);
        for (role, want) in [
            (Role::ExternalPerimeter, external),
            (Role::Perimeter, perimeter),
            (Role::SolidInfill, solid),
        ] {
            let got = layer_filament(&layer, &settings, role);
            assert!(
                close(got, want, 0.02),
                "{model} {role:?}: {got} vs Prusa {want}"
            );
        }
        for role in [Role::GapFill, Role::InternalInfill, Role::TopSolidInfill] {
            assert_eq!(
                layer_filament(&layer, &settings, role),
                0.0,
                "{model} {role:?}"
            );
        }
    }
}

#[test]
fn benchy_first_layer_records_remaining_pipeline_difference() {
    // Benchy still differs with elephant-foot disabled: wall/gap-fill geometry uses Clipper2
    // rather than Prusa's modified Clipper, even after contour simplification. The slicer also
    // reports open contours on this mesh. Do not silently claim 2% parity on this model.
    // Pin BOTH oracle and Mica measurements; the tight Mica bounds detect a compensation regression,
    // while the explicit discrepancy budgets document what remains to be fixed in the pipeline.
    let mesh = load_model("3DBenchy");
    for (compensation, rows) in [
        (
            0.2,
            [
                (Role::ExternalPerimeter, 10.42570, 10.68942, 0.03),
                (Role::Perimeter, 8.56008, 8.80512, 0.035),
                (Role::SolidInfill, 15.70387, 14.96355, 0.05),
                (Role::GapFill, 0.18474, 0.20835, 0.13),
            ],
        ),
        (
            0.0,
            [
                (Role::ExternalPerimeter, 10.19850, 10.32733, 0.015),
                (Role::Perimeter, 9.25163, 9.50573, 0.03),
                (Role::SolidInfill, 18.67183, 18.30709, 0.025),
                (Role::GapFill, 0.32048, 0.35781, 0.12),
            ],
        ),
    ] {
        let settings = PrintSettings {
            elefant_foot_compensation: compensation,
            ..classic_settings()
        };
        let object = prepare_object(&mesh, &settings).unwrap();
        let layer = object.toolpaths(0..1).remove(0);
        for (role, oracle, mica, budget) in rows {
            let got = layer_filament(&layer, &settings, role);
            assert!(
                close(got, mica, 0.01),
                "Benchy {compensation} {role:?}: {got} vs recorded Mica {mica}"
            );
            assert!(
                close(got, oracle, budget),
                "Benchy {compensation} {role:?}: {got} vs Prusa {oracle}"
            );
        }
    }
}

fn outer_bounds(layer: &mica_core::toolpath::LayerToolpaths) -> BoundingBox {
    BoundingBox::from_points(
        layer
            .paths
            .iter()
            .filter(|p| p.role == Role::ExternalPerimeter)
            .flat_map(|p| &p.points),
    )
    .unwrap()
}

#[test]
fn cube_compensation_xy_bounds_and_raft_gate() {
    let mesh = load_model("cube20");
    for xy in [-0.1, 0.0, 0.1] {
        let settings = PrintSettings {
            xy_size_compensation: xy,
            ..classic_settings()
        };
        let object = prepare_object(&mesh, &settings).unwrap();
        let layers = object.toolpaths(0..2);
        let source_bounds = BoundingBox {
            min: Point::from_mm(115.0 - xy, 100.0 - xy),
            max: Point::from_mm(135.0 + xy, 120.0 + xy),
        };
        // Prusa's first_layer_print_min/max use lslices before elephant-foot compensation.
        assert_eq!(object.first_layer_bbox(), Some(source_bounds));
        assert_eq!(
            BoundingBox::from_points(
                object
                    .uncompensated_first_layer
                    .iter()
                    .flat_map(|p| &p.contour)
            ),
            Some(source_bounds)
        );
        // 0.25 mm is half of the first-layer external flow width, plus 0.2 mm compensation.
        assert_eq!(
            outer_bounds(&layers[0]),
            BoundingBox {
                min: Point::from_mm(115.45 - xy, 100.45 - xy),
                max: Point::from_mm(134.55 + xy, 119.55 + xy)
            }
        );
        // Upper layers get XY compensation only, with the normal 0.45 mm external width.
        // Flow stores width as f32 and truncates its scaled dimension, so 0.45 mm
        // becomes 449999 scaled units. Allow its one-unit half-width rounding.
        let actual = outer_bounds(&layers[1]);
        let expected = BoundingBox {
            min: Point::from_mm(115.225 - xy, 100.225 - xy),
            max: Point::from_mm(134.775 + xy, 119.775 + xy),
        };
        assert!(
            (actual.min.x - expected.min.x).abs() <= 1
                && (actual.min.y - expected.min.y).abs() <= 1
                && (actual.max.x - expected.max.x).abs() <= 1
                && (actual.max.y - expected.max.y).abs() <= 1,
            "{actual:?} vs {expected:?}"
        );
    }
    let disabled = PrintSettings {
        elefant_foot_compensation: 0.0,
        ..classic_settings()
    };
    let raft = PrintSettings {
        raft_layers: 1,
        ..classic_settings()
    };
    let uncompensated = prepare_object(&mesh, &disabled).unwrap().toolpaths(0..2);
    let with_raft = prepare_object(&mesh, &raft).unwrap().toolpaths(0..2);
    assert_eq!(
        outer_bounds(&uncompensated[0]),
        BoundingBox {
            min: Point::from_mm(115.25, 100.25),
            max: Point::from_mm(134.75, 119.75)
        }
    );
    // A real raft now precedes the object. It disables elephant-foot compensation and the
    // object uses normal extrusion width, since its first layer is no longer on the bed.
    assert!(with_raft[0].paths.iter().all(|p| matches!(
        p.role,
        Role::SupportMaterial | Role::SupportMaterialInterface
    )));
    assert_eq!(outer_bounds(&with_raft[1]), outer_bounds(&uncompensated[1]));
    assert!((with_raft[1].print_z - 0.55).abs() < 1e-6);
}

// These oracles were measured with --perimeter-generator classic.
fn classic_settings() -> PrintSettings {
    PrintSettings {
        perimeter_generator: mica_core::settings::PerimeterGenerator::Classic,
        ..PrintSettings::core_one_hf04_speed_pla()
    }
}
