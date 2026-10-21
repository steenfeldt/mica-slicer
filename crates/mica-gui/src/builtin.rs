// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Built-in models on the plate ("Add shape"): the 3DBenchy and the basic shapes of
//! `mica_core::builtin_models`, added as a new object at a free spot of the current plate, or
//! to the selected object as a part, modifier, negative volume or support volume. One undo
//! step each ("Add cube", "Add cube modifier"). Saved projects keep them as plain mesh
//! objects named after the model ("3DBenchy", "Cube"), so every slicer opens them.
//!
//! The gallery's thumbnails are rendered once, off the UI thread, with the G-code thumbnail
//! renderer, and kept for the session.

use crate::app::App;
use crate::scene::{self, Sub, Volume};
use eframe::egui;
use glam::{Mat4, Vec2, Vec3};
use mica_core::builtin_models::BuiltinModel;
use mica_core::threemf::PartKind;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// How a built-in model joins the plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum AddAs {
    /// An object of its own.
    #[default]
    Object,
    /// A model part of the selected object.
    Part,
    Modifier,
    Negative,
    Enforcer,
    Blocker,
}

impl AddAs {
    pub const ALL: [Self; 6] = [
        Self::Object,
        Self::Part,
        Self::Modifier,
        Self::Negative,
        Self::Enforcer,
        Self::Blocker,
    ];

    /// The gallery's short label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Object => "Object",
            Self::Part => "Part",
            Self::Modifier => "Modifier",
            Self::Negative => "Negative",
            Self::Enforcer => "Enforcer",
            Self::Blocker => "Blocker",
        }
    }

    /// What it does (tooltips) and how undo names it.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Object => "A new object on the plate",
            Self::Part => "A printed part of the selected object",
            Self::Modifier => "Its own settings inside the shape (starts with 100% infill)",
            Self::Negative => "A hole in the shape of it",
            Self::Enforcer => "Supports wherever the shape reaches",
            Self::Blocker => "No supports inside the shape",
        }
    }

    pub fn part_kind(self) -> Option<PartKind> {
        Some(match self {
            Self::Object => return None,
            Self::Part => PartKind::Model,
            Self::Modifier => PartKind::Modifier,
            Self::Negative => PartKind::NegativeVolume,
            Self::Enforcer => PartKind::SupportEnforcer,
            Self::Blocker => PartKind::SupportBlocker,
        })
    }
}

/// The undo step's name: `Add cube`, `Add 3DBenchy`, `Add cube modifier`.
pub fn undo_label(model: BuiltinModel, add_as: AddAs) -> String {
    let name = match model {
        BuiltinModel::Benchy => model.name().to_string(),
        m => m.name().to_lowercase(),
    };
    match add_as {
        AddAs::Object => format!("Add {name}"),
        AddAs::Part => format!("Add {name} part"),
        AddAs::Modifier => format!("Add {name} modifier"),
        AddAs::Negative => format!("Add {name} negative volume"),
        AddAs::Enforcer => format!("Add {name} support enforcer"),
        AddAs::Blocker => format!("Add {name} support blocker"),
    }
}

/// Where and how big (both optional: the gallery uses the defaults; Mica MCP may set them).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placement {
    /// Bounding box size, mm.
    pub size: Option<[f32; 3]>,
    /// Bed XY of the new object's center (objects only); default a free spot.
    pub at: Option<Vec2>,
}

impl App {
    /// Adds `model` as `add_as` (one undo step, `label` or [`undo_label`]); the new object or
    /// part ends up selected. Errors say why nothing was added.
    pub(crate) fn add_builtin_with(
        &mut self,
        model: BuiltinModel,
        add_as: AddAs,
        placement: Placement,
        label: Option<&str>,
    ) -> Result<String, String> {
        if self.plates.current().locked {
            return Err("this plate is locked; unlock it from its plate label".into());
        }
        let target = match add_as.part_kind() {
            Some(_) => Some(self.selected.ok_or(
                "select an object first: a part, modifier or support volume belongs to one",
            )?),
            None => None,
        };
        let size = placement.size.unwrap_or_else(|| model.default_size());
        let mesh = model.mesh_sized(size)?;
        let label = label.map_or_else(|| undo_label(model, add_as), str::to_string);
        match (target, add_as.part_kind()) {
            (Some(i), Some(kind)) => Ok(self.add_builtin_volume(i, model, kind, mesh, &label)),
            _ => Ok(self.add_builtin_object(model, mesh, placement.at, &label)),
        }
    }

