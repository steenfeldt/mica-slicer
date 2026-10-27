// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The pictures Mica writes into saved projects (OrcaSlicer's per-plate set, PrusaSlicer's
//! thumbnail, the package thumbnail) and how they read back.

use mica_core::{mesh::TriangleMesh, threemf::*};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "mica-thumbs-test-{}-{}.3mf",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn cube() -> TriangleMesh {
    TriangleMesh::load_stl(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
    )
    .unwrap()
}

fn object(name: &str, tool: u32, x: f64, plate: u32) -> Object {
    let source = ResourceId {
        path: "3D/3dmodel.model".into(),
        id: 1,
    };
    let settings = Settings::from([("extruder".into(), tool.to_string())]);
    Object {
        name: name.into(),
        source: source.clone(),
        height_ranges: vec![],
        layer_height_profile: vec![],
        brim_points: vec![],
        settings,
        metadata: Settings::new(),
        instances: vec![Instance {
            transform: Transform([[1., 0., 0., x], [0., 1., 0., 0.], [0., 0., 1., 0.]]),
            printable: true,
            plate: Some(plate),
        }],
        parts: vec![Part {
            name: name.into(),
            kind: PartKind::Model,
            source,
            mesh: cube(),
            source_mesh: None,
            painting: Default::default(),
            transform: Transform::IDENTITY,
            settings: Settings::new(),
            metadata: Settings::new(),
        }],
    }
}

/// `plates` plates; plate `i` (one-based) holds one cube on tool `i` (the last plate empty
/// when `empty_last`). Tool 1 red, tool 2 blue, tool 3 translucent green.
fn project(plates: u32, empty_last: bool) -> Project {
    let mut p = Project {
        config: Some(RawConfig {
            path: "Metadata/project_settings.config".into(),
            text: r##"{"filament_colour":["#FF0000","#0000FF","#00FF0080"],"layer_height":"0.2"}"##
                .into(),
        }),
        ..Project::default()
    };
    for id in 1..=plates {
        let mut plate = Plate {
            id,
            name: String::new(),
            instances: vec![],
            metadata: Settings::new(),
        };
        if !(empty_last && id == plates) {
            plate.instances.push(PlateInstance {
                object: p.objects.len(),
                instance: 0,
            });
            p.objects
                .push(object(&format!("cube {id}"), id, f64::from(id) * 300., id));
        }
        p.plates.push(plate);
    }
    p
}

struct Archive(zip::ZipArchive<std::fs::File>);
impl Archive {
    fn open(path: &Path) -> Self {
        Self(zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap())
    }
    fn names(&self) -> Vec<String> {
        self.0.file_names().map(str::to_string).collect()
    }
    fn bytes(&mut self, name: &str) -> Vec<u8> {
        let mut out = vec![];
        self.0
            .by_name(name)
            .unwrap_or_else(|_| panic!("{name} missing"))
            .read_to_end(&mut out)
            .unwrap();
        out
    }
    fn text(&mut self, name: &str) -> String {
        String::from_utf8(self.bytes(name)).unwrap()
    }
    fn stored(&mut self, name: &str) -> bool {
        self.0.by_name(name).unwrap().compression() == zip::CompressionMethod::Stored
    }
    /// The PNG `name`, decoded: width, height, RGBA pixels.
    fn png(&mut self, name: &str) -> (u32, u32, Vec<u8>) {
        let bytes = self.bytes(name);
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba, "{name}");
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }
}

fn pixels(rgba: &[u8]) -> impl Iterator<Item = [u8; 4]> + '_ {
    rgba.as_chunks::<4>().0.iter().copied()
}

