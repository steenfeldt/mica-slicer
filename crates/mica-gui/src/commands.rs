// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The everyday object operations and the right-click menus that offer them. One
//! [`Command`] is one undo entry. The viewport and the object list build the same menus
//! from a [`MenuInfo`] snapshot and hand the chosen command to [`App::run_command`], which
//! acts on the current selection (every selected object, where that makes sense).

use crate::{
    app::App,
    keymap::{Act, Tool, shortcut_text as key},
    scene::{self, Delta, HeightRange, Object, Sub, Volume},
};
use eframe::egui;
use glam::{Vec2, Vec3};
use mica_core::threemf::PartKind;
use std::collections::BTreeSet;

/// Primitive shapes for new modifiers and negative volumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Box,
    Cylinder,
    Sphere,
    Slab,
}
impl Shape {
    pub const ALL: [Shape; 4] = [Shape::Box, Shape::Cylinder, Shape::Sphere, Shape::Slab];
    pub fn label(self) -> &'static str {
        match self {
            Shape::Box => "Box",
            Shape::Cylinder => "Cylinder",
            Shape::Sphere => "Sphere",
            Shape::Slab => "Slab",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.label().eq_ignore_ascii_case(s))
    }
    /// The primitive, sized for an object of `size` (its world bounding box): a modifier box
    /// covers half the object in each direction, a cylinder is a third of its footprint, a
    /// slab spans the footprint with a fifth of the height.
    pub fn mesh(self, size: Vec3) -> mica_core::mesh::TriangleMesh {
        use mica_core::mesh_edit as m;
        let size = size.max(Vec3::splat(1.));
        match self {
            Shape::Box => m::cuboid((size * 0.5).to_array()),
            Shape::Cylinder => m::cylinder(size.x.min(size.y) / 3., size.z * 0.6, 48),
            Shape::Sphere => m::sphere(size.min_element() * 0.35, 24, 48),
            Shape::Slab => m::slab([size.x * 1.1, size.y * 1.1, (size.z * 0.2).max(1.)]),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Copy,
    Paste,
    Duplicate,
    Delete,
    AddInstance,
    RemoveInstance,
    Centre,
    DropToBed,
    Mirror(usize),
    Rotate90,
    ChangeTool(usize),
    MoveToPlate(usize),
    AddModifier(Shape),
    AddNegative(Shape),
    AddSupportEnforcer(Shape),
    AddSupportBlocker(Shape),
    AddHeightRange,
    SplitToObjects,
    SplitToParts,
    ExportStl,
    Rename,
    ObjectSettings,
    AddModel,
    Arrange,
    ArrangeAll,
    SelectAll,
    Cut,
    /// Opens the mesh boolean card (B).
    MeshBoolean,
    /// Opens the simplify card.
    Simplify,
    /// Repairs the selected objects' meshes.
    Repair,
    /// Moves the object's texture into its mesh.
    BakeTexture,
    AutoOrient,
    /// Delete the selected volume or height range.
    DeleteSub,
    /// Move the object just inside the bed, with its skirt and brim.
    MoveInside,
    /// Open the Text / SVG tool on the selected text part, or on the text object.
    EditText,
    /// A raised text on the object, with the Text tool open on it.
    AddText,
    /// An SVG file's shapes raised on the object (a file dialog first).
    AddSvg,
}

/// What the menus need to know about the scene.
pub struct MenuInfo {
    /// Tools to choose from: the physical ones, then the mixed colors.
    pub tools: usize,
    /// Their names (`Tool 2`, a mixed color's name); empty: `Tool N`.
    pub tool_labels: Vec<String>,
    pub tool: usize,
    pub plates: Vec<String>,
    pub current_plate: usize,
    pub instances: usize,
    pub can_paste: bool,
    pub selection: usize,
    pub locked: bool,
    pub sub: Option<&'static str>,
    /// The right-clicked object (or its skirt or brim) is past the bed's edge and fits when
    /// moved: "Move inside" is offered.
    pub move_inside: bool,
    /// The Textures feature is on (Settings › Features): "Bake texture into mesh" is offered.
    pub textures: bool,
    /// The target is a text (the selected text part, or a text object): "Edit text" is offered.
    pub text: bool,
}

fn item(ui: &mut egui::Ui, text: &str, shortcut: &str, enabled: bool) -> bool {
    let b = egui::Button::new(text).shortcut_text(shortcut);
    let r = ui.add_enabled(enabled, b);
    if r.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

fn shapes(ui: &mut egui::Ui, label: &str, make: impl Fn(Shape) -> Command) -> Option<Command> {
    let mut out = None;
    ui.menu_button(label, |ui| {
        for s in Shape::ALL {
            if ui.button(s.label()).clicked() {
                out = Some(make(s));
                ui.close();
            }
        }
    });
    out
}

/// The menu for a right-clicked object (and the rest of the selection).
pub fn object_menu(ui: &mut egui::Ui, info: &MenuInfo) -> Option<Command> {
    ui.set_min_width(230.);
    let mut out = None;
    macro_rules! pick {
        ($c:expr) => {
            out = Some($c)
        };
    }
    let edit = !info.locked;
    let many = info.selection > 1;
    if info.text {
        if item(ui, "Edit text", &key(Act::Tool(Tool::Emboss)), edit) {
            pick!(Command::EditText);
        }
        ui.separator();
    }
    if !many {
        let text_key = if info.text {
            String::new()
        } else {
            key(Act::Tool(Tool::Emboss))
        };
        if item(ui, "Add text…", &text_key, edit) {
            pick!(Command::AddText);
        }
        if item(ui, "Add SVG…", "", edit) {
            pick!(Command::AddSvg);
        }
        ui.separator();
    }
    if let Some(sub) = info.sub {
        if item(ui, &format!("Delete {}", sub.to_lowercase()), "Del", edit) {
            pick!(Command::DeleteSub);
        }
        ui.separator();
    }
    if item(ui, "Copy", &key(Act::Copy), true) {
        pick!(Command::Copy);
    }
    if item(ui, "Paste", &key(Act::Paste), info.can_paste && edit) {
        pick!(Command::Paste);
    }
    if item(ui, "Duplicate", &key(Act::Clone), edit) {
        pick!(Command::Duplicate);
    }
    if item(ui, "Delete", &key(Act::Delete), edit) {
        pick!(Command::Delete);
    }
    ui.separator();
    if item(ui, "Add instance", &key(Act::AddInstance), edit) {
        pick!(Command::AddInstance);
    }
    if item(
        ui,
        "Remove instance",
        &key(Act::RemoveInstance),
        edit && info.instances > 1,
    ) {
        pick!(Command::RemoveInstance);
    }
    ui.separator();
    if info.move_inside && item(ui, "Move inside", "", edit) {
        pick!(Command::MoveInside);
    }
    if item(ui, "Center on plate", "", edit) {
        pick!(Command::Centre);
    }
    if item(ui, "Drop to bed", "", edit) {
        pick!(Command::DropToBed);
    }
    if item(ui, "Rotate 90°", "", edit) {
        pick!(Command::Rotate90);
    }
    if item(ui, "Auto-orient", &key(Act::Orient), edit) {
        pick!(Command::AutoOrient);
    }
    ui.menu_button("Mirror", |ui| {
        for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
            if ui.add_enabled(edit, egui::Button::new(*axis)).clicked() {
                out = Some(Command::Mirror(k));
                ui.close();
            }
        }
    });
    if info.tools > 1 {
        ui.menu_button("Change tool", |ui| {
            for t in 0..info.tools {
                let label = info
                    .tool_labels
                    .get(t)
                    .cloned()
                    .unwrap_or_else(|| format!("Tool {}", t + 1));
                if ui.selectable_label(t == info.tool, label).clicked() {
                    out = Some(Command::ChangeTool(t));
                    ui.close();
                }
            }
        });
    }
    ui.menu_button("Move to plate", |ui| {
        for (i, name) in info.plates.iter().enumerate() {
            if ui
                .add_enabled(
                    edit && i != info.current_plate,
                    egui::Button::new(if name.is_empty() {
                        format!("Plate {}", i + 1)
                    } else {
                        name.clone()
                    }),
                )
                .clicked()
            {
                out = Some(Command::MoveToPlate(i));
                ui.close();
            }
        }
        ui.separator();
        if ui
            .add_enabled(edit, egui::Button::new("New plate"))
            .clicked()
        {
            out = Some(Command::MoveToPlate(info.plates.len()));
            ui.close();
        }
    });
    ui.separator();
    if let Some(c) = shapes(ui, "Add modifier", Command::AddModifier) {
        pick!(c);
    }
    if let Some(c) = shapes(ui, "Add negative volume", Command::AddNegative) {
        pick!(c);
    }
    ui.menu_button("Add support volume", |ui| {
        if let Some(c) = shapes(ui, "Enforcer", Command::AddSupportEnforcer) {
            out = Some(c);
        }
        if let Some(c) = shapes(ui, "Blocker", Command::AddSupportBlocker) {
            out = Some(c);
        }
    });
    if item(ui, "Add height range", "", edit) {
        pick!(Command::AddHeightRange);
    }
    ui.separator();
    ui.menu_button("Split", |ui| {
        if ui
            .add_enabled(edit && !many, egui::Button::new("To objects"))
            .on_hover_text("One object per connected piece")
            .clicked()
        {
            out = Some(Command::SplitToObjects);
            ui.close();
        }
        if ui
            .add_enabled(edit && !many, egui::Button::new("To parts"))
            .on_hover_text("Keep one object; each connected piece becomes a part")
            .clicked()
        {
            out = Some(Command::SplitToParts);
            ui.close();
        }
    });
    if item(ui, "Cut…", &key(Act::Tool(Tool::Cut)), edit && !many) {
        pick!(Command::Cut);
    }
    if item(
        ui,
        "Mesh boolean…",
        &key(Act::Tool(Tool::MeshBoolean)),
        edit && !many,
    ) {
        pick!(Command::MeshBoolean);
    }
    if item(ui, "Simplify…", "", edit && !many) {
        pick!(Command::Simplify);
    }
    if item(ui, "Repair mesh", "", edit) {
        pick!(Command::Repair);
    }
    if info.textures
        && ui
            .add_enabled(edit && !many, egui::Button::new("Bake texture into mesh"))
            .on_hover_text("Make the texture part of the model's mesh (tops too), so other slicers and later edits see it. The texture settings and texture paint of the object are turned off.")
            .clicked()
    {
        out = Some(Command::BakeTexture);
        ui.close();
    }
    if item(ui, "Export as STL…", "", true) {
        pick!(Command::ExportStl);
    }
    ui.separator();
    if item(ui, "Rename", &key(Act::Rename), !many) {
        pick!(Command::Rename);
    }
    if item(ui, "Object settings…", "", !many) {
        pick!(Command::ObjectSettings);
    }
    out
}

/// The menu for a right-click on an empty part of the bed.
pub fn bed_menu(ui: &mut egui::Ui, info: &MenuInfo) -> Option<Command> {
    ui.set_min_width(200.);
    let mut out = None;
    if item(
        ui,
        "Paste",
        &key(Act::Paste),
        info.can_paste && !info.locked,
    ) {
        out = Some(Command::Paste);
    }
    if item(ui, "Add model…", &key(Act::Import), true) {
        out = Some(Command::AddModel);
    }
    // Arranging and selecting this plate's objects are in the plate menu that follows.
    if item(
        ui,
        "Arrange all plates",
        &key(Act::ArrangeAll),
        info.plates.len() > 1,
    ) {
        out = Some(Command::ArrangeAll);
    }
    out
}

/// Default spacing between placed copies, mm.
pub const SPACING: f32 = 6.;

impl App {
    /// Every selected object index on the current plate, sorted; the primary first-selected
    /// object is `selected`, the others `multi`.
    pub(crate) fn selection(&self) -> Vec<usize> {
        let len = self.plates.current().objects.len();
        let mut all: BTreeSet<usize> = self.multi.iter().copied().filter(|&i| i < len).collect();
        if let Some(i) = self.selected.filter(|&i| i < len) {
            all.insert(i);
        }
        all.into_iter().collect()
    }

    /// Selects `index` (None clears). With `toggle` the object joins or leaves the selection
    /// instead of replacing it.
    pub(crate) fn select(&mut self, index: Option<usize>, toggle: bool) {
        self.sub = None;
        // The cut tool works on one object: another selection closes it.
        if self
            .cut
            .as_ref()
            .is_some_and(|c| index != Some(c.object) || toggle)
        {
            self.cut = None;
        }
        self.tower_selected = false;
        self.editor.tower_mode = false;
        match (index, toggle) {
            (None, _) => {
                self.selected = None;
                self.multi.clear();
                self.editor.object_mode = false;
            }
            (Some(i), false) => {
                self.selected = Some(i);
                self.multi.clear();
            }
            (Some(i), true) => {
                if self.selected == Some(i) {
                    // Leaving the primary: the next one takes over.
                    self.selected = self.multi.pop_first();
                } else if self.multi.remove(&i) {
                    // Left the selection.
                } else {
                    match self.selected {
                        None => self.selected = Some(i),
                        Some(p) => {
                            self.multi.insert(p);
                            self.multi.remove(&i);
                            self.selected = Some(i);
                        }
                    }
                }
            }
        }
    }

    /// Replaces the selection with these indices (box select, select all).
    pub(crate) fn select_many(&mut self, indices: &[usize], toggle: bool) {
        if !toggle {
            self.selected = None;
            self.multi.clear();
        }
        self.sub = None;
        for &i in indices {
            match self.selected {
                None => self.selected = Some(i),
                Some(p) if p != i => {
                    self.multi.insert(i);
                }
                _ => {}
            }
        }
        // A new primary object closes the cut tool of the old one.
        self.validate_selection();
    }

    /// Objects of the current plate sharing `object_id` with object `i` (instances).
    pub(crate) fn instances_of(&self, i: usize) -> Vec<usize> {
        let objects = &self.plates.current().objects;
        let Some(o) = objects.get(i) else {
            return vec![];
        };
        objects
            .iter()
            .enumerate()
            .filter(|(_, p)| p.object_id == o.object_id)
            .map(|(k, _)| k)
            .collect()
    }

    /// Copies what instances share from object `i` to its siblings.
    pub(crate) fn sync_instances(&mut self, i: usize) {
        let siblings = self.instances_of(i);
        let plate = self.plates.current_mut();
        let Some(source) = plate.objects.get(i).cloned() else {
            return;
        };
        for k in siblings {
            if k != i {
                source.sync_shared(&mut plate.objects[k]);
            }
        }
    }

    pub(crate) fn next_object_id(&self) -> usize {
        self.plates
            .plates
            .iter()
            .flat_map(|p| p.objects.iter().map(|o| o.object_id))
            .max()
            .map_or(0, |n| n + 1)
    }

    /// `name`, `name (2)`, `name (3)`… unique on the plate.
    pub(crate) fn unique_name(&self, base: &str) -> String {
        let plate = self.plates.current();
        let taken = |n: &str| plate.objects.iter().any(|o| o.name == n);
        if !taken(base) {
            return base.to_string();
        }
        let stem = base
            .rsplit_once(" (")
            .filter(|(_, rest)| rest.trim_end_matches(')').parse::<usize>().is_ok())
            .map_or(base, |(s, _)| s);
        (2..)
            .map(|n| format!("{stem} ({n})"))
            .find(|n| !taken(n))
            .unwrap_or_else(|| base.to_string())
    }

    /// A new plain object of `mesh` with the plate's configuration and default tools (not yet
    /// on the plate).
    pub(crate) fn plain_object(&self, name: String, mesh: mica_core::mesh::TriangleMesh) -> Object {
        let count = self
            .config
            .as_ref()
            .map(|c| c.vector_len("nozzle_diameter").max(1))
            .unwrap_or(1);
        let tools = self
            .config
            .as_ref()
            .ok()
            .and_then(|c| {
                mica_core::print_job::ToolAssignments::resolve(
                    c,
                    &Default::default(),
                    &Default::default(),
                    0,
                    count,
                )
                .ok()
            })
            .unwrap_or(mica_core::print_job::ToolAssignments([
                Some(0),
                Some(0),
                Some(0),
                None,
                None,
            ]));
        let config = self
            .config
            .clone()
            .unwrap_or_else(|_| mica_core::config::Config::defaults());
        Object::new(mica_core::input::InputObject::plain(
            name,
            self.next_object_id(),
            0,
            mesh,
            tools,
            config,
        ))
    }

    /// Places `o` on the current plate at a free spot near `near` (XY of its footprint's
    /// bottom-left) or, with `at`, around that bed point; returns its index.
    fn place_copy(&mut self, mut o: Object, near: Vec2, at: Option<Vec2>) -> usize {
        let (min, max) = o.bounds();
        let size = (max - min).truncate();
        let bed = self.bed.clone();
        let spot = match at {
            Some(p) => {
                let corner = p - size * 0.5;
                let taken = &self.plates.current().objects;
                let clear = !taken.iter().any(|t| {
                    let (a, b) = t.bounds();
                    a.x < corner.x + size.x
                        && corner.x < b.x
                        && a.y < corner.y + size.y
                        && corner.y < b.y
                });
                if clear {
                    corner
                } else {
                    scene::free_spot(taken, size, corner, &bed, SPACING)
                }
            }
            None => scene::free_spot(&self.plates.current().objects, size, near, &bed, SPACING),
        };
        scene::move_footprint_to(&mut o, spot);
        let plate = self.plates.current_mut();
        plate.objects.push(o);
        plate.objects.len() - 1
    }

    /// Cmd+C: the selection (with settings, painting, volumes and ranges) into the clipboard.
    pub(crate) fn copy_selection(&mut self) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        let objects = &self.plates.current().objects;
        self.clipboard = sel
            .iter()
            .filter_map(|&i| objects.get(i).cloned())
            .collect();
        self.status = format!(
            "Copied {} object{}",
            self.clipboard.len(),
            if self.clipboard.len() == 1 { "" } else { "s" }
        );
    }

    /// Cmd+V: the clipboard onto the current plate, next to the originals or around `at`
    /// (the bed point under the pointer). The group keeps its relative layout.
    pub(crate) fn paste(&mut self, at: Option<Vec2>) {
        if self.clipboard.is_empty() || self.plate_locked() {
            return;
        }
        self.record("Paste");
        let mut group_min = Vec2::splat(f32::INFINITY);
        let mut group_max = -group_min;
        for o in &self.clipboard {
            let (a, b) = o.bounds();
            group_min = group_min.min(a.truncate());
            group_max = group_max.max(b.truncate());
        }
        let size = group_max - group_min;
        let bed = self.bed.clone();
        let spot = match at {
            Some(p) => p - size * 0.5,
            None => scene::free_spot(
                &self.plates.current().objects,
                size,
                group_min,
                &bed,
                SPACING,
            ),
        };
        let shift = spot - group_min;
        let mut ids = std::collections::BTreeMap::new();
        let mut next = self.next_object_id();
        let mut pasted = vec![];
        for o in self.clipboard.clone() {
            let mut copy = o.fresh();
            // Instances in the clipboard stay instances of each other; everything else is new.
            copy.object_id = *ids.entry(o.object_id).or_insert_with(|| {
                next += 1;
                next - 1
            });
            copy.name = self.unique_name(&o.name);
            copy.position += shift.extend(0.);
            let plate = self.plates.current_mut();
            plate.objects.push(copy);
            pasted.push(plate.objects.len() - 1);
        }
        self.select_many(&pasted, false);
        self.changed();
        self.status = format!(
            "Pasted {} object{}",
            pasted.len(),
            if pasted.len() == 1 { "" } else { "s" }
        );
    }

    /// Cmd+D: independent copies of the selection, placed next to the originals.
    pub(crate) fn duplicate_selection(&mut self) {
        let sel = self.selection();
        if sel.is_empty() || self.plate_locked() {
            return;
        }
        self.record("Duplicate");
        let mut made = vec![];
        let mut next = self.next_object_id();
        for i in sel {
            let Some(o) = self.plates.current().objects.get(i).cloned() else {
                continue;
            };
            let mut copy = o.fresh();
            copy.object_id = next;
            next += 1;
            copy.name = self.unique_name(&o.name);
            let near = o.bounds().0.truncate();
            made.push(self.place_copy(copy, near, None));
        }
        self.select_many(&made, false);
        self.changed();
    }

    /// "+": one more instance of each selected object (shared mesh and settings), next to it.
    pub(crate) fn add_instances(&mut self) {
        let sel = self.selection();
        if sel.is_empty() || self.plate_locked() {
            return;
        }
        self.record("Add instance");
        let mut made = vec![];
        for i in sel {
            let Some(o) = self.plates.current().objects.get(i).cloned() else {
                continue;
            };
            let mut copy = o.fresh();
            copy.instance_id = self
                .instances_of(i)
                .iter()
                .filter_map(|&k| self.plates.current().objects.get(k))
                .map(|p| p.instance_id)
                .max()
                .map_or(0, |n| n + 1);
            let near = o.bounds().0.truncate();
            made.push(self.place_copy(copy, near, None));
        }
        self.select_many(&made, false);
        self.changed();
    }

    /// "−": removes the last instance of each selected object's group (never the last one).
    pub(crate) fn remove_instances(&mut self) {
        let sel = self.selection();
        if sel.is_empty() || self.plate_locked() {
            return;
        }
        let mut doomed: BTreeSet<usize> = BTreeSet::new();
        let mut groups: BTreeSet<usize> = BTreeSet::new();
        for i in sel {
            let Some(o) = self.plates.current().objects.get(i) else {
                continue;
            };
            if !groups.insert(o.object_id) {
                continue;
            }
            let siblings = self.instances_of(i);
            if siblings.len() > 1 {
                // The highest instance id goes, so the original stays.
                let objects = &self.plates.current().objects;
                if let Some(&k) = siblings
                    .iter()
                    .max_by_key(|&&k| (objects[k].instance_id, k))
                {
                    doomed.insert(k);
                }
            }
        }
        if doomed.is_empty() {
            self.status = "Only one instance left".into();
            return;
        }
        self.record("Remove instance");
        let plate = self.plates.current_mut();
        for k in doomed.iter().rev() {
            plate.objects.remove(*k);
        }
        // The first remaining instance of each group stays selected.
        let objects = &self.plates.current().objects;
        let keep: Vec<usize> = groups
            .iter()
            .filter_map(|id| objects.iter().position(|o| o.object_id == *id))
            .collect();
        self.select_many(&keep, false);
        self.changed();
    }

    /// Runs a command on the selection (one undo entry each).
    pub(crate) fn run_command(&mut self, cmd: Command, ctx: &egui::Context) {
        let sel = self.selection();
        let primary = self
            .selected
            .filter(|&i| i < self.plates.current().objects.len());
        match cmd {
            Command::Copy => self.copy_selection(),
            Command::Paste => {
                let at = self.viewport.bed_hover.map(|p| p.truncate());
                self.paste(at);
            }
            Command::Duplicate => self.duplicate_selection(),
            Command::Delete => self.delete_selected(),
            Command::DeleteSub => self.delete_sub(),
            Command::AddText => {
                if primary.is_some() {
                    self.select(primary, false);
                    self.add_text();
                }
            }
            Command::AddSvg => {
                if primary.is_some() {
                    self.select(primary, false);
                    self.pick_svg_for_selection();
                }
            }
            Command::EditText => {
                if let Some(i) = primary
                    && !self.edit_text(i, self.sub)
                {
                    self.status = "Select a text to edit it".into();
                }
            }
            Command::AddInstance => self.add_instances(),
            Command::RemoveInstance => self.remove_instances(),
            Command::MoveInside => {
                if let Some(i) = primary {
                    let plate = self.plates.current;
                    self.move_inside(plate, i);
                }
            }
            Command::Centre => {
                if sel.is_empty() || self.plate_locked() {
                    return;
                }
                self.record("Center on plate");
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = -min;
                for &i in &sel {
                    let (a, b) = self.plates.current().objects[i].bounds();
                    min = min.min(a);
                    max = max.max(b);
                }
                let shift = (self.bed.center() - (min + max) * 0.5).with_z(0.);
                for &i in &sel {
                    self.plates.current_mut().objects[i].position += shift;
                }
                self.changed();
            }
            Command::DropToBed => {
                if sel.is_empty() || self.plate_locked() {
                    return;
                }
                self.record("Drop to bed");
                for &i in &sel {
                    self.plates.current_mut().objects[i].drop_to_bed();
                }
                self.changed();
            }
            Command::Mirror(axis) => {
                if sel.is_empty() || self.plate_locked() {
                    return;
                }
                self.record("Mirror");
                for &i in &sel {
                    let o = &mut self.plates.current_mut().objects[i];
                    o.mirror[axis] = -o.mirror[axis];
                    o.drop_to_bed();
                }
                self.changed();
            }
            Command::Rotate90 => {
                if sel.is_empty() || self.plate_locked() {
                    return;
                }
                self.record("Rotate");
                self.transform_selection(Delta::Rotate {
                    axis: Vec3::Z,
                    angle: std::f32::consts::FRAC_PI_2,
                });
                self.changed();
            }
            Command::ChangeTool(t) => {
                if sel.is_empty() {
                    return;
                }
                self.record("Change tool");
                for &i in &sel {
                    self.plates.current_mut().objects[i].set_tool(t);
                    self.sync_instances(i);
                }
                self.changed();
            }
            Command::MoveToPlate(target) => self.move_to_plate(target),
            Command::AddModifier(s) => self.add_volume(PartKind::Modifier, s),
            Command::AddNegative(s) => self.add_volume(PartKind::NegativeVolume, s),
            Command::AddSupportEnforcer(s) => self.add_volume(PartKind::SupportEnforcer, s),
            Command::AddSupportBlocker(s) => self.add_volume(PartKind::SupportBlocker, s),
            Command::AddHeightRange => self.add_height_range(),
            Command::SplitToObjects => self.split(false),
            Command::SplitToParts => self.split(true),
            Command::ExportStl => self.export_stl(),
            Command::BakeTexture => {
                self.bake_texture();
            }
            Command::Rename => {
                if let Some(i) = primary {
                    let name = self.plates.current().objects[i].name.clone();
                    self.renaming = Some((i, name));
                }
            }
            Command::ObjectSettings => {
                if primary.is_some() {
                    self.editor.object_mode = true;
                }
            }
            Command::AddModel => self.open_dialog(ctx),
            Command::Arrange => self.arrange(),
            Command::ArrangeAll => self.arrange_all(),
            Command::SelectAll => {
                let all: Vec<usize> = (0..self.plates.current().objects.len()).collect();
                self.select_many(&all, false);
            }
            Command::Cut => self.toggle_cut(),
            Command::MeshBoolean => self.toggle_boolean(),
            Command::Simplify => self.toggle_simplify(),
            Command::Repair => self.repair_selection(),
            Command::AutoOrient => self.auto_orient(),
        }
    }

    /// Applies a delta to every selected object (a translation moves them together, a Z turn
    /// or a scale acts about the selection's centre so the group keeps its layout).
    pub(crate) fn transform_selection(&mut self, delta: Delta) {
        let sel = self.selection();
        if sel.len() <= 1 {
            if let Some(i) = sel.first()
                && let Some(o) = self.plates.current_mut().objects.get_mut(*i)
            {
                o.apply(delta);
            }
            return;
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = -min;
        for &i in &sel {
            let (a, b) = self.plates.current().objects[i].bounds();
            min = min.min(a);
            max = max.max(b);
        }
        let centre = ((min + max) * 0.5).with_z(0.);
        for &i in &sel {
            let o = &mut self.plates.current_mut().objects[i];
            match delta {
                Delta::Translate(_) => o.apply(delta),
                Delta::Rotate { axis, angle } if axis.abs_diff_eq(Vec3::Z, 1e-4) => {
                    let pivot = o.pivot_world().with_z(0.);
                    let turned = glam::Quat::from_rotation_z(angle) * (pivot - centre) + centre;
                    o.position += turned - pivot;
                    o.apply(delta);
                }
                Delta::Rotate { .. } => o.apply(delta),
                Delta::Scale(f) => {
                    let pivot = o.pivot_world().with_z(0.);
                    o.position += (pivot - centre) * (f - Vec3::ONE).with_z(0.);
                    o.apply(delta);
                }
            }
        }
    }

    /// Moves the selection to plate `target` (a new plate when `target` is past the end),
    /// keeping each object's plate-local position.
    pub(crate) fn move_to_plate(&mut self, target: usize) {
        let sel = self.selection();
        if sel.is_empty() || self.plate_locked() {
            return;
        }
        if target < self.plates.len() && self.plates.plate(target).locked {
            self.status = "That plate is locked".into();
            return;
        }
        self.record("Move to plate");
        if target >= self.plates.len() {
            let current = self.plates.current;
            self.plates.add();
            self.plates.current = current;
        }
        let target = target.min(self.plates.len() - 1);
        let mut moved = vec![];
        for &i in sel.iter().rev() {
            let o = self.plates.current_mut().objects.remove(i);
            moved.push(o);
        }
        // Object ids stay unique per plate.
        let base = self.plates.plates[target]
            .objects
            .iter()
            .map(|o| o.object_id)
            .max()
            .map_or(0, |n| n + 1);
        let mut ids = std::collections::BTreeMap::new();
        let mut next = base;
        for mut o in moved.into_iter().rev() {
            o.object_id = *ids.entry(o.object_id).or_insert_with(|| {
                next += 1;
                next - 1
            });
            self.plates.plates[target].objects.push(o);
        }
        self.plates.plates[target].invalidate();
        self.select(None, false);
        self.changed();
        self.select_plate(target);
        let n = self.plates.plate(target).objects.len();
        let moved_n = ids.len();
        self.select_many(&(n - moved_n..n).collect::<Vec<_>>(), false);
    }

    /// A new modifier / negative / support volume of `shape` on the primary object, centred
    /// in it, selected for editing.
    pub(crate) fn add_volume(&mut self, kind: PartKind, shape: Shape) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let (min, max) = o.bounds();
        let size = max - min;
        let mesh = shape.mesh(size);
        let n = o.volumes.iter().filter(|v| v.kind == kind).count() + 1;
        let mut v = Volume::new(
            format!("{} {n}", scene::kind_label(&kind)),
            kind.clone(),
            mesh,
        );
        // Centred in the object (local frame = the mesh's frame, pivot at its centre).
        v.position = o.center();
        if matches!(kind, PartKind::Modifier) {
            // A modifier without settings does nothing; start with dense infill.
            v.overrides.insert("fill_density".into(), "100%".into());
        }
        let id = v.id;
        self.record(match kind {
            PartKind::Modifier => "Add modifier",
            PartKind::NegativeVolume => "Add negative volume",
            _ => "Add support volume",
        });
        self.plates.current_mut().objects[i].volumes.push(v);
        self.sync_instances(i);
        self.sub = Some(Sub::Volume(id));
        self.editor.object_mode = true;
        self.changed();
    }

    /// A new height range above the last one (or from the bottom), 5 mm tall, selected.
    pub(crate) fn add_height_range(&mut self) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let (min, max) = o.bounds();
        let height = max.z - min.z;
        let start = o.height_ranges.iter().map(|r| r.max_z).fold(0f32, f32::max);
        if start >= height - 0.2 {
            self.status = "The height ranges already reach the top of the object".into();
            return;
        }
        let end = (start + 5.).min(height);
        let mut r = HeightRange::new(start, end);
        r.overrides.insert("layer_height".into(), "0.1".into());
        let id = r.id;
        self.record("Add height range");
        let o = &mut self.plates.current_mut().objects[i];
        o.height_ranges.push(r);
        o.normalize_ranges();
        self.sync_instances(i);
        self.sub = Some(Sub::Range(id));
        self.editor.object_mode = true;
        self.changed();
    }

    /// Tab / Shift+Tab: the primary object's next (previous) part in tree order, and back to
    /// the object itself after the last one; the tree and the sidebar scope follow.
    pub(crate) fn cycle_part(&mut self, back: bool) {
        let Some(o) = self
            .selected
            .and_then(|i| self.plates.current().objects.get(i))
        else {
            return;
        };
        self.sub = crate::tree::cycle(o, self.sub, back);
        self.editor.object_mode = true;
    }

    /// Deletes the selected volume or range.
    pub(crate) fn delete_sub(&mut self) {
        let (Some(i), Some(sub)) = (self.selected, self.sub) else {
            return;
        };
        if self.plate_locked() {
            return;
        }
        self.record(match sub {
            Sub::Volume(_) => "Delete volume",
            Sub::Range(_) => "Delete height range",
        });
        let o = &mut self.plates.current_mut().objects[i];
        match sub {
            Sub::Volume(id) => o.volumes.retain(|v| v.id != id),
            Sub::Range(id) => o.height_ranges.retain(|r| r.id != id),
        }
        self.sync_instances(i);
        self.sub = None;
        self.changed();
    }

    /// Splits the primary object by connected components: into objects, or into parts of one
    /// object (each part becomes a model volume of the regions model).
    pub(crate) fn split(&mut self, to_parts: bool) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let Some(o) = self.plates.current().objects.get(i).cloned() else {
            return;
        };
        let placed = o.placed_mesh();
        let pieces = mica_core::mesh_edit::connected_components(&placed);
        if pieces.len() < 2 {
            self.status = "The object is one connected piece".into();
            return;
        }
        self.record(if to_parts {
            "Split to parts"
        } else {
            "Split to objects"
        });
        let stem = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        if to_parts {
            let model = mica_core::regions::RegionModel {
                volumes: pieces
                    .iter()
                    .map(|m| mica_core::regions::Volume {
                        mesh: m.clone(),
                        kind: PartKind::Model,
                        config: mica_core::config::Config::new(),
                        painting: Default::default(),
                    })
                    .collect(),
                height_ranges: vec![],
                ..Default::default()
            };
            let merged = mica_core::mesh_edit::merge(&pieces.iter().collect::<Vec<_>>());
            let mut n = Object::from_mesh(o.name.clone(), merged, &o);
            n.object_id = o.object_id;
            n.parts = (1..=pieces.len())
                .map(|k| (format!("{stem} part {k}"), "Model".to_string()))
                .collect();
            n.region = Some(std::sync::Arc::new(model));
            n.height_ranges = o.height_ranges.clone();
            n.volumes = o.volumes.iter().map(Volume::fresh).collect();
            // The volumes were placed in the old mesh frame; the new mesh is in world
            // coordinates, so take them through the old transform.
            for v in &mut n.volumes {
                let t = o.transform() * v.local_transform();
                let (s, r, p) = t.to_scale_rotation_translation();
                v.position = p;
                v.rotation = r;
                v.scale = s;
            }
            self.plates.current_mut().objects[i] = n;
            self.select(Some(i), false);
        } else {
            let first_id = self.next_object_id();
            let mut made = vec![];
            for (k, m) in pieces.into_iter().enumerate() {
                let mut n = Object::from_mesh(format!("{stem} {}", k + 1), m, &o);
                n.object_id = first_id + k;
                n.volumes = vec![];
                n.height_ranges = vec![];
                if k == 0 {
                    self.plates.current_mut().objects[i] = n;
                    made.push(i);
                } else {
                    self.plates.current_mut().objects.push(n);
                    made.push(self.plates.current().objects.len() - 1);
                }
            }
            self.select_many(&made, false);
        }
        self.changed();
    }

    /// "Export as STL…": the selection (placed, merged) into a binary STL.
    pub(crate) fn export_stl(&mut self) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        let objects = &self.plates.current().objects;
        let name = objects[sel[0]].name.clone();
        let stem = std::path::Path::new(&name)
            .file_stem()
            .map_or(name.clone(), |s| s.to_string_lossy().into_owned());
        let meshes: Vec<_> = sel.iter().map(|&i| objects[i].placed_mesh()).collect();
        let merged = mica_core::mesh_edit::merge(&meshes.iter().collect::<Vec<_>>());
        if self.headless {
            self.status = format!("Export as STL: {} triangles", merged.faces.len());
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}.stl"))
            .add_filter("STL", &["stl"])
            .save_file()
        else {
            return;
        };
        let result = std::fs::File::create(&path).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            mica_core::mesh_edit::write_stl(&merged, &stem, &mut w)
        });
        match result {
            Ok(()) => self.notify_info(format!("Exported {}", path.display())),
            Err(e) => self.notify_error(
                "The STL was not exported",
                format!("Export to {} failed: {e}", path.display()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Options};
    use mica_profiles::user::Store;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

    fn app(models: &[&str]) -> (eframe::egui::Context, App) {
        let ctx = eframe::egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-cmd-test-{}-{}",
            std::process::id(),
            NEXT_STORE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let app = App::create(
            &ctx,
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
        );
        (ctx, app)
    }

    fn names(app: &App) -> Vec<String> {
        app.plates
            .current()
            .objects
            .iter()
            .map(|o| o.name.clone())
            .collect()
    }

    fn overlaps(app: &App) -> bool {
        let objects = &app.plates.current().objects;
        objects.iter().enumerate().any(|(i, a)| {
            objects.iter().skip(i + 1).any(|b| {
                let (amin, amax) = a.bounds();
                let (bmin, bmax) = b.bounds();
                amin.x < bmax.x && bmin.x < amax.x && amin.y < bmax.y && bmin.y < amax.y
            })
        })
    }

    /// Shift-click builds a selection, toggles out of it, and the primary follows.
    #[test]
    fn selection_toggles_and_keeps_a_primary() {
        let (_ctx, mut app) = app(&["cube20.stl", "hole_block.stl", "cube20.stl"]);
        assert_eq!(app.plates.current().objects.len(), 3);
        app.select(Some(0), false);
        app.select(Some(2), true);
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.selection(), vec![0, 2]);
        app.select(Some(2), true);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.selection(), vec![0]);
        app.select_many(&[0, 1, 2], false);
        assert_eq!(app.selection(), vec![0, 1, 2]);
        app.select(None, false);
        assert!(app.selection().is_empty());
        // A box with toggle adds to what is there.
        app.select(Some(1), false);
        app.select_many(&[2], true);
        assert_eq!(app.selection(), vec![1, 2]);
    }

    /// Copy / paste keeps settings, volumes and ranges, places the copies clear of
    /// everything, gives them new object ids and unique names; one undo entry.
    #[test]
    fn copy_paste_and_duplicate() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        app.select(Some(0), false);
        {
            let o = &mut app.plates.current_mut().objects[0];
            o.overrides.insert("perimeters".into(), "4".into());
            let mut v = Volume::new(
                "dense",
                PartKind::Modifier,
                mica_core::mesh_edit::cuboid([5., 5., 5.]),
            );
            v.overrides.insert("fill_density".into(), "100%".into());
            o.volumes.push(v);
            o.height_ranges.push(HeightRange::new(0., 5.));
        }
        app.run_command(Command::Copy, &ctx);
        assert_eq!(app.clipboard.len(), 1);
        app.run_command(Command::Paste, &ctx);
        assert_eq!(names(&app), ["cube20.stl", "cube20.stl (2)"]);
        let pasted = &app.plates.current().objects[1];
        assert_eq!(
            pasted.overrides.get("perimeters").map(String::as_str),
            Some("4")
        );
        assert_eq!(pasted.volumes.len(), 1);
        assert_eq!(pasted.height_ranges.len(), 1);
        assert_ne!(pasted.object_id, app.plates.current().objects[0].object_id);
        assert_ne!(
            pasted.volumes[0].id,
            app.plates.current().objects[0].volumes[0].id
        );
        assert!(std::sync::Arc::ptr_eq(
            &pasted.mesh,
            &app.plates.current().objects[0].mesh
        ));
        assert!(!overlaps(&app));
        assert_eq!(app.selection(), vec![1]);
        assert_eq!(app.undo.undo_label(), Some("Paste"));
        // Paste at a bed point lands the copy there.
        app.paste(Some(glam::Vec2::new(200., 180.)));
        let (min, max) = app.plates.current().objects[2].bounds();
        let centre = (min + max) * 0.5;
        assert!((centre.x - 200.).abs() < 1e-3 && (centre.y - 180.).abs() < 1e-3);
        // Duplicate makes independent copies next to each selected object.
        app.select_many(&[0, 1], false);
        app.run_command(Command::Duplicate, &ctx);
        assert_eq!(app.plates.current().objects.len(), 5);
        assert_eq!(app.selection(), vec![3, 4]);
        assert!(!overlaps(&app));
        let ids: std::collections::BTreeSet<usize> = app
            .plates
            .current()
            .objects
            .iter()
            .map(|o| o.object_id)
            .collect();
        assert_eq!(ids.len(), 5, "duplicates are independent objects");
        app.undo();
        assert_eq!(app.plates.current().objects.len(), 3);
    }

    /// Instances share the mesh and `object_id`, get consecutive instance ids, sit clear of
    /// the original, and settings edits reach every instance; "−" removes the last one.
    #[test]
    fn instances_share_and_sync() {
        let (ctx, mut app) = app(&["hole_block.stl"]);
        app.select(Some(0), false);
        app.run_command(Command::AddInstance, &ctx);
        app.run_command(Command::AddInstance, &ctx);
        let objects = &app.plates.current().objects;
        assert_eq!(objects.len(), 3);
        assert!(objects.iter().all(|o| o.object_id == objects[0].object_id));
        assert_eq!(
            objects.iter().map(|o| o.instance_id).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(
            objects
                .iter()
                .all(|o| std::sync::Arc::ptr_eq(&o.mesh, &objects[0].mesh))
        );
        assert!(!overlaps(&app));
        assert_eq!(app.instances_of(0), vec![0, 1, 2]);
        // An override on one instance reaches the others.
        app.select(Some(1), false);
        app.apply_edit(
            None,
            crate::panels::editor::Edit::Set {
                key: "perimeters".into(),
                raw: "5".into(),
            },
        );
        app.end_gesture();
        for o in &app.plates.current().objects {
            assert_eq!(o.overrides.get("perimeters").map(String::as_str), Some("5"));
        }
        // A modifier added on one shows on all, with distinct volume ids.
        app.run_command(Command::AddModifier(Shape::Box), &ctx);
        let objects = &app.plates.current().objects;
        assert!(objects.iter().all(|o| o.volumes.len() == 1));
        assert_ne!(objects[0].volumes[0].id, objects[1].volumes[0].id);
        assert!(matches!(app.sub, Some(Sub::Volume(_))));
        // "−" removes the highest instance, never the last.
        app.select(Some(0), false);
        app.run_command(Command::RemoveInstance, &ctx);
        assert_eq!(app.plates.current().objects.len(), 2);
        app.run_command(Command::RemoveInstance, &ctx);
        assert_eq!(app.plates.current().objects.len(), 1);
        app.run_command(Command::RemoveInstance, &ctx);
        assert_eq!(app.plates.current().objects.len(), 1);
        assert!(app.status.contains("Only one instance"));
    }

    /// Group operations act on every selected object: centre, mirror, rotate, tool, move to
    /// plate, delete.
    #[test]
    fn group_commands_act_on_the_whole_selection() {
        let (ctx, mut app) = app(&["cube20.stl", "hole_block.stl", "cube20.stl"]);
        app.select_many(&[0, 1, 2], false);
        app.run_command(Command::Centre, &ctx);
        let mut min = glam::Vec3::splat(f32::INFINITY);
        let mut max = -min;
        for o in &app.plates.current().objects {
            let (a, b) = o.bounds();
            min = min.min(a);
            max = max.max(b);
        }
        let c = (min + max) * 0.5;
        assert!((c.truncate() - app.bed.center().truncate()).length() < 1e-2);
        app.run_command(Command::Mirror(0), &ctx);
        assert!(app.plates.current().objects.iter().all(|o| o.mirror.x < 0.));
        app.run_command(Command::Rotate90, &ctx);
        assert!(
            app.plates
                .current()
                .objects
                .iter()
                .all(|o| (o.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5)
        );
        assert!(!overlaps(&app), "a group turn keeps the layout");
        app.run_command(Command::ChangeTool(0), &ctx);
        // Move two to a new plate: they keep their plate-local positions and the selection
        // follows them there.
        app.select_many(&[0, 2], false);
        let pos = app.plates.current().objects[0].position;
        app.run_command(Command::MoveToPlate(1), &ctx);
        assert_eq!(app.plates.len(), 2);
        assert_eq!(app.plates.current, 1);
        assert_eq!(app.plates.plate(0).objects.len(), 1);
        assert_eq!(app.plates.plate(1).objects.len(), 2);
        assert_eq!(app.plates.plate(1).objects[0].position, pos);
        assert_eq!(app.selection(), vec![0, 1]);
        app.run_command(Command::Delete, &ctx);
        assert!(app.plates.current().objects.is_empty());
        assert_eq!(app.undo.undo_label(), Some("Delete"));
        app.undo();
        assert_eq!(app.plates.current().objects.len(), 2);
    }

    /// Modifiers, negative volumes and height ranges land on the object (and in the engine's
    /// regions model), sized from the object; the range list stays sorted.
    #[test]
    fn volumes_and_ranges_feed_the_regions_model() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        app.select(Some(0), false);
        app.run_command(Command::AddModifier(Shape::Box), &ctx);
        app.run_command(Command::AddNegative(Shape::Cylinder), &ctx);
        app.run_command(Command::AddSupportBlocker(Shape::Sphere), &ctx);
        app.run_command(Command::AddHeightRange, &ctx);
        app.run_command(Command::AddHeightRange, &ctx);
        let o = &app.plates.current().objects[0];
        assert_eq!(o.volumes.len(), 3);
        assert_eq!(o.volumes[0].kind, PartKind::Modifier);
        assert_eq!(
            o.volumes[0]
                .overrides
                .get("fill_density")
                .map(String::as_str),
            Some("100%")
        );
        assert_eq!(o.volumes[1].kind, PartKind::NegativeVolume);
        assert_eq!(o.volumes[2].kind, PartKind::SupportBlocker);
        assert_eq!(o.height_ranges.len(), 2);
        assert_eq!(o.height_ranges[0].label(), "0.0–5.0 mm");
        assert_eq!(o.height_ranges[1].label(), "5.0–10.0 mm");
        let model = o.placed_region().expect("regions model");
        assert_eq!(model.volumes.len(), 4);
        assert_eq!(model.volumes[0].kind, PartKind::Model);
        assert_eq!(model.height_ranges.len(), 2);
        model.validate().expect("valid regions model");
        // The modifier box sits inside the cube.
        let (vmin, vmax) = o.volume_bounds(&o.volumes[0]);
        let (omin, omax) = o.bounds();
        assert!(vmin.x > omin.x && vmax.x < omax.x && vmin.z >= omin.z - 1e-3);
        // Deleting the selected range, then undo.
        assert!(matches!(app.sub, Some(Sub::Range(_))));
        app.run_command(Command::DeleteSub, &ctx);
        assert_eq!(app.plates.current().objects[0].height_ranges.len(), 1);
        app.undo();
        assert_eq!(app.plates.current().objects[0].height_ranges.len(), 2);
    }

    /// Split by connected components: two separate pieces become two objects or two parts.
    #[test]
    fn split_to_objects_and_parts() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        // Two boxes in one mesh.
        let two =
            mica_core::mesh_edit::merge(&[&mica_core::mesh_edit::cuboid([10., 10., 10.]), &{
                let mut b = mica_core::mesh_edit::cuboid([6., 6., 6.]);
                b.translate([30., 0., 0.]);
                b
            }]);
        let like = app.plates.current().objects[0].clone();
        app.plates.current_mut().objects[0] = Object::from_mesh("pair", two, &like);
        app.select(Some(0), false);
        app.run_command(Command::SplitToParts, &ctx);
        let o = &app.plates.current().objects[0];
        assert_eq!(o.parts.len(), 2);
        assert_eq!(o.region.as_ref().unwrap().volumes.len(), 2);
        app.undo();
        app.select(Some(0), false);
        app.run_command(Command::SplitToObjects, &ctx);
        assert_eq!(names(&app), ["pair 1", "pair 2"]);
        assert_eq!(app.selection(), vec![0, 1]);
        let a = app.plates.current().objects[0].mesh.faces.len();
        let b = app.plates.current().objects[1].mesh.faces.len();
        assert_eq!(a + b, 24);
    }

    /// Arrange packs objects without overlaps inside the bed and spills what does not fit
    /// onto a new plate; Arrange all gathers every plate.
    #[test]
    fn arrange_packs_and_spills() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        app.select(Some(0), false);
        for _ in 0..11 {
            app.run_command(Command::Duplicate, &ctx);
        }
        assert_eq!(app.plates.current().objects.len(), 12);
        app.run_command(Command::Arrange, &ctx);
        assert_eq!(app.plates.len(), 1);
        assert!(!overlaps(&app));
        for o in &app.plates.current().objects {
            let (min, max) = o.bounds();
            assert_eq!(
                app.bed.placement(min, max),
                crate::scene::Placement::Inside,
                "{min:?} {max:?} bed {:?} {:?}",
                app.bed.min,
                app.bed.max
            );
        }
        // Huge objects spill onto a second plate.
        for o in &mut app.plates.current_mut().objects {
            o.scale = glam::Vec3::splat(4.);
            o.drop_to_bed();
        }
        app.run_command(Command::Arrange, &ctx);
        assert!(app.plates.len() >= 2, "{} plates", app.plates.len());
        assert!(app.status.contains("did not fit"));
        for p in &app.plates.plates {
            for o in &p.objects {
                let (min, max) = o.bounds();
                assert_eq!(app.bed.placement(min, max), crate::scene::Placement::Inside);
            }
        }
        // Back to small: arranging all plates gathers everything onto one.
        for p in &mut app.plates.plates {
            for o in &mut p.objects {
                o.scale = glam::Vec3::ONE;
                o.drop_to_bed();
            }
        }
        app.arrange_all();
        assert_eq!(app.plates.len(), 1);
        assert_eq!(app.plates.current().objects.len(), 12);
        assert!(!overlaps(&app));
        assert_eq!(app.undo.undo_label(), Some("Arrange all plates"));
    }

    /// Auto-orient lays a tilted cube flat again, in one undo entry.
    #[test]
    fn auto_orient_flattens_a_tilted_cube() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        app.select(Some(0), false);
        {
            let o = &mut app.plates.current_mut().objects[0];
            o.set_euler(glam::Vec3::new(0.6, 0.3, 0.));
            o.drop_to_bed();
        }
        let (min, max) = app.plates.current().objects[0].bounds();
        assert!(max.z - min.z > 21., "tilted: {}", max.z - min.z);
        app.run_command(Command::AutoOrient, &ctx);
        let o = &app.plates.current().objects[0];
        let (min, max) = o.bounds();
        assert!(
            (max.z - min.z - 20.).abs() < 1e-2,
            "flat again: {}",
            max.z - min.z
        );
        assert!(o.lowest_z().abs() < 1e-3);
        assert_eq!(app.undo.undo_label(), Some("Auto-orient"));
        assert!(app.status.starts_with("Auto-orient"));
    }

    /// The cut tool splits the selected object into two resting halves, one undo entry.
    #[test]
    fn cut_command_splits_the_object() {
        let (ctx, mut app) = app(&["cube20.stl"]);
        app.select(Some(0), false);
        app.run_command(Command::Cut, &ctx);
        assert!(app.cut.is_some());
        app.cut.as_mut().unwrap().set_z(6.);
        app.apply_cut();
        assert!(app.cut.is_none());
        assert_eq!(app.plates.current().objects.len(), 2);
        assert_eq!(app.selection(), vec![0, 1]);
        assert!(!overlaps(&app));
        let heights: Vec<f32> = app
            .plates
            .current()
            .objects
            .iter()
            .map(|o| {
                let (a, b) = o.bounds();
                b.z - a.z
            })
            .collect();
        assert!(
            (heights[0] - 14.).abs() < 1e-2 && (heights[1] - 6.).abs() < 1e-2,
            "{heights:?}"
        );
        assert_eq!(app.undo.undo_label(), Some("Cut"));
        app.undo();
        assert_eq!(app.plates.current().objects.len(), 1);
    }

    #[test]
    fn shapes_parse_and_size_from_the_object() {
        assert_eq!(Shape::parse("cylinder"), Some(Shape::Cylinder));
        assert_eq!(Shape::parse("Box"), Some(Shape::Box));
        assert_eq!(Shape::parse("torus"), None);
        let size = Vec3::new(60., 30., 48.);
        for s in Shape::ALL {
            let m = s.mesh(size);
            assert!(mica_core::mesh_cut::is_watertight(&m), "{}", s.label());
            let (min, max) = m.bounds();
            assert!(
                max[0] - min[0] <= 66.1 && max[2] - min[2] <= 48.1,
                "{}",
                s.label()
            );
        }
    }
}
