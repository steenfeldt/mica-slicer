// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "+ Add" by intent: the user picks what they want ("Stronger here", "Finer layers here",
//! "Cut a hole") and Mica creates the right part (modifier, height range, support enforcer
//! or blocker, negative volume) with a descriptive name and sensible values filled in, fitted
//! around the middle of the object and selected so the gizmo can move it. One intent is one
//! undo step. Saved recipes (`{kind, shape, overrides}` with a name) live in `recipes.toml`
//! next to the preferences and come back under "Recipes ▸".

use crate::app::App;
use crate::commands::Shape;
use crate::scene::{self, HeightRange, Object, Sub, Volume};
use glam::{Mat4, Quat, Vec3};
use mica_core::threemf::PartKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What the user wants to add to the selected object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Stronger,
    Finer,
    Supports,
    NoSupports,
    Smooth,
    Hole,
    EmptyModifier,
}

impl Intent {
    /// The menu's intents above the separator, in order.
    pub const MENU: [Intent; 6] = [
        Intent::Stronger,
        Intent::Finer,
        Intent::Supports,
        Intent::NoSupports,
        Intent::Smooth,
        Intent::Hole,
    ];
    /// Menu label, also the new part's name.
    pub fn label(self) -> &'static str {
        match self {
            Intent::Stronger => "Stronger here",
            Intent::Finer => "Finer layers here",
            Intent::Supports => "Supports here",
            Intent::NoSupports => "No supports here",
            Intent::Smooth => "Smooth top here",
            Intent::Hole => "Cut a hole",
            Intent::EmptyModifier => "Empty modifier…",
        }
    }
    /// The line under the label.
    pub fn hint(self) -> &'static str {
        match self {
            Intent::Stronger => "More walls and infill inside a shape",
            Intent::Finer => "Thinner layers between two heights",
            Intent::Supports => "Support enforcer",
            Intent::NoSupports => "Support blocker",
            Intent::Smooth => "Ironing inside a shape",
            Intent::Hole => "Negative volume",
            Intent::EmptyModifier => "A modifier without settings",
        }
    }
    /// The kind of part it creates (`None`: a height range).
    pub fn kind(self) -> Option<PartKind> {
        match self {
            Intent::Stronger | Intent::Smooth | Intent::EmptyModifier => Some(PartKind::Modifier),
            Intent::Finer => None,
            Intent::Supports => Some(PartKind::SupportEnforcer),
            Intent::NoSupports => Some(PartKind::SupportBlocker),
            Intent::Hole => Some(PartKind::NegativeVolume),
        }
    }
    fn part_name(self) -> &'static str {
        match self {
            Intent::Hole => "Hole",
            Intent::EmptyModifier => "Modifier",
            other => other.label(),
        }
    }
}

/// A saved part: its kind, shape and overrides under a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub name: String,
    /// `modifier`, `negative`, `enforcer`, `blocker` or `range`.
    pub kind: String,
    /// `box`, `cylinder`, `sphere` or `slab` (unused for ranges).
    #[serde(default)]
    pub shape: String,
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
}

pub fn kind_name(kind: &PartKind) -> &'static str {
    match kind {
        PartKind::Modifier => "modifier",
        PartKind::NegativeVolume => "negative",
        PartKind::SupportEnforcer => "enforcer",
        PartKind::SupportBlocker => "blocker",
        PartKind::Model => "part",
        PartKind::Unknown(_) => "volume",
    }
}

fn kind_from(name: &str) -> Option<PartKind> {
    Some(match name {
        "modifier" => PartKind::Modifier,
        "negative" => PartKind::NegativeVolume,
        "enforcer" => PartKind::SupportEnforcer,
        "blocker" => PartKind::SupportBlocker,
        _ => return None,
    })
}

/// The saved recipes and where they are kept (`None`: in memory only, headless runs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipes {
    pub list: Vec<Recipe>,
    pub path: Option<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "recipe")]
    recipes: Vec<Recipe>,
}

