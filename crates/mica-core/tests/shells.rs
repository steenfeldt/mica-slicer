// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Top/bottom surfaces and solid shells (`mica_core::shells`) compared with PrusaSlicer 2.9.6.
//!
//! Reference: PrusaSlicer 2.9.6, CORE One HF0.4 / 0.20mm SPEED / Prusament PLA (profiles 2.5.12:
//! `ensure_vertical_shell_thickness = enabled`, 5 top layers / 0.7 mm, 3 bottom layers / 0.5 mm) with
//! `--perimeter-generator classic --elefant-foot-compensation 0`, centred at 125,110; filament mm per layer
//! and `;TYPE:` read from its G-code.
//!
//! Bridge-over-infill and narrow internal solids are enabled. Bridge roles are checked separately
//! below; reference Prusa uses the same TYPE label for external and internal bridges.

use mica_core::geometry::{SCALE, polyline_length};
use mica_core::mesh::TriangleMesh;
use mica_core::print::{SlicedObject, slice_object};
use mica_core::settings::PrintSettings;
use mica_core::toolpath::Role;

fn model(name: &str) -> TriangleMesh {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
    let mut m = TriangleMesh::load_stl(format!("{dir}{name}")).unwrap();
    m.place_on_bed(125.0, 110.0);
    m
}

fn filament_mm(obj: &SlicedObject, s: &PrintSettings, layers: &[usize], role: Role) -> f64 {
    let area = std::f64::consts::PI * (s.filament_diameter / 2.0).powi(2);
    layers
        .iter()
        .flat_map(|&l| &obj.layers[l].paths)
        .filter(|p| p.role == role)
        .map(|p| polyline_length(&p.points) / SCALE * p.flow.mm3_per_mm() / area)
        .sum()
}

#[track_caller]
fn check(what: &str, got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= want * tol,
        "{what}: {got:.3} mm, PrusaSlicer {want:.3} mm"
    );
}

/// `tests/models/shells.stl`: a stepped pyramid (top surfaces at 4, 8 and 12 mm) and a cone (sloping wall).
#[test]
fn stepped_pyramid_and_cone_match_prusaslicer() {
    let s = PrintSettings::core_one_hf04_speed_pla();
    let obj = slice_object(&model("shells.stl"), &s).unwrap();
    assert_eq!(obj.layers.len(), 60);
    let layer = |z: f64| (z / 0.2).round() as usize - 1;
    let solid = |zs: &[f64]| {
        let ls: Vec<usize> = zs.iter().map(|&z| layer(z)).collect();
        filament_mm(&obj, &s, &ls, Role::SolidInfill)
    };
    let sparse = |zs: &[f64]| {
        let ls: Vec<usize> = zs.iter().map(|&z| layer(z)).collect();
        filament_mm(&obj, &s, &ls, Role::InternalInfill)
    };
    let top = |z: f64| filament_mm(&obj, &s, &[layer(z)], Role::TopSolidInfill);

    // Bottom layers: 3 layers (0.6 mm ≥ bottom_solid_min_thickness 0.5).
    for (z, want) in [(0.4, 63.30), (0.6, 62.67)] {
        check(&format!("bottom solid at z={z}"), solid(&[z]), want, 0.03);
    }
    // Under each top surface: top_solid_layers 5 (4 solid layers below the top layer).
    for (z, want) in [
        (3.4, 26.99),
        (3.6, 26.94),
        (3.8, 26.88),
        (7.4, 15.43),
        (7.8, 15.38),
    ] {
        check(
            &format!("solid under a step at z={z}"),
            solid(&[z]),
            want,
            0.03,
        );
    }
    for (z, want) in [(11.4, 5.18), (11.6, 5.07), (11.8, 4.84)] {
        check(
            &format!("solid under the top at z={z}"),
            solid(&[z]),
            want,
            0.03,
        );
    }
    for (z, want) in [(4.0, 24.71), (8.0, 13.94), (12.0, 4.75)] {
        check(&format!("top at z={z}"), top(z), want, 0.03);
    }
    // The cone's sloping wall: ensure_vertical_shell_thickness keeps a solid ring inside it on every layer.
    // Summed over the layers between the steps (single layers vary by a few percent with the fill lines).
    let range = |a: f64, b: f64| -> Vec<f64> {
        (layer(a)..=layer(b))
            .map(|l| (l + 1) as f64 * 0.2)
            .collect()
    };
    for (a, b, want_solid, want_sparse) in [
        (0.8, 2.8, 25.35, 108.00),
        (4.2, 6.8, 23.37, 61.12),
        (8.2, 10.8, 13.95, 19.02),
    ] {
        let zs = range(a, b);
        check(&format!("solid z={a}..{b}"), solid(&zs), want_solid, 0.03);
        check(
            &format!("sparse z={a}..{b}"),
            sparse(&zs),
            want_sparse,
            0.03,
        );
    }
    for (z, want_solid, want_sparse) in [(1.0, 2.42, 10.13), (2.0, 2.30, 9.73), (6.0, 1.58, 4.00)] {
        check(
            &format!("cone solid at z={z}"),
            solid(&[z]),
            want_solid,
            0.03,
        );
        check(&format!("sparse at z={z}"), sparse(&[z]), want_sparse, 0.03);
    }
}

/// 3DBenchy, per-role totals.
#[test]
fn benchy_roles_match_prusaslicer() {
    let s = PrintSettings::core_one_hf04_speed_pla();
    let obj = slice_object(&model("3DBenchy.stl"), &s).unwrap();
    let all: Vec<usize> = (0..obj.layers.len()).collect();
    let total = |role| filament_mm(&obj, &s, &all, role);
    // The CORE One settings use Arachne walls (the profile default), so the reference is PrusaSlicer 2.9.6's
    // default run: top solid infill 115.5 mm, internal infill 583.9 mm. (With classic walls PrusaSlicer has
    // 115.4 / 572.8 mm; the infill area depends slightly on the wall generator.)
    check("top solid infill", total(Role::TopSolidInfill), 115.5, 0.03);
    check("internal infill", total(Role::InternalInfill), 583.9, 0.03);
    check("solid infill", total(Role::SolidInfill), 667.7, 0.03);
    check("external bridges", total(Role::BridgeInfill), 51.0552, 0.05);
    check(
        "internal bridges",
        total(Role::InternalBridgeInfill),
        168.7697,
        0.05,
    );
}