#[test]
fn orca_flavour_writes_every_plates_pictures() {
    let p = project(3, true);
    let temp = Temp::new();
    write(&temp.0, &p, Flavour::Orca).unwrap();
    let mut z = Archive::open(&temp.0);
    let names = z.names();
    for n in 1..=3 {
        for (name, size) in [
            (format!("Metadata/plate_{n}.png"), 512),
            (format!("Metadata/plate_{n}_small.png"), 128),
            (format!("Metadata/plate_no_light_{n}.png"), 512),
        ] {
            assert!(names.contains(&name), "{name} in {names:?}");
            assert!(z.stored(&name), "{name} is stored, as Orca adds PNGs");
            let (w, h, _) = z.png(&name);
            assert_eq!((w, h), (size, size), "{name}");
        }
    }
    assert!(
        !names
            .iter()
            .any(|n| n.contains("plate_4") || n.contains("top_") || n.contains("pick_"))
    );
    // The package thumbnail is plate 1's picture.
    assert_eq!(
        z.bytes("Metadata/thumbnail.png"),
        z.bytes("Metadata/plate_1.png")
    );
    let types = z.text("[Content_Types].xml");
    assert!(types.contains(r#"<Default Extension="png" ContentType="image/png"/>"#));
    let rels = z.text("_rels/.rels");
    for (target, kind) in [
        (
            "/3D/3dmodel.model",
            "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel",
        ),
        (
            "/Metadata/thumbnail.png",
            "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail",
        ),
        (
            "/Metadata/plate_1.png",
            "http://schemas.bambulab.com/package/2021/cover-thumbnail-middle",
        ),
        (
            "/Metadata/plate_1_small.png",
            "http://schemas.bambulab.com/package/2021/cover-thumbnail-small",
        ),
    ] {
        assert!(
            rels.contains(&format!("Target=\"{target}\""))
                && rels.contains(&format!("Type=\"{kind}\"")),
            "{target} {kind} in {rels}"
        );
    }
    let settings = z.text("Metadata/model_settings.config");
    for n in 1..=3 {
        assert!(settings.contains(&format!(
            r#"key="thumbnail_file" value="Metadata/plate_{n}.png""#
        )));
        assert!(settings.contains(&format!(
            r#"key="thumbnail_no_light_file" value="Metadata/plate_no_light_{n}.png""#
        )));
    }

    // Plate 1: the red cube (tool 1), lit; unlit in its filament colour with alpha 255 - 0.
    let (_, _, lit) = z.png("Metadata/plate_1.png");
    let opaque: Vec<_> = pixels(&lit).filter(|p| p[3] == 255).collect();
    assert!(opaque.len() > 512 * 512 / 10, "the cube fills the picture");
    assert!(opaque.iter().all(|p| p[0] > p[1] && p[0] > p[2]), "red");
    let (_, _, flat) = z.png("Metadata/plate_no_light_1.png");
    assert!(pixels(&flat).any(|p| p == [255, 0, 0, 255]));
    // Plate 2: blue, tool 2 encoded as alpha 254.
    let (_, _, flat) = z.png("Metadata/plate_no_light_2.png");
    assert!(pixels(&flat).any(|p| p == [0, 0, 255, 254]));
    assert!(!pixels(&flat).any(|p| p[0] > 128));
    // The empty last plate: a transparent picture, as Orca writes one.
    let (_, _, empty) = z.png("Metadata/plate_3.png");
    assert!(empty.iter().all(|&v| v == 0));
}

#[test]
fn translucent_filament_draws_translucent() {
    // Tool 3 is #00FF0080: the cube on plate 3 shows at about half opacity.
    let p = project(3, false);
    let temp = Temp::new();
    write(&temp.0, &p, Flavour::Orca).unwrap();
    let mut z = Archive::open(&temp.0);
    let (_, _, lit) = z.png("Metadata/plate_3.png");
    let covered: Vec<_> = pixels(&lit).filter(|p| p[3] > 0).collect();
    assert!(covered.len() > 512 * 512 / 10);
    assert!(covered.iter().all(|p| p[3] < 200), "translucent");
    assert!(covered.iter().any(|p| (120..=136).contains(&p[3])));
    assert!(
        covered.iter().all(|p| p[1] >= p[0] && p[1] >= p[2]),
        "green"
    );
    // The unlit picture keeps Orca's extruder code (tool 3: 253), not the filament alpha.
    let (_, _, flat) = z.png("Metadata/plate_no_light_3.png");
    assert!(pixels(&flat).any(|p| p == [0, 255, 0, 253]));
}

#[test]
fn prusa_flavour_writes_its_thumbnail() {
    let p = project(2, false);
    let temp = Temp::new();
    write(&temp.0, &p, Flavour::Prusa).unwrap();
    let mut z = Archive::open(&temp.0);
    let names = z.names();
    assert!(!names.iter().any(|n| n.contains("plate_")), "{names:?}");
    let (w, h, lit) = z.png("Metadata/thumbnail.png");
    assert_eq!((w, h), (256, 256));
    // The first bed: the red cube only.
    let opaque: Vec<_> = pixels(&lit).filter(|p| p[3] == 255).collect();
    assert!(opaque.len() > 256 * 256 / 10);
    assert!(
        opaque.iter().all(|p| p[0] > p[2]),
        "red, not the second bed's blue"
    );
    let rels = z.text("_rels/.rels");
    assert!(rels.contains(
        r#"Target="/Metadata/thumbnail.png" Id="rel1" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail""#
    ));
    assert!(!rels.contains("bambulab"));
    assert!(z.text("[Content_Types].xml").contains(r#"Extension="png""#));
}

#[test]
fn mica_saves_reopen_without_skipped_pictures() {
    for flavour in [Flavour::Orca, Flavour::Prusa] {
        let mut p = project(2, false);
        // An older Mica save carried the source project's pictures as skipped.
        p.ignored_features.insert("thumbnails/images".into());
        let temp = Temp::new();
        write(&temp.0, &p, flavour).unwrap();
        let again = load_3mf(&temp.0).unwrap();
        assert!(
            again.ignored_features.is_empty(),
            "{:?}",
            again.ignored_features
        );
        // The native archive alone (no Mica sidecar) reads without a picture notice too.
        let mut z = Archive::open(&temp.0);
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for name in z.names() {
            if name == "Metadata/mica.json" {
                continue;
            }
            let data = z.bytes(&name);
            out.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut out, &data).unwrap();
        }
        let bytes = out.finish().unwrap().into_inner();
        let native = read_3mf(std::io::Cursor::new(bytes)).unwrap();
        assert!(
            native.ignored_features.is_empty(),
            "{:?}",
            native.ignored_features
        );
    }
}

#[test]
fn colour_painting_shows_in_the_pictures() {
    // The red cube (tool 1) painted all over with tool 2 (blue): both flavours draw it blue.
    let mut p = project(1, false);
    let part = &mut p.objects[0].parts[0];
    for face in 0..part.mesh.faces.len() as u32 {
        part.painting.material.push(face, "8").unwrap();
    }
    for flavour in [Flavour::Orca, Flavour::Prusa] {
        let temp = Temp::new();
        write(&temp.0, &p, flavour).unwrap();
        let mut z = Archive::open(&temp.0);
        let (_, _, lit) = z.png("Metadata/thumbnail.png");
        let opaque: Vec<_> = pixels(&lit).filter(|p| p[3] == 255).collect();
        assert!(!opaque.is_empty());
        assert!(opaque.iter().all(|p| p[2] > p[0]), "{flavour:?}: blue");
    }
}