impl Recipes {
    /// The recipes file next to the preferences file.
    pub fn path_for(prefs: &Path) -> PathBuf {
        prefs.with_file_name("recipes.toml")
    }
    /// Reads `path`; a missing file is no recipes, a broken one a warning.
    pub fn load(path: Option<PathBuf>) -> (Self, Option<String>) {
        let mut out = Self {
            list: vec![],
            path: path.clone(),
        };
        let Some(path) = path else {
            return (out, None);
        };
        match mica_core::shared_file::read_to_string(&path) {
            Ok(text) => match toml::from_str::<File>(&text) {
                Ok(f) => out.list = f.recipes,
                Err(e) => {
                    return (
                        out,
                        Some(format!(
                            "{}: {e}; the recipes were not loaded",
                            path.display()
                        )),
                    );
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return (out, Some(format!("{}: {e}", path.display()))),
        }
        (out, None)
    }
    /// Writes the file atomically (nothing without a path).
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string(&File {
            recipes: self.list.clone(),
        })
        .map_err(|e| e.to_string())?;
        let text = format!("# Mica Slicer part recipes.\n{text}");
        mica_core::shared_file::locked(path, || mica_core::shared_file::write_text(path, &text))
    }
}

/// A primitive of `shape` whose bounding box is exactly `size`, centred at the origin.
pub fn fitted(shape: Shape, size: Vec3) -> mica_core::mesh::TriangleMesh {
    use mica_core::mesh_edit as m;
    let size = size.max(Vec3::splat(0.5));
    let unit = match shape {
        Shape::Box | Shape::Slab => return m::cuboid(size.to_array()),
        Shape::Cylinder => m::cylinder(0.5, 1., 48),
        Shape::Sphere => m::sphere(0.5, 24, 48),
    };
    scene::transform_mesh(&unit, Mat4::from_scale(size))
}

/// The primitive a volume's mesh most likely is (for a recipe): eight corners are a box, a
/// mesh with every vertex equally far from its centre a sphere, anything else a cylinder.
pub fn guess_shape(mesh: &mica_core::mesh::TriangleMesh) -> Shape {
    if mesh.vertices.len() == 8 {
        return Shape::Box;
    }
    let (a, b) = mesh.bounds();
    let (a, b) = (Vec3::from(a), Vec3::from(b));
    let c = (a + b) * 0.5;
    let half = (b - a) * 0.5;
    let on_sphere = half.min_element() > 0.
        && mesh.vertices.iter().all(|v| {
            let d = (Vec3::from(*v) - c) / half;
            (d.length() - 1.).abs() < 0.05
        });
    if on_sphere {
        Shape::Sphere
    } else {
        Shape::Cylinder
    }
}

/// The primitive a volume's mesh is, when it is one: a box or sphere as [`guess_shape`] finds
/// them, a cylinder when every vertex is on its side or its axis. `None` for any other mesh
/// (a model dropped in from the Library).
pub fn primitive_shape(mesh: &mica_core::mesh::TriangleMesh) -> Option<Shape> {
    let shape = guess_shape(mesh);
    if shape != Shape::Cylinder {
        return Some(shape);
    }
    let (a, b) = mesh.bounds();
    let (a, b) = (Vec3::from(a), Vec3::from(b));
    let c = (a + b) * 0.5;
    let half = (b - a) * 0.5;
    let on_cylinder = half.x > 0.
        && half.y > 0.
        && mesh.vertices.iter().all(|v| {
            let d = (Vec3::from(*v) - c) / half;
            let r = d.truncate().length();
            (r - 1.).abs() < 0.05 || r < 0.05
        });
    on_cylinder.then_some(Shape::Cylinder)
}

/// The object's size in its own (mesh) frame.
fn local_size(o: &Object) -> Vec3 {
    let (a, b) = o.local_bounds;
    Vec3::from(b) - Vec3::from(a)
}

/// Where a new part of an intent goes in the object's frame: the box's centre and size,
/// 40 % of the footprint across, full height (a third at the top for a smooth top).
fn placement(o: &Object, intent: Option<Intent>) -> (Vec3, Vec3) {
    let size = local_size(o);
    let mut centre = o.center();
    let mut box_size = Vec3::new(size.x * 0.4, size.y * 0.4, size.z);
    if intent == Some(Intent::Smooth) {
        box_size.z = size.z / 3.;
        centre.z += size.z / 3.;
    }
    if intent == Some(Intent::Hole) {
        // Through the whole object.
        box_size.z = size.z * 1.1;
    }
    (centre, box_size)
}

/// `name`, else `name (2)`, `name (3)`… unique among the object's parts.
fn unique_part_name(o: &Object, base: &str) -> String {
    let taken = |n: &str| o.volumes.iter().any(|v| v.name == n);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| base.to_string())
}

