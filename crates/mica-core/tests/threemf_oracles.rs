// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Read-only 3MF oracles. No user file is copied, extracted or modified.
//!
//! The pinned Snapmaker Orca Benchy runs everywhere the upstream data is fetched. The checks
//! against the PrusaSlicer 2.9.6 reference app (`out/ref`, or `MICA_PRUSASLICER` +
//! `MICA_PRUSA_DATADIR`) and against private projects are `#[ignore]`d: run them with
//! `cargo test -p mica-core --test threemf_oracles -- --ignored`, where a missing app or file
//! fails instead of passing.
use mica_core::mesh::TriangleMesh;
use mica_core::threemf::{PartKind, Project, load_3mf};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The PrusaSlicer 2.9.6 reference executable and its data folder.
fn prusa() -> (PathBuf, PathBuf) {
    let reference = mica_testkit::root().join("out/ref");
    let app = std::env::var_os("MICA_PRUSASLICER").map_or_else(
        || reference.join("PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer"),
        PathBuf::from,
    );
    let data = std::env::var_os("MICA_PRUSA_DATADIR")
        .map_or_else(|| reference.join("datadir"), PathBuf::from);
    assert!(
        app.is_file() && data.is_dir(),
        "the PrusaSlicer 2.9.6 reference app is missing ({} / {}): set MICA_PRUSASLICER and \
         MICA_PRUSA_DATADIR",
        app.display(),
        data.display()
    );
    (app, data)
}

fn compare_prusa(path: &Path, project: &Project) {
    let (app, data) = prusa();
    let output = Command::new(app)
        .arg("--datadir")
        .arg(data)
        .arg("--info")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let mut expected = Vec::new();
    let mut facets = 0;
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("number_of_facets = ") {
            facets = n.parse::<usize>().unwrap();
        }
        if let Some(v) = line.strip_prefix("volume = ") {
            expected.push((facets, v.parse::<f64>().unwrap()));
        }
    }
    // Prusa's generic Orca reader exposes modifier meshes as separate objects. Compare all
    // imported parts here; the slicing helper is separately checked to exclude modifiers.
    let mut actual = Vec::new();
    for o in &project.objects {
        for instance in &o.instances {
            for part in &o.parts {
                actual.push((
                    part.mesh.faces.len(),
                    part.mesh.volume() * instance.transform.determinant().abs(),
                ));
            }
        }
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    actual.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    assert_eq!(expected.len(), actual.len(), "{}", path.display());
    for ((ef, ev), (af, av)) in expected.iter().zip(&actual) {
        assert_eq!(ef, af, "{}", path.display());
        // Prusa accumulates volume in float, Mica in double; the open dense painted mesh
        // also has 81 open edges according to Prusa. Allow 0.003%, never geometry repair.
        assert!(
            (ev - av).abs() <= ev.abs().max(1.0) * 3e-5,
            "{}: Prusa={ev} Mica={av}",
            path.display()
        );
    }
}

#[test]
#[ignore = "needs private models and the PrusaSlicer reference app: set MICA_PRIVATE_MODELS, cargo test -- --ignored"]
fn user_projects() {
    // Relative to MICA_PRIVATE_MODELS (default: ~/models/private): private U1 projects, a
    // densely painted single object, a 20-object 5-plate project, an object with a modifier
    // and a 5-object 5-plate project.
    let files = [
        ("dense-painted.3mf", 1, 1, 1),
        ("cat.3mf", 20, 20, 5),
        ("holder.3mf", 1, 2, 1),
        ("five-plates.3mf", 5, 5, 5),
    ];
    for (file, objects, parts, plates) in files {
        let path = mica_testkit::private_file(file);
        let p = load_3mf(&path).unwrap();
        assert_eq!(p.objects.len(), objects);
        assert_eq!(
            p.objects.iter().map(|o| o.parts.len()).sum::<usize>(),
            parts
        );
        assert_eq!(p.plates.len(), plates);
        assert!(p.config.is_some());
        if file == "holder.3mf" {
            assert_eq!(p.objects[0].parts[1].kind, PartKind::Modifier);
            assert_eq!(
                p.objects[0].placed_model_mesh().unwrap().faces.len(),
                19_840
            );
        }
        eprintln!(
            "{file}: {objects} objects, {parts} parts, {plates} plates, model volume {:.6} mm³",
            p.placed_model_mesh().unwrap().volume()
        );
        compare_prusa(&path, &p);
    }
}

/// The pinned Snapmaker Orca Benchy 3MF; `None` (a reported skip) when it is not fetched.
fn pinned_benchy() -> Option<(PathBuf, Project)> {
    let path = mica_testkit::upstream("snapmaker-orca/resources/handy_models/3DBenchy.3mf");
    if !mica_testkit::require_fixture(
        &path,
        "Snapmaker Orca handy models: scripts/fetch-upstream.sh --profiles snapmaker-orca",
    ) {
        return None;
    }
    let p = load_3mf(&path).unwrap();
    Some((path, p))
}

#[test]
fn upstream_benchy_matches_the_stl() {
    let Some((_, p)) = pinned_benchy() else {
        return;
    };
    let mesh = p.placed_model_mesh().unwrap();
    let stl = TriangleMesh::load_stl(mica_testkit::model("3DBenchy.stl")).unwrap();
    // These two pinned source assets are different tessellations, contrary to the initial
    // oracle assumption. Keep the discrepancy explicit instead of dropping 52 triangles.
    assert_eq!(mesh.faces.len(), 225_154);
    assert_eq!(stl.faces.len(), 225_102);
    assert!((mesh.volume() - stl.volume()).abs() / stl.volume() < 0.0001);
    eprintln!(
        "Benchy: 3MF {} triangles, {:.6} mm³; STL {} triangles, {:.6} mm³",
        mesh.faces.len(),
        mesh.volume(),
        stl.faces.len(),
        stl.volume()
    );
}

#[test]
#[ignore = "needs the PrusaSlicer 2.9.6 reference app (out/ref or MICA_PRUSASLICER): cargo test -- --ignored"]
fn upstream_benchy_matches_prusaslicer_info() {
    let (path, p) = pinned_benchy().expect("the pinned Snapmaker Orca Benchy 3MF");
    compare_prusa(&path, &p);
}

#[test]
#[ignore = "needs a PrusaSlicer 3MF export: set MICA_PRUSA_3MF, cargo test -- --ignored"]
fn prusa_export() {
    let path = std::env::var_os("MICA_PRUSA_3MF").expect("set MICA_PRUSA_3MF to a 3MF file");
    let path = Path::new(&path);
    assert!(path.is_file(), "{} is missing", path.display());
    let p = load_3mf(path).unwrap();
    assert!(!p.objects.is_empty());
    compare_prusa(path, &p);
}