    /// The gallery's click: the model at its default size, errors shown to the user.
    pub(crate) fn add_builtin(&mut self, model: BuiltinModel, add_as: AddAs) {
        if let Err(e) = self.add_builtin_with(model, add_as, Placement::default(), None) {
            self.notify_error(format!("{} was not added", model.name()), e);
        }
    }

    fn add_builtin_object(
        &mut self,
        model: BuiltinModel,
        mesh: mica_core::mesh::TriangleMesh,
        at: Option<Vec2>,
        label: &str,
    ) -> String {
        self.record(label);
        let mut o = self.plain_object(self.unique_name(model.name()), mesh);
        let (a, b) = o.bounds();
        let size = (b - a).truncate();
        let objects = &self.plates.current().objects;
        let corner = match at {
            Some(c) => c - size * 0.5,
            // An empty plate: the middle; else the nearest free spot around it.
            None if objects.is_empty() => self.bed.center().truncate() - size * 0.5,
            None => scene::free_spot(
                objects,
                size,
                self.bed.center().truncate() - size * 0.5,
                &self.bed,
                6.,
            ),
        };
        o.position += (corner - a.truncate()).extend(0.);
        o.drop_to_bed();
        let name = o.name.clone();
        self.plates.current_mut().objects.push(o);
        let i = self.plates.current().objects.len() - 1;
        self.select(Some(i), false);
        self.changed();
        name
    }

    /// A volume of `kind` on object `i`, at its true size whatever the object's scale: a part
    /// stands on the bed beside the object (it joins the object's print), every other kind
    /// sits in the object's middle.
    fn add_builtin_volume(
        &mut self,
        i: usize,
        model: BuiltinModel,
        kind: PartKind,
        mut mesh: mica_core::mesh::TriangleMesh,
        label: &str,
    ) -> String {
        let (lo, hi) = mesh.bounds();
        let size = Vec3::from(hi) - Vec3::from(lo);
        // Centred on its own origin, which is where the volume's position puts it.
        let mid = (Vec3::from(lo) + Vec3::from(hi)) * 0.5;
        for v in &mut mesh.vertices {
            *v = (Vec3::from(*v) - mid).to_array();
        }
        let o = &self.plates.current().objects[i];
        let (min, max) = o.bounds();
        let world = if kind == PartKind::Model {
            Vec3::new(
                max.x + size.x * 0.5,
                (min.y + max.y) * 0.5,
                min.z + size.z * 0.5,
            )
        } else {
            (min + max) * 0.5
        };
        let local = o.transform().inverse() * Mat4::from_translation(world);
        let (scale, rotation, position) = local.to_scale_rotation_translation();
        let mut v = Volume::new(model.name(), kind.clone(), mesh);
        v.position = position;
        v.rotation = rotation;
        v.scale = scale;
        if kind == PartKind::Modifier {
            // A modifier without settings does nothing; start with dense infill.
            v.overrides.insert("fill_density".into(), "100%".into());
        }
        let id = v.id;
        self.record(label);
        self.plates.current_mut().objects[i].volumes.push(v);
        self.sync_instances(i);
        self.sub = Some(Sub::Volume(id));
        self.editor.object_mode = true;
        self.changed();
        model.name().to_string()
    }
}

// ----- thumbnails ------------------------------------------------------------------------

/// Rendered size (shown at about half for sharp edges on high-density screens).
const THUMB_PX: u16 = 112;
/// Mica's violet, as the library's thumbnails.
const THUMB_COLOR: [u8; 3] = [0x8b, 0x7c, 0xf6];

#[derive(Default)]
struct Thumbs {
    started: bool,
    images: HashMap<BuiltinModel, egui::ColorImage>,
    textures: HashMap<BuiltinModel, egui::TextureHandle>,
}

