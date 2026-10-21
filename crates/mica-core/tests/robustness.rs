// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
//! Deterministic malformed-input corpus, including a byte-mutation sweep. No fuzz dependency.
use mica_core::{
    config::Config, mesh::TriangleMesh, print::try_prepare_object, settings::PrintSettings,
};
use std::io::{Cursor, Write};
fn cube() -> TriangleMesh {
    TriangleMesh::load_stl(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/models/cube20.stl"
    ))
    .unwrap()
}
fn settings() -> PrintSettings {
    PrintSettings::core_one_hf04_speed_pla()
}
fn archive(xml: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}
#[test]
fn invalid_face_index_is_an_error_before_indexing() {
    let mut mesh = cube();
    mesh.faces[0][0] = u32::MAX;
    assert!(try_prepare_object(&mesh, &settings()).is_err());
}
#[test]
fn nonfinite_and_enormous_coordinates() {
    for v in [f32::NAN, f32::INFINITY, 1e20] {
        let mut mesh = cube();
        mesh.vertices[0][0] = v;
        assert!(try_prepare_object(&mesh, &settings()).is_err());
    }
}
#[test]
fn open_and_nonmanifold_meshes_produce_diagnostics_without_panics() {
    for mode in 0..2 {
        let mut mesh = cube();
        if mode == 0 {
            mesh.faces.remove(0);
        } else {
            mesh.faces.push(mesh.faces[0]);
        }
        mesh.place_on_bed(100., 100.);
        assert!(
            !mica_core::safety::placement(&mesh, &Config::defaults())
                .unwrap()
                .is_empty()
        );
        assert!(std::panic::catch_unwind(|| try_prepare_object(&mesh, &settings())).is_ok());
    }
}
#[test]
fn intersecting_shells_are_sliced_without_panicking() {
    let mut a = cube();
    let mut b = cube();
    b.translate([5., 5., 5.]);
    let base = a.vertices.len() as u32;
    a.vertices.extend(b.vertices);
    a.faces
        .extend(b.faces.into_iter().map(|f| f.map(|i| i + base)));
    a.place_on_bed(100., 100.);
    assert!(
        mica_core::safety::placement(&a, &Config::defaults())
            .unwrap()
            .iter()
            .any(|w| w.contains("self-intersection"))
    );
    let p = try_prepare_object(&a, &settings()).unwrap();
    assert!(p.report.layers > 0);
}
#[test]
fn degenerate_zero_volume_and_empty_meshes_are_errors() {
    let mut flat = cube();
    for v in &mut flat.vertices {
        v[2] = 0.;
    }
    let degenerate = TriangleMesh {
        vertices: vec![[0., 0., 0.], [1., 1., 1.], [2., 2., 2.]],
        faces: vec![[0, 1, 2]],
    };
    for m in [flat, degenerate, TriangleMesh::default()] {
        assert!(try_prepare_object(&m, &settings()).is_err());
    }
}
#[test]
fn enormous_tiny_and_below_bed_models_are_diagnosed() {
    let c = Config::defaults();
    let mut big = cube();
    for v in &mut big.vertices {
        v[0] *= 100.;
    }
    assert!(mica_core::safety::placement(&big, &c).is_err());
    let mut small = cube();
    for v in &mut small.vertices {
        for x in v {
            *x *= 0.02;
        }
    }
    small.place_on_bed(100., 100.);
    assert!(
        mica_core::safety::placement(&small, &c)
            .unwrap()
            .iter()
            .any(|w| w.contains("1 mm"))
    );
    assert!(std::panic::catch_unwind(|| try_prepare_object(&small, &settings())).is_ok());
    let mut below = cube();
    below.translate([0., 0., -100.]);
    assert!(mica_core::safety::placement(&below, &c).is_err());
    // GUI monkey test (seed 16): a 1 km XY contour compensation made every layer kilometres
    // wide; the job ran for minutes past its cancel before finding nothing to print. A
    // compensation that grows the model past the bed is refused up front.
    let mut centred = cube();
    centred.place_on_bed(100., 100.);
    for (key, value, ok) in [
        ("xy_contour_compensation", "1000000", false),
        ("xy_size_compensation", "1000", false),
        ("xy_contour_compensation", "0.2", true),
        ("xy_contour_compensation", "-1000000", true),
    ] {
        let mut grown = c.clone();
        grown.set_raw(key, value).unwrap();
        let result = mica_core::safety::placement(&centred, &grown);
        assert_eq!(result.is_ok(), ok, "{key} = {value}: {result:?}");
    }
}
#[test]
fn malformed_and_empty_3mf_xml_are_errors() {
    let empty = zip::ZipWriter::new(Cursor::new(Vec::new()))
        .finish()
        .unwrap()
        .into_inner();
    assert!(mica_core::threemf::read_3mf(Cursor::new(empty)).is_err());
    for xml in [
        "",
        "<model>",
        "<model><resources></model>",
        "<model><resources/><build/></model>",
        "<!DOCTYPE x [<!ENTITY a 'boom'>]><model>&a;</model>",
    ] {
        assert!(
            mica_core::threemf::read_3mf(Cursor::new(archive(xml))).is_err(),
            "{xml}"
        );
    }
}
#[test]
fn bad_painting_strings_are_errors() {
    for text in [
        "garbage",
        "F",
        "1",
        "é",
        "111111111111111111111111111111111111111111111111111111",
    ] {
        let mut painting = mica_core::painting::FacetStates::default();
        assert!(painting.push(0, text).is_err(), "{text}");
    }
}
#[test]
fn contradictory_and_nonfinite_settings_are_diagnosed() {
    let mut s = settings();
    s.layer_height = 1.;
    assert!(try_prepare_object(&cube(), &s).is_err());
    s = settings();
    s.layer_height = 0.;
    assert!(try_prepare_object(&cube(), &s).is_err());
    // GUI monkey test: a 0.0001 mm first layer was sliced and written as an empty layer at
    // `;Z:0` (Z has three decimals in G-code), which `mica validate` rejects.
    s = settings();
    s.first_layer_height = 0.0001;
    let error = try_prepare_object(&cube(), &s).err().expect("refused");
    assert!(
        format!("{error:?}").contains("0.01 mm minimum"),
        "{error:?}"
    );
    s = settings();
    s.layer_height = 0.01;
    s.first_layer_height = 0.01;
    assert!(mica_core::safety::settings(&s).is_ok());
    // GUI monkey test (seed 15): a 17.24 mm first-layer width on a 0.4 mm nozzle printed a
    // raft base whose single moves extruded 266 mm of filament. Widths above OrcaSlicer's
    // 5× nozzle bound are refused, the first-layer and support widths included.
    s = settings();
    s.first_layer_extrusion_width = 17.24;
    s.support_material_first_layer_extrusion_width = 17.24;
    let error = try_prepare_object(&cube(), &s).err().expect("refused");
    assert!(
        format!("{error:?}").contains("cannot be extruded"),
        "{error:?}"
    );
    s = settings();
    s.support_material_extrusion_width = s.nozzle_diameter * 5.01;
    assert!(mica_core::safety::settings(&s).is_err());
    s.support_material_extrusion_width = s.nozzle_diameter * 5.;
    assert!(mica_core::safety::settings(&s).is_ok());
    // Seed 14: a 100 mm Orca bridge line width. Orca refuses bridges wider than the nozzle.
    use mica_core::settings::FloatOrPercent;
    s = settings();
    s.orca_walls.bridge_line_width = Some(FloatOrPercent::Mm(100.));
    assert!(mica_core::safety::settings(&s).is_err());
    s.orca_walls.bridge_line_width = Some(FloatOrPercent::Percent(100.));
    assert!(mica_core::safety::settings(&s).is_ok());
    let mut c = Config::defaults();
    assert!(c.set_raw("layer_height", "NaN").is_err());
    c.set_raw("nozzle_diameter", "0").unwrap();
    assert!(PrintSettings::from_config(&c).is_err());
}
#[test]
fn mutated_stl_never_panics() {
    let source = include_bytes!("../../../tests/models/cube20.stl");
    for i in (0..source.len()).step_by(17) {
        let mut bytes = source.to_vec();
        bytes[i] ^= 0xff;
        assert!(
            std::panic::catch_unwind(|| TriangleMesh::from_stl_bytes(&bytes)).is_ok(),
            "byte {i}"
        );
    }
}

#[test]
fn zero_walls_and_infill_are_warned() {
    let mut c = Config::defaults();
    c.set_raw("perimeters", "0").unwrap();
    c.set_raw("fill_density", "0%").unwrap();
    assert!(!mica_core::safety::config_warnings(&c).is_empty());
    assert!(
        std::panic::catch_unwind(|| try_prepare_object(
            &cube(),
            &PrintSettings::from_config(&c).unwrap()
        ))
        .is_ok()
    );
}

#[test]
fn facet_offset_overflow_is_a_clear_error() {
    let mut a = mica_core::painting::FacetStates::default();
    let mut b = mica_core::painting::FacetStates::default();
    b.push(1, "4").unwrap();
    assert!(a.append_shifted(&b, u32::MAX).is_err());
}
#[test]
fn public_preparation_helpers_return_errors() {
    assert!(mica_core::print::prepare_object(&TriangleMesh::default(), &settings()).is_err());
    assert!(mica_core::print::slice_object(&TriangleMesh::default(), &settings()).is_err());
}