/// `0.12` from 0.12000001: two decimals, trailing zeros dropped.
fn mm(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The finer layer height for a range: 60 % of the current one, at least 0.08 mm and the
/// printer's minimum layer height.
pub fn finer_layer_height(current: f64, printer_min: f64) -> f64 {
    let v = (current * 0.6).max(0.08).max(printer_min);
    (v * 100.).round() / 100.
}

impl App {
    /// The configuration a new part of the primary object starts from: the plate's with the
    /// object's own overrides.
    fn part_base(&self, i: usize) -> Option<mica_core::config::Config> {
        let o = self.plates.current().objects.get(i)?;
        let plate = self.plate_config(self.plates.current).ok()?;
        crate::worker::object_config(&plate, o).ok()
    }

    /// Creates what `intent` asks for on the primary object (one undo step) and selects it.
    pub(crate) fn add_intent(&mut self, intent: Intent) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self.plates.current().objects.get(i).cloned() else {
            return;
        };
        let base = self.part_base(i);
        let int = |k: &str| base.as_ref().and_then(|c| c.int(k));
        let float = |k: &str| base.as_ref().and_then(|c| c.float(k));
        let mut overrides = BTreeMap::new();
        match intent {
            Intent::Stronger => {
                let walls = int("perimeters").unwrap_or(2) + 2;
                overrides.insert("perimeters".to_string(), walls.to_string());
                overrides.insert("fill_density".to_string(), "40%".to_string());
                overrides.insert("fill_pattern".to_string(), "gyroid".to_string());
            }
            Intent::Smooth => {
                overrides.insert("ironing".to_string(), "1".to_string());
            }
            Intent::Finer => {
                let current = float("layer_height").unwrap_or(0.2);
                let min = base
                    .as_ref()
                    .and_then(|c| c.float_at("min_layer_height", 0))
                    .unwrap_or(0.);
                overrides.insert(
                    "layer_height".to_string(),
                    mm(finer_layer_height(current, min)),
                );
                let (lo, hi) = self.range_around_selection(&o);
                return self.push_range(i, intent.label(), lo, hi, overrides);
            }
            _ => {}
        }
        let kind = intent.kind().expect("intents with a part kind");
        let (centre, size) = placement(&o, Some(intent));
        let name = if intent == Intent::EmptyModifier {
            let n = o.volumes.iter().filter(|v| v.kind == kind).count() + 1;
            unique_part_name(&o, &format!("Modifier {n}"))
        } else {
            unique_part_name(&o, intent.part_name())
        };
        self.push_volume(i, &name, kind, fitted(Shape::Box, size), centre, overrides);
    }

    /// Adds `recipe` to the primary object (one undo step) and selects it.
    pub(crate) fn add_recipe(&mut self, index: usize) {
        let Some(recipe) = self.recipes.list.get(index).cloned() else {
            return;
        };
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self.plates.current().objects.get(i).cloned() else {
            return;
        };
        if recipe.kind == "range" {
            let (lo, hi) = self.range_around_selection(&o);
            return self.push_range(i, &recipe.name, lo, hi, recipe.overrides);
        }
        let Some(kind) = kind_from(&recipe.kind) else {
            self.notify_warning(format!(
                "The recipe “{}” has an unknown kind “{}”",
                recipe.name, recipe.kind
            ));
            return;
        };
        let shape = Shape::parse(&recipe.shape).unwrap_or(Shape::Box);
        let (centre, size) = placement(&o, None);
        let name = unique_part_name(&o, &recipe.name);
        self.push_volume(
            i,
            &name,
            kind,
            fitted(shape, size),
            centre,
            recipe.overrides,
        );
    }

    fn push_volume(
        &mut self,
        i: usize,
        name: &str,
        kind: PartKind,
        mesh: mica_core::mesh::TriangleMesh,
        centre: Vec3,
        overrides: BTreeMap<String, String>,
    ) {
        let mut v = Volume::new(name, kind, mesh);
        v.position = centre;
        v.overrides = overrides;
        let id = v.id;
        self.record(&format!("Add “{name}”"));
        self.plates.current_mut().objects[i].volumes.push(v);
        self.sync_instances(i);
        self.sub = Some(Sub::Volume(id));
        self.editor.object_mode = true;
        self.changed();
    }

    fn push_range(
        &mut self,
        i: usize,
        name: &str,
        lo: f32,
        hi: f32,
        overrides: BTreeMap<String, String>,
    ) {
        let mut r = HeightRange::new(lo, hi);
        r.overrides = overrides;
        let id = r.id;
        let mut o = self.plates.current().objects[i].clone();
        o.height_ranges.push(r);
        o.normalize_ranges();
        if o.range(id).is_none() {
            // Other ranges cover all of it already.
            self.notify_warning("The height ranges already cover that part of the object");
            return;
        }
        self.record(&format!("Add “{name}”"));
        self.plates.current_mut().objects[i] = o;
        self.sync_instances(i);
        self.sub = Some(Sub::Range(id));
        self.editor.object_mode = true;
        self.changed();
    }

    /// Millimetres above the object's bottom around the selection: the selected part's
    /// height span, else the middle third of the object.
    fn range_around_selection(&self, o: &Object) -> (f32, f32) {
        let (min, max) = o.bounds();
        let bottom = o.lowest_z();
        let height = (max.z - min.z).max(0.1);
        if let Some(Sub::Volume(id)) = self.sub
            && let Some(v) = o.volume(id)
        {
            let (a, b) = o.volume_bounds(v);
            let lo = (a.z - bottom).clamp(0., height);
            let hi = (b.z - bottom).clamp(0., height);
            if hi > lo + 0.2 && hi - lo < height * 0.9 {
                return (lo, hi);
            }
        }
        (height / 3., height * 2. / 3.)
    }

    /// "Fit to selection": the selected volume takes the object's bounds (one undo step).
    pub(crate) fn fit_part_to_object(&mut self) {
        let (Some(i), Some(Sub::Volume(id))) = (self.selected, self.sub) else {
            return;
        };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let target = local_size(o);
        let centre = o.center();
        let Some(v) = o.volume(id) else { return };
        let (a, b) = v.local_bounds;
        let own = (Vec3::from(b) - Vec3::from(a)).max(Vec3::splat(1e-3));
        let mid = (Vec3::from(a) + Vec3::from(b)) * 0.5;
        let scale = target / own;
        self.record("Fit to selection");
        let o = &mut self.plates.current_mut().objects[i];
        if let Some(v) = o.volume_mut(id) {
            v.rotation = Quat::IDENTITY;
            v.scale = scale;
            v.position = centre - mid * scale;
        }
        self.sync_instances(i);
        self.changed();
    }

    /// "Change shape": the selected volume becomes `shape` of the same size (one undo step).
    pub(crate) fn change_part_shape(&mut self, shape: Shape) {
        let (Some(i), Some(Sub::Volume(id))) = (self.selected, self.sub) else {
            return;
        };
        let Some(v) = self
            .plates
            .current()
            .objects
            .get(i)
            .and_then(|o| o.volume(id))
        else {
            return;
        };
        let (a, b) = v.local_bounds;
        let size = (Vec3::from(b) - Vec3::from(a)) * v.scale;
        let mid = v
            .local_transform()
            .transform_point3((Vec3::from(a) + Vec3::from(b)) * 0.5);
        self.record("Change shape");
        let mesh = fitted(shape, size);
        let o = &mut self.plates.current_mut().objects[i];
        if let Some(v) = o.volume_mut(id) {
            v.local_bounds = mesh.bounds();
            v.mesh = std::sync::Arc::new(mesh);
            v.scale = Vec3::ONE;
            v.position = mid;
        }
        self.sync_instances(i);
        self.changed();
    }

    /// "Save recipe": the selected part's kind, shape and overrides under `name`. A recipe of
    /// the same name is replaced. Errors go to the notifications.
    pub(crate) fn save_recipe(&mut self, name: &str) {
        let name = crate::tree::clean_name(name);
        let (Some(i), Some(sub)) = (self.selected, self.sub) else {
            return;
        };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let recipe = match sub {
            Sub::Volume(id) => {
                let Some(v) = o.volume(id) else { return };
                Recipe {
                    name: name.clone(),
                    kind: kind_name(&v.kind).into(),
                    shape: guess_shape(&v.mesh).label().to_lowercase(),
                    overrides: v.overrides.clone(),
                }
            }
            Sub::Range(id) => {
                let Some(r) = o.range(id) else { return };
                Recipe {
                    name: name.clone(),
                    kind: "range".into(),
                    shape: String::new(),
                    overrides: r.overrides.clone(),
                }
            }
        };
        if name.is_empty() {
            self.notify_warning("A recipe needs a name");
            return;
        }
        let before = self.recipes.list.clone();
        self.recipes.list.retain(|r| r.name != name);
        self.recipes.list.push(recipe);
        match self.recipes.save() {
            Ok(()) => self.status = format!("Saved the recipe “{name}”"),
            Err(e) => {
                self.recipes.list = before;
                self.notify_error("The recipe was not saved", e);
            }
        }
    }

    /// Deletes recipe `index` (from the Recipes submenu).
    pub(crate) fn delete_recipe(&mut self, index: usize) {
        if index >= self.recipes.list.len() {
            return;
        }
        let before = self.recipes.list.clone();
        let gone = self.recipes.list.remove(index);
        match self.recipes.save() {
            Ok(()) => self.status = format!("Deleted the recipe “{}”", gone.name),
            Err(e) => {
                self.recipes.list = before;
                self.notify_error("The recipe was not deleted", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Options;
    use eframe::egui;
    use mica_profiles::user::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mica-intents-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn app() -> App {
        let ctx = egui::Context::default();
        let mut app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir().join("store"))),
                files: vec![],
                headless: true,
            },
        );
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/cube20.stl"
        ));
        let result = crate::app::load_files(vec![path], &app.config, app.bed.center());
        app.finish_load(result);
        app.select(Some(0), false);
        app
    }

    fn base(app: &App) -> mica_core::config::Config {
        app.part_base(0).unwrap()
    }

    #[test]
    fn each_intent_creates_the_right_part_in_one_undo_step() {
        for intent in Intent::MENU.into_iter().chain([Intent::EmptyModifier]) {
            let mut a = app();
            let walls = base(&a).int("perimeters").unwrap();
            let height = base(&a).float("layer_height").unwrap();
            a.add_intent(intent);
            let o = &a.plates.current().objects[0];
            match intent.kind() {
                Some(kind) => {
                    assert_eq!(o.volumes.len(), 1, "{intent:?}");
                    let v = &o.volumes[0];
                    assert_eq!(v.kind, kind, "{intent:?}");
                    assert_eq!(a.sub, Some(Sub::Volume(v.id)));
                    let get = |k: &str| v.overrides.get(k).map(String::as_str);
                    match intent {
                        Intent::Stronger => {
                            assert_eq!(v.name, "Stronger here");
                            assert_eq!(get("perimeters"), Some((walls + 2).to_string().as_str()));
                            assert_eq!(get("fill_density"), Some("40%"));
                            assert_eq!(get("fill_pattern"), Some("gyroid"));
                        }
                        Intent::Smooth => assert_eq!(get("ironing"), Some("1")),
                        Intent::Hole => assert_eq!(v.name, "Hole"),
                        _ => assert!(v.overrides.is_empty(), "{intent:?}"),
                    }
                    // Centred in the object, 40 % of its footprint across.
                    let (a0, b0) = o.local_bounds;
                    let size = Vec3::from(b0) - Vec3::from(a0);
                    let (va, vb) = v.local_bounds;
                    assert!(((vb[0] - va[0]) - size.x * 0.4).abs() < 1e-3);
                    assert!((v.position - o.center()).truncate().length() < 1e-3);
                }
                None => {
                    assert_eq!(o.height_ranges.len(), 1);
                    let r = &o.height_ranges[0];
                    let want = finer_layer_height(height, 0.);
                    assert_eq!(
                        r.overrides.get("layer_height").map(String::as_str),
                        Some(mm(want).as_str())
                    );
                    // The middle third.
                    assert!((r.min_z - 20. / 3.).abs() < 1e-3 && (r.max_z - 40. / 3.).abs() < 1e-3);
                }
            }
            // One undo step takes it all back.
            a.undo();
            let o = &a.plates.current().objects[0];
            assert!(
                o.volumes.is_empty() && o.height_ranges.is_empty(),
                "{intent:?}"
            );
        }
    }

    #[test]
    fn finer_layers_respect_the_floor_and_the_printer() {
        assert_eq!(finer_layer_height(0.2, 0.), 0.12);
        assert_eq!(finer_layer_height(0.1, 0.), 0.08);
        assert_eq!(finer_layer_height(0.2, 0.15), 0.15);
        assert_eq!(mm(0.12000001), "0.12");
    }

    #[test]
    fn fit_and_change_shape_keep_the_part_in_place() {
        let mut a = app();
        a.add_intent(Intent::Stronger);
        a.change_part_shape(Shape::Cylinder);
        let o = &a.plates.current().objects[0];
        let v = &o.volumes[0];
        assert_eq!(guess_shape(&v.mesh), Shape::Cylinder);
        assert!((v.position - o.center()).length() < 1e-3);
        a.fit_part_to_object();
        let o = &a.plates.current().objects[0];
        let (min, max) = o.volume_bounds(&o.volumes[0]);
        let (omin, omax) = o.bounds();
        assert!((min - omin).length() < 1e-2 && (max - omax).length() < 1e-2);
        a.undo();
        a.undo();
        assert_eq!(
            guess_shape(&a.plates.current().objects[0].volumes[0].mesh),
            Shape::Box
        );
    }

    #[test]
    fn recipes_save_load_add_and_delete() {
        let d = dir();
        let mut a = app();
        a.recipes = Recipes {
            list: vec![],
            path: Some(Recipes::path_for(&d.join("mica.toml"))),
        };
        a.add_intent(Intent::Stronger);
        a.save_recipe("Strong core");
        assert_eq!(a.recipes.list.len(), 1);
        let (loaded, warning) = Recipes::load(a.recipes.path.clone());
        assert!(warning.is_none());
        assert_eq!(loaded.list, a.recipes.list);
        assert_eq!(loaded.list[0].kind, "modifier");
        assert_eq!(loaded.list[0].shape, "box");
        // Added from the menu: same kind and overrides, named after the recipe.
        a.sub = None;
        a.add_recipe(0);
        let o = &a.plates.current().objects[0];
        assert_eq!(o.volumes.len(), 2);
        assert_eq!(o.volumes[1].name, "Strong core");
        assert_eq!(o.volumes[1].overrides, o.volumes[0].overrides);
        // A range recipe, then deleting the first.
        a.add_intent(Intent::Finer);
        a.save_recipe("Fine band");
        assert_eq!(Recipes::load(a.recipes.path.clone()).0.list.len(), 2);
        a.delete_recipe(0);
        let (loaded, _) = Recipes::load(a.recipes.path.clone());
        assert_eq!(loaded.list.len(), 1);
        assert_eq!(loaded.list[0].kind, "range");
        // A file that cannot be written: the list stays as it was and a notification says so.
        a.recipes.path = Some(d.join("missing-dir/\0/recipes.toml"));
        a.delete_recipe(0);
        assert_eq!(a.recipes.list.len(), 1);
        assert!(
            a.notifications
                .list
                .iter()
                .any(|n| n.title.contains("recipe"))
        );
    }
}