fn thumbs() -> &'static Mutex<Thumbs> {
    static T: OnceLock<Mutex<Thumbs>> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn render_thumbnail(model: BuiltinModel) -> Option<egui::ColorImage> {
    let mesh = model.mesh().ok()?;
    let img = mica_gcode::thumbnails::render(
        &mesh,
        THUMB_PX,
        THUMB_PX,
        THUMB_COLOR,
        mica_gcode::thumbnails::Camera::Prusa,
    )
    .ok()?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [usize::from(img.width), usize::from(img.height)],
        &img.rgba,
    ))
}

/// `model`'s gallery picture; `None` while it is being rendered (the first call starts all
/// of them on a thread, which wakes the window as each one is ready).
pub fn thumbnail(ctx: &egui::Context, model: BuiltinModel) -> Option<egui::TextureHandle> {
    let mut t = thumbs().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tex) = t.textures.get(&model) {
        return Some(tex.clone());
    }
    if let Some(img) = t.images.remove(&model) {
        let tex = ctx.load_texture(
            format!("mica-shape-{}", model.id()),
            img,
            egui::TextureOptions::LINEAR,
        );
        t.textures.insert(model, tex.clone());
        return Some(tex);
    }
    if !t.started {
        t.started = true;
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("mica-shape-thumbnails".into())
            .spawn(move || {
                // All at once: the 3DBenchy (an 11 MB mesh) takes longest.
                std::thread::scope(|s| {
                    for m in BuiltinModel::ALL {
                        let wake = &wake;
                        s.spawn(move || {
                            if let Some(img) = render_thumbnail(m) {
                                thumbs()
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .images
                                    .insert(m, img);
                                wake.request_repaint();
                            }
                        });
                    }
                });
            });
        if spawned.is_err() {
            // No thread: render here, one per call.
            t.started = false;
            drop(t);
            let img = render_thumbnail(model)?;
            let tex = ctx.load_texture(
                format!("mica-shape-{}", model.id()),
                img,
                egui::TextureOptions::LINEAR,
            );
            thumbs()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .textures
                .insert(model, tex.clone());
            return Some(tex);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Options};
    use mica_profiles::user::Store;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

    fn app(models: &[&str]) -> App {
        let dir = std::env::temp_dir().join(format!(
            "mica-builtin-test-{}-{}",
            std::process::id(),
            NEXT_STORE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        App::create(
            &egui::Context::default(),
            Options {
                prefs_path: None,
                store: Some(Store::open(dir)),
                files: models
                    .iter()
                    .map(|m| {
                        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/"))
                            .join(m)
                    })
                    .collect(),
                headless: true,
            },
        )
    }

    fn app_with_cube() -> App {
        let mut app = app(&["cube20.stl"]);
        app.select(None, false);
        app
    }

    #[test]
    fn a_shape_lands_on_an_empty_plate_in_the_middle_as_one_undo_step() {
        let mut app = app(&[]);
        app.add_builtin(BuiltinModel::Cube, AddAs::Object);
        let objects = &app.plates.current().objects;
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].name, "Cube");
        assert_eq!(app.selected, Some(0));
        let (a, b) = objects[0].bounds();
        assert!((b - a - Vec3::splat(20.)).abs().max_element() < 1e-3);
        assert!(a.z.abs() < 1e-4);
        let mid = (a + b).truncate() * 0.5;
        assert!((mid - app.bed.center().truncate()).length() < 1e-3);
        assert_eq!(app.undo.undo_label(), Some("Add cube"));
        app.undo();
        assert!(app.plates.current().objects.is_empty());
    }

    #[test]
    fn a_second_shape_finds_a_free_spot() {
        let mut app = app(&[]);
        app.add_builtin(BuiltinModel::Cube, AddAs::Object);
        app.add_builtin(BuiltinModel::Cylinder, AddAs::Object);
        app.add_builtin(BuiltinModel::Cube, AddAs::Object);
        let objects = &app.plates.current().objects;
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[2].name, "Cube (2)");
        for i in 0..3 {
            for j in i + 1..3 {
                let (a0, a1) = objects[i].bounds();
                let (b0, b1) = objects[j].bounds();
                let apart = a1.x <= b0.x || b1.x <= a0.x || a1.y <= b0.y || b1.y <= a0.y;
                assert!(apart, "{i} and {j} overlap");
            }
        }
        assert_eq!(app.selected, Some(2));
    }

    #[test]
    fn shapes_join_the_selected_object_as_every_part_kind() {
        let mut app = app_with_cube();
        assert!(
            app.add_builtin_with(
                BuiltinModel::Sphere,
                AddAs::Modifier,
                Placement::default(),
                None
            )
            .is_err(),
            "nothing selected"
        );
        app.select(Some(0), false);
        for add_as in AddAs::ALL.into_iter().skip(1) {
            app.add_builtin(BuiltinModel::Sphere, add_as);
        }
        let o = &app.plates.current().objects[0];
        let kinds: Vec<PartKind> = o.volumes.iter().map(|v| v.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                PartKind::Model,
                PartKind::Modifier,
                PartKind::NegativeVolume,
                PartKind::SupportEnforcer,
                PartKind::SupportBlocker
            ]
        );
        assert_eq!(o.volumes[1].overrides.get("fill_density").unwrap(), "100%");
        assert_eq!(app.undo.undo_label(), Some("Add sphere support blocker"));
        // The modifier sits in the cube's middle, the part beside it on the bed.
        let world = |k: usize| {
            let v = &o.volumes[k];
            (o.transform() * v.local_transform()).transform_point3(Vec3::ZERO)
        };
        let cube_mid = {
            let (a, b) = (Vec3::from(o.local_bounds.0), Vec3::from(o.local_bounds.1));
            o.transform().transform_point3((a + b) * 0.5)
        };
        assert!((world(1) - cube_mid).length() < 1e-3);
        let part = world(0);
        assert!((part.z - 10.).abs() < 1e-3, "{part}");
        assert!(part.x > cube_mid.x + 19.);
        assert_eq!(app.sub, Some(Sub::Volume(o.volumes[4].id)));
    }

    #[test]
    fn a_scaled_object_gets_its_part_at_true_size() {
        let mut app = app_with_cube();
        app.plates.current_mut().objects[0].scale = Vec3::splat(2.);
        app.select(Some(0), false);
        app.add_builtin(BuiltinModel::Cube, AddAs::Negative);
        let o = &app.plates.current().objects[0];
        let v = &o.volumes[0];
        let t = o.transform() * v.local_transform();
        let (lo, hi) = (Vec3::from(v.local_bounds.0), Vec3::from(v.local_bounds.1));
        let size = t.transform_point3(hi) - t.transform_point3(lo);
        assert!(
            (size - Vec3::splat(20.)).abs().max_element() < 1e-3,
            "{size}"
        );
    }

    #[test]
    fn saved_projects_keep_them_as_plain_named_meshes() {
        let mut app = app(&[]);
        app.add_builtin(BuiltinModel::Benchy, AddAs::Object);
        app.add_builtin(BuiltinModel::Cube, AddAs::Object);
        app.add_builtin(BuiltinModel::Torus, AddAs::Modifier);
        for flavour in [
            mica_core::threemf::Flavour::Prusa,
            mica_core::threemf::Flavour::Orca,
        ] {
            let project = app.build_project(flavour).unwrap();
            let names: Vec<&str> = project.objects.iter().map(|o| o.name.as_str()).collect();
            assert_eq!(names, ["3DBenchy", "Cube"], "{flavour:?}");
            let kinds: Vec<PartKind> = project.objects[1]
                .parts
                .iter()
                .map(|p| p.kind.clone())
                .collect();
            assert_eq!(kinds, [PartKind::Model, PartKind::Modifier], "{flavour:?}");
            assert!(project.objects[0].parts[0].mesh.faces.len() > 225_000);
        }
    }

    #[test]
    fn a_locked_plate_takes_no_shape() {
        let mut app = app(&[]);
        app.plates.current_mut().locked = true;
        app.add_builtin(BuiltinModel::Cube, AddAs::Object);
        assert!(app.plates.current().objects.is_empty());
    }

    #[test]
    fn undo_steps_are_named_after_the_model() {
        assert_eq!(
            undo_label(BuiltinModel::Benchy, AddAs::Object),
            "Add 3DBenchy"
        );
        assert_eq!(undo_label(BuiltinModel::Cube, AddAs::Object), "Add cube");
    }
}
