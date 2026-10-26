// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The scene an `mica mcp` session works on: one project (an STL or a 3MF, loaded as
//! `mica slice --export-3mf` loads it), the printer profiles it was opened with, one colour
//! painting selector per model part (the GUI's `TriangleSelector`), the rendered views the
//! AI can point into, and an undo stack with one entry per tool call. The input file is
//! never written; projects are saved to a new path and sliced from a temporary copy.

use super::colors;
use super::render::{self, Affine, Camera, DrawPart, Options, Rendered};
use crate::{SliceArgs, project_export, project_settings};
use mica_core::config::Config;
use mica_core::fdm_mixer;
use mica_core::full_spectrum::{FullSpectrum, VirtualExtruder};
use mica_core::painting::FacetStates;
use mica_core::painting::selector::{Brush, Cursor, HeightRange, Spread, TriangleSelector};
use mica_core::threemf::{self, Flavour, PartKind, Project, RawConfig, Settings};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

type V3 = [f32; 3];

/// Most undo steps kept.
const UNDO_LIMIT: usize = 64;
/// Rendered views kept for pointing (each holds a facet buffer of 4 bytes per pixel).
const VIEW_LIMIT: usize = 8;
/// Largest share of a part one fill may cover without `allow_large`: on a smooth model a
/// fill only stops at sharp edges, and one click can repaint a whole hull.
pub const LARGE_FILL: f64 = 0.40;

/// How `paint_at` paints (the tool's optional arguments).
#[derive(Clone, Debug)]
pub struct PaintMode {
    pub mode: String,
    /// Smart fill: the angle between neighbouring facets it crosses, degrees.
    pub angle: f32,
    /// Sphere radius, mm.
    pub radius: f32,
    /// Smart fill and connected: how far from the clicked point, along the surface, mm
    /// (0 = no limit).
    pub max_distance: f32,
    /// Lets one fill cover more than [`LARGE_FILL`] of the part.
    pub allow_large: bool,
}

impl Default for PaintMode {
    fn default() -> Self {
        Self {
            mode: "smart_fill".into(),
            angle: 30.,
            radius: 3.,
            max_distance: 0.,
            allow_large: false,
        }
    }
}

/// How `open` chooses the printer: a shortcut (`U1`, `XL`, `CORE One`) or exact preset names.
#[derive(Clone, Debug, Default)]
pub struct Profiles {
    pub printer: Option<String>,
    pub vendor: Option<String>,
    pub print: Option<String>,
    pub filament: Option<String>,
    pub bed: Option<String>,
}

impl Profiles {
    /// Opens `model` with these profiles.
    pub fn open(&self, model: &Path) -> Result<Scene, String> {
        Scene::open(self.slice_args(model)?)
    }

    /// The `mica slice` arguments for `model` with these profiles.
    pub(crate) fn slice_args(&self, model: &Path) -> Result<SliceArgs, String> {
        let mut args = vec![model.to_string_lossy().into_owned()];
        let mut set = |flag: &str, value: &str| {
            if let Some(i) = args.iter().position(|a| a == flag) {
                args[i + 1] = value.into();
            } else {
                args.extend([flag.to_string(), value.to_string()]);
            }
        };
        if let Some(printer) = &self.printer {
            let key: String = printer
                .to_ascii_lowercase()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            match key.as_str() {
                "u1" | "snapmakeru1" => {
                    set("--vendor", "Snapmaker");
                    set("--printer", "Snapmaker U1 (0.4 nozzle)");
                    set("--print", "0.20 Standard @Snapmaker U1 (0.4 nozzle)");
                    set("--filament", "Snapmaker PLA @U1");
                    set("--bed", "High Temp Plate");
                }
                "xl" | "prusaxl" | "xl5t" | "prusaxl5t" => {
                    set(
                        "--printer",
                        "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
                    );
                    set("--print", "0.20mm SPEED @XLIS 0.4");
                    set("--filament", "Prusament PLA @XLIS");
                }
                "coreone" | "prusacoreone" => {}
                _ => set("--printer", printer),
            }
        }
        for (flag, value) in [
            ("--vendor", &self.vendor),
            ("--print", &self.print),
            ("--filament", &self.filament),
            ("--bed", &self.bed),
        ] {
            if let Some(v) = value {
                set(flag, v);
            }
        }
        crate::parse_slice(&args)
    }
}

/// One placement of a model part in a view (the facet numbering of [`Rendered`]).
#[derive(Clone, Debug)]
struct Slot {
    object: usize,
    part: usize,
    instance: usize,
    /// Maps the instance's bed space back to the first instance's (where the selector lives).
    to_first: Option<Affine>,
}

pub struct View {
    pub id: u32,
    pub camera: Camera,
    rendered: Rendered,
    slots: Vec<Slot>,
}

struct Snapshot {
    label: String,
    /// Parts per object: `add_shape` only appends, so undo truncates back to this.
    shape: Vec<usize>,
    paint: Vec<Vec<FacetStates>>,
    object_settings: Vec<Settings>,
    part_settings: Vec<Vec<Settings>>,
    colors: Vec<[u8; 3]>,
    mixes_json: Option<String>,
}

pub struct Scene {
    pub source: PathBuf,
    args: SliceArgs,
    config: Config,
    original: Option<RawConfig>,
    selected_orca: Option<mica_profiles::orca::RawConfig>,
    project: Project,
    pub colors: Vec<[u8; 3]>,
    /// Live mode supplies the window's current switch before each call.
    pub color_mix_enabled: bool,
    /// Colour painting per model part (`None` for modifiers and other volumes).
    selectors: Vec<Vec<Option<TriangleSelector>>>,
    undo: Vec<Snapshot>,
    views: VecDeque<View>,
    next_view: u32,
    pub edited: bool,
    /// Bumped by every edit that changed something and every undo (live mode's "did this
    /// call change the scene").
    revision: u64,
}

/// What a painting call reports back.
pub struct Painted {
    pub object: usize,
    pub part: usize,
    pub triangles: i64,
    pub area: f64,
}

fn affine(t: &threemf::Transform) -> Affine {
    t.0
}

/// Column-major 4 × 4 (the selector's transform) from a row-major 3 × 4.
fn columns(t: &threemf::Transform) -> [[f32; 4]; 4] {
    std::array::from_fn(|c| {
        std::array::from_fn(|r| {
            if r < 3 {
                t.0[r][c] as f32
            } else if c == 3 {
                1.
            } else {
                0.
            }
        })
    })
}

fn tri_area(t: [V3; 3]) -> f64 {
    let a: [f64; 3] = std::array::from_fn(|i| f64::from(t[1][i] - t[0][i]));
    let b: [f64; 3] = std::array::from_fn(|i| f64::from(t[2][i] - t[0][i]));
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    0.5 * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
}

fn tool_setting(s: &Settings) -> Option<usize> {
    s.get("extruder")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|t| *t > 0)
}

fn round(v: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (v * f).round() / f
}

fn round3(p: V3) -> [f64; 3] {
    p.map(|v| round(f64::from(v), 2))
}

impl Scene {
    /// Opens `args.model` with the profiles in `args`, as `mica slice --export-3mf` would load it.
    pub(crate) fn open(mut args: SliceArgs) -> Result<Self, String> {
        let source = args.model.clone();
        if !source.is_file() {
            return Err(format!("{}: no such file", source.display()));
        }
        let ext = source
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "stl" && ext != "3mf" {
            return Err("open expects an .stl or .3mf file".into());
        }
        // Resolving for an export also prepares the Orca project settings a save needs.
        args.export_3mf = Some(PathBuf::from("mica-mcp.3mf"));
        let resolved = project_settings::resolve(&args)?;
        args.export_3mf = None;
        let mut config = resolved.config;
        crate::apply_slicing_rules(&args, &mut config)?;
        let center = mica_core::input::bed_center(&config);
        let project = project_export::build(&args, center, resolved.original.as_ref())?;
        if project.objects.is_empty() {
            return Err(format!("{}: no objects to paint", source.display()));
        }
        let colors = project_settings::colours(&config)
            .iter()
            .map(|c| mica_core::color::parse_hex(c).unwrap_or([0xF5; 3]))
            .collect();
        let mut scene = Self {
            source,
            args,
            config,
            original: resolved.original,
            selected_orca: resolved.selected_orca,
            project,
            colors,
            color_mix_enabled: mica_profiles::features::Features::load().0.color_mix,
            selectors: vec![],
            undo: vec![],
            views: VecDeque::new(),
            next_view: 1,
            edited: false,
            revision: 0,
        };
        scene.selectors = (0..scene.project.objects.len())
            .map(|o| {
                (0..scene.project.objects[o].parts.len())
                    .map(|p| scene.build_selector(o, p))
                    .collect()
            })
            .collect();
        Ok(scene)
    }

    fn build_selector(&self, o: usize, p: usize) -> Option<TriangleSelector> {
        let object = &self.project.objects[o];
        let part = &object.parts[p];
        if part.kind != PartKind::Model || part.mesh.faces.is_empty() {
            return None;
        }
        let t = object
            .instances
            .first()
            .map(|i| i.transform)
            .unwrap_or_default();
        let states = &part.painting.material;
        let mut sel = TriangleSelector::new(&part.mesh, &columns(&t), states.reversed());
        sel.load(states);
        Some(sel)
    }

    pub fn tools(&self) -> usize {
        self.config.vector_len("nozzle_diameter").max(1)
    }

    /// Project recipes, using the same 3MF data and validation as the GUI.
    pub fn mixes(&self) -> Result<FullSpectrum, String> {
        self.project
            .full_spectrum_json
            .as_deref()
            .map(|s| FullSpectrum::from_json(s, self.tools()))
            .unwrap_or_else(|| Ok(FullSpectrum::default()))
    }

    /// Stores recipes with predicted swatches, exactly as the GUI saves its list.
    pub fn set_mixes(&mut self, mixes: &FullSpectrum) -> Result<(), String> {
        let mut list = mixes.extruders().to_vec();
        for mix in &mut list {
            mix.color = Some(fdm_mixer::rgb_to_hex(self.mix_color(mix)));
        }
        let spectrum = FullSpectrum::new(list, self.tools())?;
        let physical = self
            .colors
            .iter()
            .map(|c| fdm_mixer::rgb_to_hex(*c))
            .collect::<Vec<_>>();
        self.project.full_spectrum_json =
            (!spectrum.extruders().is_empty()).then(|| spectrum.to_json(&physical));
        Ok(())
    }

    pub fn mix_color(&self, mix: &VirtualExtruder) -> [u8; 3] {
        mix.predicted_rgb(&self.colors)
    }

    /// Physical slots followed by virtual tool swatches; never extends the profile vectors.
    fn palette(&self) -> Vec<[u8; 3]> {
        let mut out = self.colors.clone();
        if let Ok(mixes) = self.mixes() {
            for mix in mixes.extruders() {
                out.resize(out.len().max(mix.id), [128; 3]);
                out[mix.id - 1] = if self.color_mix_enabled {
                    self.mix_color(mix)
                } else {
                    self.colors[mix.main_extruder() - 1]
                };
            }
        }
        out
    }

    pub fn logical_tools(&self) -> usize {
        self.tools().max(self.mixes().map_or(0, |m| m.max_id()))
    }

    fn valid_tool(&self, tool: usize) -> bool {
        (1..=self.tools()).contains(&tool)
            || (self.color_mix_enabled && self.mixes().is_ok_and(|m| m.contains(tool)))
    }

    fn printer_name(&self) -> String {
        self.config
            .string("printer_settings_id")
            .filter(|s| !s.is_empty())
            .map_or_else(|| self.args.printer.clone(), str::to_string)
    }

    /// The tool (one based) of a part's unpainted surface.
    pub fn part_tool(&self, o: usize, p: usize) -> usize {
        let object = &self.project.objects[o];
        tool_setting(&object.parts[p].settings)
            .or_else(|| tool_setting(&object.settings))
            .unwrap_or(1)
    }

    // ----- addressing ----------------------------------------------------------------------

    /// A zero-based object index from the one-based argument (default: the only object).
    pub fn object_index(&self, object: Option<u32>) -> Result<usize, String> {
        let n = self.project.objects.len();
        match object {
            None if n == 1 => Ok(0),
            None => Err(format!(
                "this scene has {n} objects; pass object (1..={n}, see scene_info)"
            )),
            Some(i) if i >= 1 && (i as usize) <= n => Ok(i as usize - 1),
            Some(i) => Err(format!("object {i} does not exist; objects are 1..={n}")),
        }
    }

    fn part_indices(&self, o: usize, part: Option<u32>) -> Result<Vec<usize>, String> {
        let parts = &self.project.objects[o].parts;
        match part {
            None => Ok((0..parts.len())
                .filter(|&p| self.selectors[o][p].is_some())
                .collect()),
            Some(i) if i >= 1 && (i as usize) <= parts.len() => {
                let p = i as usize - 1;
                if self.selectors[o][p].is_none() {
                    return Err(format!(
                        "part {i} of object {} is a {} volume, not a model part; it cannot be painted",
                        o + 1,
                        kind_name(&parts[p].kind)
                    ));
                }
                Ok(vec![p])
            }
            Some(i) => Err(format!(
                "part {i} does not exist; object {} has parts 1..={}",
                o + 1,
                parts.len()
            )),
        }
    }

    /// The facet state for a one-based tool (0 erases back to the part's own tool).
    fn state(&self, tool: u32) -> Result<u8, String> {
        let tools = self.tools();
        if tools < 2 && tool != 0 {
            return Err(format!(
                "{} has one tool, so color painting has no effect; open the model with a \
                 multi-tool printer (printer \"U1\" or \"XL\")",
                self.printer_name()
            ));
        }
        if tool != 0 && !self.valid_tool(tool as usize) {
            return Err(format!(
                "tool {tool} does not exist; this printer has tools 1..={tools} (0 erases paint)"
            ));
        }
        Ok(tool as u8)
    }

    // ----- undo ----------------------------------------------------------------------------

    fn snapshot(&self, label: &str) -> Snapshot {
        Snapshot {
            label: label.into(),
            shape: self.project.objects.iter().map(|o| o.parts.len()).collect(),
            paint: self
                .project
                .objects
                .iter()
                .map(|o| {
                    o.parts
                        .iter()
                        .map(|p| p.painting.material.clone())
                        .collect()
                })
                .collect(),
            object_settings: self
                .project
                .objects
                .iter()
                .map(|o| o.settings.clone())
                .collect(),
            part_settings: self
                .project
                .objects
                .iter()
                .map(|o| o.parts.iter().map(|p| p.settings.clone()).collect())
                .collect(),
            colors: self.colors.clone(),
            mixes_json: self.project.full_spectrum_json.clone(),
        }
    }

    fn same(&self, s: &Snapshot) -> bool {
        let now = self.snapshot("");
        now.shape == s.shape
            && now.paint == s.paint
            && now.object_settings == s.object_settings
            && now.part_settings == s.part_settings
            && now.colors == s.colors
            && now.mixes_json == s.mixes_json
    }

    /// Runs one edit as one undo step; an edit that changed nothing leaves no step.
    pub fn edit<T>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<(T, bool), String> {
        let before = self.snapshot(label);
        let result = f(self);
        let changed = !self.same(&before);
        match result {
            Ok(v) => {
                if changed {
                    self.undo.push(before);
                    if self.undo.len() > UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.edited = true;
                    self.revision += 1;
                }
                Ok((v, changed))
            }
            Err(e) => {
                if changed {
                    self.restore(before);
                }
                Err(e)
            }
        }
    }

    fn restore(&mut self, s: Snapshot) {
        if self.snapshot("").shape != s.shape {
            // Added objects and parts go; views may show them, so they go too.
            self.project.objects.truncate(s.shape.len());
            self.selectors.truncate(s.shape.len());
            for (o, &n) in s.shape.iter().enumerate() {
                self.project.objects[o].parts.truncate(n);
                self.selectors[o].truncate(n);
            }
            let n = self.project.objects.len();
            for plate in &mut self.project.plates {
                plate.instances.retain(|i| i.object < n);
            }
            self.views.clear();
        }
        for (o, parts) in s.paint.into_iter().enumerate() {
            for (p, states) in parts.into_iter().enumerate() {
                if self.project.objects[o].parts[p].painting.material != states {
                    self.project.objects[o].parts[p].painting.material = states;
                    self.selectors[o][p] = self.build_selector(o, p);
                }
            }
        }
        for (o, settings) in s.object_settings.into_iter().enumerate() {
            self.project.objects[o].settings = settings;
        }
        for (o, parts) in s.part_settings.into_iter().enumerate() {
            for (p, settings) in parts.into_iter().enumerate() {
                self.project.objects[o].parts[p].settings = settings;
            }
        }
        self.project.full_spectrum_json = s.mixes_json;
        self.set_colors(s.colors);
    }

    /// Renames the only object (a scene opened from a generated shape's STL).
    pub fn rename_object(&mut self, o: usize, name: &str) {
        if let Some(obj) = self.project.objects.get_mut(o) {
            obj.name = name.to_string();
            for p in &mut obj.parts {
                p.name = name.to_string();
            }
        }
    }

    /// Moves every instance of object `o` so its bounding box's XY middle is at `at`.
    pub fn move_object_to(&mut self, o: usize, at: [f32; 2]) -> Result<(), String> {
        let (lo, hi) = self.bounds(&[o]).ok_or("the object has no geometry")?;
        let d = [
            f64::from(at[0] - (lo[0] + hi[0]) / 2.),
            f64::from(at[1] - (lo[1] + hi[1]) / 2.),
        ];
        for inst in &mut self.project.objects[o].instances {
            inst.transform.0[0][3] += d[0];
            inst.transform.0[1][3] += d[1];
        }
        for p in 0..self.project.objects[o].parts.len() {
            self.selectors[o][p] = self.build_selector(o, p);
        }
        self.views.clear();
        Ok(())
    }

    /// `add_shape`: `model` at `size` as a new object (at `position`, else the bed's middle
    /// when it is empty, else beside the others) or, with `kind`, as a part of `object`
    /// (centred in it; a model part stands on the bed beside it). Call it through
    /// [`Scene::edit`] for its undo step.
    pub fn add_shape(
        &mut self,
        model: mica_core::builtin_models::BuiltinModel,
        size: [f32; 3],
        position: Option<[f32; 2]>,
        kind: Option<PartKind>,
        object: Option<u32>,
    ) -> Result<Value, String> {
        let mut mesh = model.mesh_sized(size)?;
        let next_id = self
            .project
            .objects
            .iter()
            .flat_map(|o| std::iter::once(o.source.id).chain(o.parts.iter().map(|p| p.source.id)))
            .max()
            .unwrap_or(0)
            + 1;
        let source = threemf::ResourceId {
            path: "3D/3dmodel.model".into(),
            id: next_id,
        };
        let part = |mesh, kind: PartKind| threemf::Part {
            name: model.name().into(),
            kind,
            source: source.clone(),
            mesh,
            source_mesh: None,
            painting: Default::default(),
            transform: threemf::Transform::IDENTITY,
            settings: Settings::new(),
            metadata: Settings::new(),
        };
        match kind {
            None => {
                let all: Vec<usize> = (0..self.project.objects.len()).collect();
                let at = match (position, self.bounds(&all)) {
                    (Some(p), _) => p,
                    (None, None) => {
                        let c = mica_core::input::bed_center(&self.config);
                        [c.0, c.1]
                    }
                    (None, Some((lo, hi))) => [hi[0] + 6. + size[0] / 2., (lo[1] + hi[1]) / 2.],
                };
                mesh.translate([at[0], at[1], 0.]);
                let plate = self.project.plates.first().map(|p| p.id);
                let o = self.project.objects.len();
                self.project.objects.push(threemf::Object {
                    height_ranges: vec![],
                    layer_height_profile: vec![],
                    brim_points: vec![],
                    name: model.name().into(),
                    source: source.clone(),
                    parts: vec![part(mesh, PartKind::Model)],
                    instances: vec![threemf::Instance {
                        transform: threemf::Transform::IDENTITY,
                        printable: true,
                        plate,
                    }],
                    settings: Settings::new(),
                    metadata: Settings::new(),
                });
                if let Some(p) = self.project.plates.first_mut() {
                    p.instances.push(threemf::PlateInstance {
                        object: o,
                        instance: 0,
                    });
                }
                self.selectors.push(vec![self.build_selector(o, 0)]);
                Ok(json!({
                    "added": model.name(),
                    "kind": "object",
                    "object": o + 1,
                    "center_mm": [round(f64::from(at[0]), 2), round(f64::from(at[1]), 2)],
                    "size_mm": round3(size),
                }))
            }
            Some(kind) => {
                let o = self.object_index(object)?;
                let (lo, hi) = self.bounds(&[o]).ok_or("that object has no geometry")?;
                let world: [f32; 3] = if kind == PartKind::Model {
                    [
                        hi[0] + size[0] / 2.,
                        (lo[1] + hi[1]) / 2.,
                        lo[2] + size[2] / 2.,
                    ]
                } else {
                    std::array::from_fn(|k| (lo[k] + hi[k]) / 2.)
                };
                // Centred on `world`, then into the object's frame (its first instance).
                for v in &mut mesh.vertices {
                    v[0] += world[0];
                    v[1] += world[1];
                    v[2] += world[2] - size[2] / 2.;
                }
                let t = self.project.objects[o]
                    .instances
                    .first()
                    .map(|i| i.transform)
                    .unwrap_or_default();
                let inv = t
                    .inverse()
                    .ok_or("the object's placement cannot be inverted")?;
                for v in &mut mesh.vertices {
                    *v = inv.apply(*v);
                }
                if inv.determinant() < 0. {
                    for f in &mut mesh.faces {
                        f.swap(1, 2);
                    }
                }
                let mut p = part(mesh, kind.clone());
                if kind == PartKind::Modifier {
                    p.settings.insert("fill_density".into(), "100%".into());
                }
                self.project.objects[o].parts.push(p);
                let n = self.project.objects[o].parts.len() - 1;
                let sel = self.build_selector(o, n);
                self.selectors[o].push(sel);
                Ok(json!({
                    "added": model.name(),
                    "kind": kind_name(&kind),
                    "object": o + 1,
                    "part": n + 1,
                    "size_mm": round3(size),
                }))
            }
        }
    }

    /// Undoes the last edit; returns its label.
    pub fn undo(&mut self) -> Result<String, String> {
        let s = self.undo.pop().ok_or("nothing to undo")?;
        let label = s.label.clone();
        self.restore(s);
        self.edited = true;
        self.revision += 1;
        Ok(label)
    }

    pub fn undo_steps(&self) -> usize {
        self.undo.len()
    }

    /// Changes with every edit that changed something and every undo.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The project as edited (painting in each model part's `painting.material`, tools in
    /// the `extruder` settings).
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The id the next rendered view gets.
    pub fn next_view_id(&self) -> u32 {
        self.next_view
    }

    /// Numbers this scene's views from `next` on, so a scene that replaces another (live mode
    /// syncing the window again) never reuses an id the AI still holds.
    pub fn continue_view_ids(&mut self, next: u32) {
        self.next_view = self.next_view.max(next);
    }

    // ----- painting ------------------------------------------------------------------------

    fn store(&mut self, o: usize, p: usize) {
        if let Some(sel) = &self.selectors[o][p] {
            self.project.objects[o].parts[p].painting.material = sel.serialize();
        }
    }

    /// Area (mm²) and leaf count with exactly this state, over parts.
    fn state_totals(&self, o: usize, parts: &[usize], state: u8) -> (f64, i64) {
        let mut area = 0.;
        let mut leaves = 0;
        for &p in parts {
            if let Some(sel) = &self.selectors[o][p] {
                sel.world_leaves(true, |_, s, t| {
                    if s == state {
                        area += tri_area(t);
                        leaves += 1;
                    }
                });
            }
        }
        (area, leaves)
    }

    /// Runs a selector edit on parts of object `o` and reports what took the state.
    fn paint_parts(
        &mut self,
        o: usize,
        parts: &[usize],
        state: u8,
        mut f: impl FnMut(&mut TriangleSelector),
    ) -> Painted {
        let (a0, l0) = self.state_totals(o, parts, state);
        for &p in parts {
            if let Some(sel) = self.selectors[o][p].as_mut() {
                f(sel);
            }
            self.store(o, p);
        }
        let (a1, l1) = self.state_totals(o, parts, state);
        Painted {
            object: o,
            part: parts.first().copied().unwrap_or(0),
            triangles: (l1 - l0).max(0),
            area: (a1 - a0).max(0.),
        }
    }

    /// Paints at a pixel of a rendered view.
    pub fn paint_at(
        &mut self,
        view_id: u32,
        x: u32,
        y: u32,
        tool: u32,
        how: &PaintMode,
    ) -> Result<(Painted, Value), String> {
        let state = self.state(tool)?;
        let hit = self.hit(view_id, x, y, true)?;
        let (o, p, facet) = (hit.object, hit.part, hit.facet);
        let fill_angle = match how.mode.as_str() {
            "smart_fill" | "fill" => Some(how.angle.clamp(0., 180.)),
            "connected" => Some(180.),
            _ => None,
        };
        if !(how.max_distance.is_finite() && how.max_distance >= 0.) {
            return Err("max_distance is a distance in mm (0 = no limit)".into());
        }
        let painted = match how.mode.as_str() {
            "smart_fill" | "fill" | "connected" => {
                let angle = fill_angle.unwrap_or(30.);
                let spread = (how.max_distance > 0.).then_some(Spread {
                    origin: hit.point,
                    max_distance: how.max_distance,
                });
                // Erasing cannot add colour changes; painting more than a large share of
                // the part in one click is usually a fill that ran away.
                if state != 0 && !how.allow_large {
                    self.check_fill_size(o, p, facet, angle, hit.point, spread)?;
                }
                self.paint_parts(o, &[p], state, |s| {
                    s.fill_within(facet, angle, state, None, spread)
                })
            }
            "triangle" => self.paint_parts(o, &[p], state, |s| s.set_facet(facet, state)),
            "sphere" => {
                let radius = how.radius.clamp(0.2, 200.);
                let camera = std::array::from_fn(|i| hit.point[i] - hit.dir[i] * 1000.);
                let cursor = Cursor::new(Brush::Sphere, hit.point, camera, radius);
                self.paint_parts(o, &[p], state, |s| {
                    s.paint(facet, &cursor, state, true, None)
                })
            }
            other => {
                return Err(format!(
                    "unknown mode '{other}'; use smart_fill, connected, triangle or sphere"
                ));
            }
        };
        Ok((painted, hit.describe(self)))
    }

    /// Refuses a fill that would cover more than [`LARGE_FILL`] of its part, with the area
    /// it would cover and spread limits that cover less.
    fn check_fill_size(
        &self,
        o: usize,
        p: usize,
        facet: usize,
        angle: f32,
        origin: V3,
        spread: Option<Spread>,
    ) -> Result<(), String> {
        let Some(sel) = &self.selectors[o][p] else {
            return Ok(());
        };
        let mut probe = sel.blank();
        probe.fill_within(facet, angle, 1, None, spread);
        let (mut covered, mut total) = (0., 0.);
        probe.world_leaves(true, |_, s, t| {
            let a = tri_area(t);
            total += a;
            if s == 1 {
                covered += a;
            }
        });
        let share = covered / total.max(1e-9);
        if share <= LARGE_FILL {
            return Ok(());
        }
        // The spread limits that cover a tenth, a fifth and 30 % of the part.
        let reach = sel.fill_reach(facet, angle, None, origin);
        let mut sum = 0.;
        let mut marks: Vec<(f64, f64)> = vec![];
        let mut want = [0.1, 0.2, 0.3].into_iter().peekable();
        for (d, a) in reach {
            sum += f64::from(a);
            while let Some(&w) = want.peek() {
                if sum / total.max(1e-9) < w {
                    break;
                }
                marks.push((w, f64::from(d).max(0.5)));
                want.next();
            }
        }
        let limits: Vec<String> = marks
            .iter()
            .map(|(w, d)| {
                format!(
                    "max_distance {} covers about {:.0}%",
                    round(*d, 1),
                    w * 100.
                )
            })
            .collect();
        let suggested = marks
            .iter()
            .find(|m| m.0 >= 0.2)
            .or(marks.first())
            .map_or(5., |m| round(m.1, 1));
        let part = &self.project.objects[o].parts[p];
        Err(format!(
            "this fill would cover {:.0}% of the part ({} of {} mm² of part {} '{}'), more \
             than the {:.0}% one fill may cover; nothing was painted. A fill only stops at edges \
             sharper than its angle, so on a smooth surface it runs over the whole part. Use \
             max_distance (suggested: {suggested} mm; {}), a height band (paint_height_band) or \
             direction painting (paint_by_direction), or set_part_tool for a whole part. If the \
             whole region is really meant, repeat with allow_large: true.",
            share * 100.,
            round(covered, 0),
            round(total, 0),
            p + 1,
            part.name,
            LARGE_FILL * 100.,
            if limits.is_empty() {
                "a few mm keeps it local".to_string()
            } else {
                limits.join(", ")
            },
        ))
    }

    pub fn paint_height_band(
        &mut self,
        object: Option<u32>,
        z_min: f32,
        z_max: f32,
        tool: u32,
    ) -> Result<Painted, String> {
        let state = self.state(tool)?;
        let o = self.object_index(object)?;
        if !(z_min.is_finite() && z_max.is_finite() && z_min < z_max) {
            return Err("z_min must be below z_max (bed millimetres)".into());
        }
        let parts = self.part_indices(o, None)?;
        let range = HeightRange {
            bottom: z_min,
            top: z_max,
        };
        Ok(self.paint_parts(o, &parts, state, |s| s.paint_height_range(&range, state)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint_by_direction(
        &mut self,
        object: Option<u32>,
        part: Option<u32>,
        direction: &str,
        max_angle: f32,
        tool: u32,
        z_min: Option<f32>,
        z_max: Option<f32>,
    ) -> Result<Painted, String> {
        let state = self.state(tool)?;
        let o = self.object_index(object)?;
        let parts = self.part_indices(o, part)?;
        let max_angle = max_angle.clamp(0., 180.);
        let cos = max_angle.to_radians().cos();
        let sin = max_angle.to_radians().sin();
        let d = direction.trim().to_ascii_lowercase();
        let side = d == "side" || d == "sides";
        let dir: V3 = match d.as_str() {
            "up" | "top" | "+z" => [0., 0., 1.],
            "down" | "bottom" | "-z" => [0., 0., -1.],
            "front" | "-y" => [0., -1., 0.],
            "back" | "+y" => [0., 1., 0.],
            "left" | "-x" => [-1., 0., 0.],
            "right" | "+x" => [1., 0., 0.],
            _ if side => [0., 0., 0.],
            _ => {
                let v: Vec<f32> = d
                    .trim_matches(|c| c == '[' || c == ']' || c == '(' || c == ')')
                    .split(',')
                    .map(|s| s.trim().parse::<f32>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| {
                        format!(
                            "direction '{direction}' is not one of up, down, side, front, back, \
                             left, right or a vector \"x,y,z\""
                        )
                    })?;
                let [x, y, z] = v[..] else {
                    return Err("a direction vector has three numbers, \"x,y,z\"".into());
                };
                let n = (x * x + y * y + z * z).sqrt();
                if !n.is_finite() || n == 0. {
                    return Err("the direction vector must not be zero".into());
                }
                [x / n, y / n, z / n]
            }
        };
        let (lo, hi) = (
            z_min.unwrap_or(f32::NEG_INFINITY),
            z_max.unwrap_or(f32::INFINITY),
        );
        Ok(self.paint_parts(o, &parts, state, |s| {
            for f in 0..s.facet_count() {
                let n = s.normal(f);
                let ok = if side {
                    n[2].abs() <= sin
                } else {
                    n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2] >= cos
                };
                if !ok {
                    continue;
                }
                let t = s.world_triangle(f);
                let z = (t[0][2] + t[1][2] + t[2][2]) / 3.;
                if z >= lo && z <= hi {
                    s.set_facet(f, state);
                }
            }
        }))
    }

    pub fn set_part_tool(
        &mut self,
        object: Option<u32>,
        part: Option<u32>,
        tool: u32,
    ) -> Result<String, String> {
        let tools = self.tools();
        if !self.valid_tool(tool as usize) {
            return Err(format!("tool must be 1..={tools}"));
        }
        let o = self.object_index(object)?;
        let value = tool.to_string();
        let obj = &mut self.project.objects[o];
        match part {
            None => {
                obj.settings.insert("extruder".into(), value.clone());
                for p in &mut obj.parts {
                    if p.kind == PartKind::Model {
                        p.settings.insert("extruder".into(), value.clone());
                    }
                }
                Ok(format!(
                    "object {} ({}) now prints with tool {tool} where it is not painted",
                    o + 1,
                    obj.name
                ))
            }
            Some(_) => {
                let p = self.part_indices(o, part)?[0];
                let obj = &mut self.project.objects[o];
                obj.parts[p]
                    .settings
                    .insert("extruder".into(), value.clone());
                Ok(format!(
                    "part {} ({}) of object {} now prints with tool {tool} where it is not painted",
                    p + 1,
                    obj.parts[p].name,
                    o + 1
                ))
            }
        }
    }

    pub fn clear_paint(
        &mut self,
        object: Option<u32>,
        part: Option<u32>,
    ) -> Result<String, String> {
        let objects = match object {
            None if part.is_some() => vec![self.object_index(None)?],
            None => (0..self.project.objects.len()).collect(),
            Some(_) => vec![self.object_index(object)?],
        };
        let mut cleared = 0;
        for o in objects {
            for p in self.part_indices(o, part)? {
                if let Some(sel) = self.selectors[o][p].as_mut()
                    && sel.has_painting()
                {
                    sel.clear();
                    cleared += 1;
                }
                self.store(o, p);
            }
        }
        Ok(format!("cleared the color paint of {cleared} part(s)"))
    }

    fn set_colors(&mut self, colors: Vec<[u8; 3]>) {
        self.colors = colors;
        let text: Vec<String> = self.colors.iter().map(|c| colors::hex(*c)).collect();
        let _ = self.config.set_raw("filament_colour", &text.join(";"));
    }

    /// Sets slot colours from names or hex ("" keeps a slot).
    pub fn set_filament_colors(&mut self, wanted: &[String]) -> Result<String, String> {
        let tools = self.tools();
        if wanted.len() > tools {
            return Err(format!(
                "{} colors given, but this printer has {tools} tool(s)",
                wanted.len()
            ));
        }
        let mut colors = self.colors.clone();
        colors.resize(tools, [0xF5; 3]);
        for (i, w) in wanted.iter().enumerate() {
            if w.trim().is_empty() {
                continue;
            }
            colors[i] = colors::parse(w).ok_or_else(|| {
                format!(
                    "'{w}' is not a color; use #RRGGBB or one of: {}",
                    colors::known()
                )
            })?;
        }
        let changed = colors != self.colors;
        self.set_colors(colors);
        if changed {
            let mixes = self.mixes()?;
            self.set_mixes(&mixes)?;
        }
        Ok(self
            .colors
            .iter()
            .enumerate()
            .map(|(i, c)| format!("T{} {}", i + 1, colors::name(*c)))
            .collect::<Vec<_>>()
            .join(", "))
    }

    // ----- views ---------------------------------------------------------------------------

    /// Bounds (bed mm) of the given objects' model parts at all instances.
    fn bounds(&self, objects: &[usize]) -> Option<(V3, V3)> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for &o in objects {
            let obj = &self.project.objects[o];
            for inst in &obj.instances {
                for part in obj.parts.iter().filter(|p| p.kind == PartKind::Model) {
                    for v in &part.mesh.vertices {
                        let p = inst.transform.apply(*v);
                        for a in 0..3 {
                            lo[a] = lo[a].min(p[a]);
                            hi[a] = hi[a].max(p[a]);
                        }
                    }
                }
            }
        }
        (lo[0] <= hi[0] && lo.iter().chain(&hi).all(|v| v.is_finite())).then_some((lo, hi))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_view(
        &mut self,
        azimuth: f32,
        elevation: f32,
        size: u32,
        object: Option<u32>,
        zoom: f32,
        center: Option<V3>,
        options: Options,
    ) -> Result<(u32, Vec<u8>, Value), String> {
        let size = size.clamp(render::MIN_SIZE, render::MAX_SIZE);
        let objects = match object {
            Some(_) => vec![self.object_index(object)?],
            None => (0..self.project.objects.len()).collect(),
        };
        let bounds = self.bounds(&objects).ok_or("nothing to render")?;
        if !(azimuth.is_finite() && elevation.is_finite() && zoom.is_finite() && zoom > 0.) {
            return Err("azimuth, elevation and zoom must be finite numbers (zoom > 0)".into());
        }
        let camera = Camera::frame(azimuth, elevation, bounds, size, size, zoom, center);
        let mut slots = vec![];
        let mut draw = vec![];
        for o in 0..self.project.objects.len() {
            let obj = &self.project.objects[o];
            let first = obj
                .instances
                .first()
                .map(|i| i.transform)
                .unwrap_or_default();
            let inv_first = first.inverse();
            for (k, inst) in obj.instances.iter().enumerate() {
                let (to_instance, to_first) = if k == 0 {
                    (None, None)
                } else {
                    let Some(inv) = inv_first else { continue };
                    let m = inst.transform.compose(inv);
                    let Some(back) = m.inverse() else { continue };
                    (Some(affine(&m)), Some(affine(&back)))
                };
                for p in 0..obj.parts.len() {
                    if let Some(sel) = &self.selectors[o][p] {
                        slots.push(Slot {
                            object: o,
                            part: p,
                            instance: k,
                            to_first,
                        });
                        draw.push(DrawPart {
                            selector: sel,
                            base_tool: self.part_tool(o, p).min(255) as u8,
                            to_instance,
                        });
                    }
                }
            }
        }
        let t0 = std::time::Instant::now();
        let mut rendered = render::render(&draw, &self.palette(), &camera, options);
        let render_ms = t0.elapsed().as_secs_f64() * 1000.;
        drop(draw);
        let png =
            mica_gcode::thumbnails::encode_png_rgb(rendered.width, rendered.height, &rendered.rgb)
                .map_err(|e| format!("PNG encoding failed: {e}"))?;
        rendered.rgb = vec![];
        let id = self.next_view;
        self.next_view += 1;
        let view = View {
            id,
            camera,
            rendered,
            slots,
        };
        let info = self.describe_view(&view, render_ms);
        self.views.push_back(view);
        while self.views.len() > VIEW_LIMIT {
            self.views.pop_front();
        }
        Ok((id, png, info))
    }

    fn describe_view(&self, view: &View, render_ms: f64) -> Value {
        let r = &view.rendered;
        let (w, h) = (r.width, r.height);
        // Per placed part: pixel count, bounding box and the model pixel nearest its centroid.
        let n = view.slots.len();
        let mut count = vec![0usize; n];
        let mut bbox = vec![[u32::MAX, u32::MAX, 0, 0]; n];
        let mut sum = vec![[0f64; 2]; n];
        for y in 0..h {
            for x in 0..w {
                if let Some((s, _)) = r.pick(x, y) {
                    count[s] += 1;
                    let b = &mut bbox[s];
                    b[0] = b[0].min(x);
                    b[1] = b[1].min(y);
                    b[2] = b[2].max(x);
                    b[3] = b[3].max(y);
                    sum[s][0] += f64::from(x);
                    sum[s][1] += f64::from(y);
                }
            }
        }
        let mut visible = vec![];
        for s in 0..n {
            if count[s] == 0 {
                continue;
            }
            let c = [sum[s][0] / count[s] as f64, sum[s][1] / count[s] as f64];
            let b = bbox[s];
            let mut best = (f64::INFINITY, [0u32; 2]);
            for y in b[1]..=b[3] {
                for x in b[0]..=b[2] {
                    if r.pick(x, y).is_some_and(|(k, _)| k == s) {
                        let d = (f64::from(x) - c[0]).powi(2) + (f64::from(y) - c[1]).powi(2);
                        if d < best.0 {
                            best = (d, [x, y]);
                        }
                    }
                }
            }
            let slot = &view.slots[s];
            let obj = &self.project.objects[slot.object];
            visible.push(json!({
                "object": slot.object + 1,
                "object_name": obj.name,
                "part": slot.part + 1,
                "part_name": obj.parts[slot.part].name,
                "instance": slot.instance + 1,
                "pixels": count[s],
                "bbox_px": b,
                "center_px": best.1,
            }));
        }
        let model: usize = r.tool_pixels.iter().sum();
        let tools: Vec<Value> = r
            .tool_pixels
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(t, n)| {
                let c = self.palette().get(t).copied().unwrap_or([200; 3]);
                json!({
                    "tool": t + 1,
                    "color": colors::name(c),
                    "share_of_model_pixels_pct": round(100. * *n as f64 / model.max(1) as f64, 1),
                })
            })
            .collect();
        json!({
            "view_id": view.id,
            "width": w,
            "height": h,
            "azimuth": view.camera.azimuth,
            "elevation": view.camera.elevation,
            "mm_per_pixel": round(1. / f64::from(view.camera.scale), 4),
            "visible_parts": visible,
            "visible_tools": tools,
            "render_ms": round(render_ms, 1),
        })
    }

    /// The facet under a pixel; a background pixel snaps to the nearest model pixel within a
    /// few pixels.
    fn hit(&self, view_id: u32, x: u32, y: u32, snap: bool) -> Result<Hit, String> {
        let view = self.views.iter().find(|v| v.id == view_id).ok_or_else(|| {
            let ids: Vec<String> = self.views.iter().map(|v| v.id.to_string()).collect();
            format!(
                "view {view_id} is not available (kept views: {}); call render_view first",
                if ids.is_empty() {
                    "none".into()
                } else {
                    ids.join(", ")
                }
            )
        })?;
        let r = &view.rendered;
        if x >= r.width || y >= r.height {
            return Err(format!(
                "pixel ({x}, {y}) is outside the {}×{} image of view {view_id}",
                r.width, r.height
            ));
        }
        let mut found = r.pick(x, y).map(|h| (h, x, y));
        if found.is_none() && snap {
            const SNAP: i64 = 6;
            let mut best = i64::MAX;
            for dy in -SNAP..=SNAP {
                for dx in -SNAP..=SNAP {
                    let (sx, sy) = (i64::from(x) + dx, i64::from(y) + dy);
                    if sx < 0 || sy < 0 || dx * dx + dy * dy >= best {
                        continue;
                    }
                    if let Some(h) = r.pick(sx as u32, sy as u32) {
                        best = dx * dx + dy * dy;
                        found = Some((h, sx as u32, sy as u32));
                    }
                }
            }
        }
        let ((s, facet), px, py) = found.ok_or_else(|| {
            format!(
                "pixel ({x}, {y}) of view {view_id} shows background, not the model; pick a \
                 pixel on the model (render_view lists each part's bbox_px and center_px)"
            )
        })?;
        let slot = &view.slots[s];
        let sel = self.selectors[slot.object][slot.part]
            .as_ref()
            .ok_or("internal: the view refers to a part without a selector")?;
        let tri = sel.world_triangle(facet);
        let (origin, dir) = view.camera.ray(px as f32 + 0.5, py as f32 + 0.5);
        // Intersect in the first instance's space.
        let (origin, dir) = match &slot.to_first {
            Some(t) => {
                let o2 = render::apply(t, origin);
                let d2 = render::apply(t, std::array::from_fn(|i| origin[i] + dir[i]));
                (o2, std::array::from_fn(|i| d2[i] - o2[i]))
            }
            None => (origin, dir),
        };
        let e1: V3 = std::array::from_fn(|i| tri[1][i] - tri[0][i]);
        let e2: V3 = std::array::from_fn(|i| tri[2][i] - tri[0][i]);
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let denom = n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2];
        let point = if denom.abs() > 1e-12 {
            let t = (n[0] * (tri[0][0] - origin[0])
                + n[1] * (tri[0][1] - origin[1])
                + n[2] * (tri[0][2] - origin[2]))
                / denom;
            std::array::from_fn(|i| origin[i] + dir[i] * t)
        } else {
            std::array::from_fn(|i| (tri[0][i] + tri[1][i] + tri[2][i]) / 3.)
        };
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2])
            .sqrt()
            .max(1e-12);
        Ok(Hit {
            object: slot.object,
            part: slot.part,
            facet,
            point,
            dir: dir.map(|v| v / len),
            normal: sel.normal(facet),
            pixel: [px, py],
            snapped: (px, py) != (x, y),
        })
    }

    /// What is under a pixel (nothing is painted).
    pub fn pick(&self, view_id: u32, x: u32, y: u32) -> Result<Value, String> {
        Ok(self.hit(view_id, x, y, true)?.describe(self))
    }

    /// The bed point and the facet's outward normal exactly under a pixel, if any.
    pub fn pixel_point(&self, view_id: u32, x: u32, y: u32) -> Option<(V3, V3)> {
        let h = self.hit(view_id, x, y, false).ok()?;
        Some((h.point, h.normal))
    }

    /// The tool (one based) currently printing a facet: its painted state, else the part's.
    fn facet_tool(&self, o: usize, p: usize, facet: usize) -> Vec<usize> {
        let base = self.part_tool(o, p);
        let mut tools = vec![];
        if let Some(sel) = &self.selectors[o][p] {
            sel.world_leaves(true, |f, s, _| {
                let t = if s == 0 { base } else { usize::from(s) };
                if f == facet && !tools.contains(&t) {
                    tools.push(t);
                }
            });
        }
        if tools.is_empty() {
            tools.push(base);
        }
        tools
    }

    // ----- reports -------------------------------------------------------------------------

    fn filament_json(&self) -> Vec<Value> {
        (0..self.tools())
            .map(|t| {
                let c = self.colors.get(t).copied().unwrap_or([0xF5; 3]);
                json!({
                    "tool": t + 1,
                    "color": colors::name(c),
                    "hex": colors::hex(c),
                    "filament": self.config.string_at("filament_settings_id", t).unwrap_or(&self.args.filament),
                    "type": self.config.string_at("filament_type", t).unwrap_or(""),
                })
            })
            .collect()
    }

    /// Painted area per tool (index tool - 1), mm², of one part; unknown states go last.
    fn part_areas(&self, o: usize, p: usize) -> (f64, Vec<f64>) {
        let tools = self.logical_tools();
        let base = self.part_tool(o, p);
        let mut by_tool = vec![0.; tools + 1];
        let mut total = 0.;
        if let Some(sel) = &self.selectors[o][p] {
            sel.world_leaves(true, |_, s, t| {
                let tool = if s == 0 { base } else { usize::from(s) };
                let a = tri_area(t);
                total += a;
                by_tool[if (1..=tools).contains(&tool) {
                    tool - 1
                } else {
                    tools
                }] += a;
            });
        }
        (total, by_tool)
    }

    fn area_json(&self, total: f64, by_tool: &[f64]) -> Vec<Value> {
        by_tool
            .iter()
            .enumerate()
            .filter(|(_, a)| **a > 0.)
            .map(|(t, a)| {
                if t < self.logical_tools() {
                    let c = self.palette().get(t).copied().unwrap_or([0xF5; 3]);
                    json!({
                        "tool": t + 1,
                        "color": colors::name(c),
                        "area_mm2": round(*a, 1),
                        "area_pct": round(100. * a / total.max(1e-9), 2),
                    })
                } else {
                    json!({
                        "tool": "other (painted with a tool this printer does not have)",
                        "area_mm2": round(*a, 1),
                        "area_pct": round(100. * a / total.max(1e-9), 2),
                    })
                }
            })
            .collect()
    }

    /// Area percentage per tool for one object (all model parts).
    pub fn object_tool_pct(&self, o: usize) -> Vec<f64> {
        let mut total = 0.;
        let mut by = vec![0.; self.logical_tools() + 1];
        for p in 0..self.project.objects[o].parts.len() {
            let (t, b) = self.part_areas(o, p);
            total += t;
            for (k, a) in b.iter().enumerate() {
                by[k] += a;
            }
        }
        by.iter().map(|a| 100. * a / total.max(1e-9)).collect()
    }

    pub fn info(&self) -> Value {
        let mut objects = vec![];
        for (o, obj) in self.project.objects.iter().enumerate() {
            let (lo, hi) = self.bounds(&[o]).unwrap_or(([0.; 3], [0.; 3]));
            let mut parts = vec![];
            let mut total = 0.;
            let mut by = vec![0.; self.logical_tools() + 1];
            let mut triangles = 0;
            for (p, part) in obj.parts.iter().enumerate() {
                let (t, b) = self.part_areas(o, p);
                total += t;
                for (k, a) in b.iter().enumerate() {
                    by[k] += a;
                }
                triangles += part.mesh.faces.len();
                let (plo, phi) = part.mesh.bounds();
                let painted = self.selectors[o][p]
                    .as_ref()
                    .is_some_and(TriangleSelector::has_painting);
                parts.push(json!({
                    "part": p + 1,
                    "name": part.name,
                    "kind": kind_name(&part.kind),
                    "triangles": part.mesh.faces.len(),
                    "size_mm": round3(std::array::from_fn(|i| phi[i] - plo[i])),
                    "tool": self.part_tool(o, p),
                    "painted": painted,
                    "area_by_tool": self.area_json(t, &b),
                }));
            }
            objects.push(json!({
                "object": o + 1,
                "name": obj.name,
                "instances": obj.instances.len(),
                "triangles": triangles,
                "size_mm": round3(std::array::from_fn(|i| hi[i] - lo[i])),
                "bbox_min_mm": round3(lo),
                "bbox_max_mm": round3(hi),
                "tool": tool_setting(&obj.settings).unwrap_or(1),
                "area_by_tool": self.area_json(total, &by),
                "parts": parts,
            }));
        }
        let distinct = {
            let mut c = self.colors.clone();
            c.sort();
            c.dedup();
            c.len()
        };
        let mut notes = vec![];
        if self.tools() < 2 {
            notes.push(
                "single-tool printer: color painting has no effect; reopen with printer \"U1\" or \"XL\""
                    .to_string(),
            );
        } else if distinct < self.tools().min(self.colors.len()) {
            notes.push(
                "several tools share one color; call set_filament_colors with the colors \
                 actually loaded so renders tell the tools apart"
                    .to_string(),
            );
        }
        json!({
            "file": self.source.display().to_string(),
            "unsaved_changes": self.edited,
            "printer": {
                "name": self.printer_name(),
                "vendor": self.args.vendor,
                "print_profile": self.config.string("print_settings_id").unwrap_or(&self.args.print),
                "tools": self.tools(),
            },
            "filaments": self.filament_json(),
            "mixes": self.mix_info(false),
            "coordinates": "millimetres on the bed; z is up from the bed, -y is the front",
            "objects": objects,
            "undo_steps": self.undo.len(),
            "notes": notes,
        })
    }

    // ----- files ---------------------------------------------------------------------------

    fn default_flavour(&self) -> Flavour {
        if self.args.vendor == "Prusa" {
            Flavour::Prusa
        } else {
            Flavour::Orca
        }
    }

    pub fn same_file(a: &Path, b: &Path) -> bool {
        let canon = |p: &Path| {
            p.canonicalize().ok().or_else(|| {
                let parent = p
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                Some(parent.canonicalize().ok()?.join(p.file_name()?))
            })
        };
        matches!((canon(a), canon(b)), (Some(x), Some(y)) if x == y)
    }

    /// Where `save_project` writes: `path` (`~/` is the home folder, a relative path is taken
    /// from the server's working folder), or without one a new name in the user's Desktop
    /// (Documents without a Desktop, else the home folder), never next to the input.
    pub fn save_path(&self, path: Option<&str>) -> Result<PathBuf, String> {
        let stem = self
            .source
            .file_stem()
            .map_or("model".into(), |s| s.to_string_lossy().into_owned());
        resolve_save_path(path, &stem)
    }

    /// Writes the project (with its painting and tools) to a new 3MF.
    pub fn save(
        &self,
        path: &Path,
        flavour: Option<Flavour>,
        overwrite: bool,
    ) -> Result<String, String> {
        self.save_project(self.project.clone(), path, flavour, overwrite)
    }

    fn save_project(
        &self,
        project: Project,
        path: &Path,
        flavour: Option<Flavour>,
        overwrite: bool,
    ) -> Result<String, String> {
        check_save_target(path, Some(&self.source), overwrite)?;
        let mut a = self.args.clone();
        a.flavour = Some(flavour.unwrap_or_else(|| self.default_flavour()));
        // The slot colours may have changed since the settings were resolved: the saved
        // settings carry the current ones.
        let hexes: Vec<String> = self.colors.iter().map(|c| colors::hex(*c)).collect();
        let original = self.original.as_ref().map(|r| with_colours(r, &hexes));
        let selected = self.selected_orca.clone().map(|mut raw| {
            raw.insert(
                "filament_colour".into(),
                Value::Array(hexes.iter().cloned().map(Value::String).collect()),
            );
            raw
        });
        project_export::finish(
            &a,
            &self.config,
            project,
            original.as_ref(),
            selected.as_ref(),
            path,
        )
    }

    /// Slices a temporary copy of the project as `mica slice` would and summarises it.
    pub fn slice_summary(&self) -> Result<Value, String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mica-mcp-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let result = self.slice_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// Saves `project` into `dir` as `name`.3mf, slices it as `mica slice` would and reads the
    /// G-code back with the validator.
    fn slice_project(
        &self,
        dir: &Path,
        project: Project,
        name: &str,
    ) -> Result<(crate::Sliced, mica_gcode::validate::Report), String> {
        let file = dir.join(format!("{name}.3mf"));
        self.save_project(project, &file, None, true)?;
        let mut a = self.args.clone();
        a.model = file;
        a.extra_models.clear();
        a.output = Some(dir.join(format!("{name}.gcode")));
        a.export_3mf = None;
        a.flavour = None;
        let job = mica_core::job::Boundary::new("profile resolution");
        let sliced = job
            .run(|| crate::slice_job(a, &job))?
            .ok_or("the slicer wrote no G-code")?;
        let profile = mica_gcode::validate::Profile::from_config(&sliced.config)?;
        let report = mica_gcode::validate::validate_file(&sliced.out, &profile)
            .map_err(|e| format!("reading the G-code back: {e}"))?;
        Ok((sliced, report))
    }

    /// The project printed in one color: no color paint, every object and part on tool 1.
    fn single_colour_project(&self) -> Project {
        let mut project = self.project.clone();
        let reset = |settings: &mut Settings| {
            for key in [
                "extruder",
                "perimeter_extruder",
                "infill_extruder",
                "solid_infill_extruder",
            ] {
                if settings.contains_key(key) {
                    settings.insert(key.into(), "1".into());
                }
            }
        };
        for o in &mut project.objects {
            reset(&mut o.settings);
            for p in &mut o.parts {
                p.painting.material = FacetStates::default();
                reset(&mut p.settings);
            }
            for r in &mut o.height_ranges {
                reset(&mut r.settings);
            }
        }
        project
    }

    /// Mixes defined or referenced by model surfaces and region tool settings.
    fn mix_info(&self, used_only: bool) -> Vec<Value> {
        let mut used = std::collections::BTreeSet::new();
        if used_only {
            for (o, object) in self.project.objects.iter().enumerate() {
                for p in 0..object.parts.len() {
                    for (t, area) in self.part_areas(o, p).1.iter().enumerate() {
                        if *area > 0. {
                            used.insert(t + 1);
                        }
                    }
                }
                // Modifiers and height ranges can assign mixes without painted facets.
                for settings in std::iter::once(&object.settings)
                    .chain(object.parts.iter().map(|p| &p.settings))
                    .chain(object.height_ranges.iter().map(|r| &r.settings))
                {
                    for key in [
                        "extruder",
                        "perimeter_extruder",
                        "infill_extruder",
                        "solid_infill_extruder",
                    ] {
                        if let Some(t) = settings.get(key).and_then(|v| v.parse::<usize>().ok()) {
                            used.insert(t);
                        }
                    }
                }
            }
        }
        self.mixes().unwrap_or_default().extruders().iter()
            .filter(|m| !used_only || used.contains(&m.id))
            .map(|m| json!({
                "tool": m.id, "name": m.name, "color": colors::name(self.mix_color(m)),
                "pattern": m.pattern.key(),
                "components": m.components.iter().map(|c| json!({"tool": c.extruder, "ratio": c.ratio})).collect::<Vec<_>>(),
            })).collect()
    }

    fn slice_in(&self, dir: &Path) -> Result<Value, String> {
        let t0 = std::time::Instant::now();
        let (sliced, report) = self.slice_project(dir, self.project.clone(), "project")?;
        let c = &sliced.config;
        let grams_of = |t: usize, mm: f64| {
            let d = c.float_at("filament_diameter", t).unwrap_or(1.75);
            let density = c
                .float_at("filament_density", t)
                .filter(|d| *d > 0.)
                .unwrap_or(1.24);
            mm * std::f64::consts::PI * (d / 2.).powi(2) * density / 1000.
        };
        let filaments: Vec<Value> = report
            .filament_mm
            .iter()
            .enumerate()
            .filter(|(_, mm)| **mm > 0.)
            .map(|(t, mm)| {
                let grams = grams_of(t, *mm);
                let color = self.colors.get(t).copied().unwrap_or([0xF5; 3]);
                json!({
                    "tool": t + 1,
                    "color": colors::name(color),
                    "filament_m": round(mm / 1000., 2),
                    "grams": round(grams, 1),
                })
            })
            .collect();
        let mut warnings: Vec<String> = sliced.warnings.clone();
        // The G-code check's findings, once per rule with a count.
        let mut rules: Vec<(&str, usize, String)> = vec![];
        for d in &report.diagnostics {
            match rules.iter_mut().find(|r| r.0 == d.rule) {
                Some(r) => r.1 += 1,
                None => rules.push((d.rule, 1, format!("{:?}: {}", d.severity, d.message))),
            }
        }
        for (rule, n, first) in rules.iter().take(8) {
            warnings.push(format!("G-code check [{rule}] ×{n}: {first}"));
        }
        let secs = sliced.stats.time_s.max(0.);
        let total_mm: f64 = report.filament_mm.iter().sum();
        let total_g: f64 = report
            .filament_mm
            .iter()
            .enumerate()
            .map(|(t, mm)| grams_of(t, *mm))
            .sum();
        // Filament the prime/wipe tower and purging take, per tool.
        let mut tower_mm = 0.;
        let mut tower_g = 0.;
        for (t, roles) in report.tool_role_filament_mm.iter().enumerate() {
            for (role, mm) in roles {
                let r = role.to_ascii_lowercase();
                if r.contains("tower") || r.contains("purge") || r.contains("flush") {
                    tower_mm += mm;
                    tower_g += grams_of(t, *mm);
                }
            }
        }
        let mut out = json!({
            "printer": self.printer_name(),
            "layers": sliced.layers,
            "estimated_time": hours(secs),
            "estimated_time_s": secs.round(),
            "filament_by_tool": filaments,
            "total_filament_m": round(total_mm / 1000., 2),
            "total_filament_g": round(total_g, 1),
            "tool_changes": report.toolchanges,
            "purge_and_tower_filament_m": round(tower_mm / 1000., 2),
            "purge_and_tower_filament_g": round(tower_g, 1),
            "gcode_check": { "errors": report.errors, "warnings": report.warnings },
            "warnings": warnings,
        });
        let mixes_used = self.mix_info(true);
        out["mixes_used"] = json!(mixes_used);
        if !mixes_used.is_empty() && self.color_mix_enabled {
            out["mix_time_cost"] = json!(
                "Mixed colors alternate physical tools per layer; tool changes add time and may add purge and tower filament."
            );
        }
        // The same model in one color, for what the colors cost.
        if report.toolchanges > 0 || report.filament_mm.iter().filter(|mm| **mm > 0.).count() > 1 {
            let (one, one_report) =
                self.slice_project(dir, self.single_colour_project(), "single-color")?;
            let one_secs = one.stats.time_s.max(0.);
            let one_mm: f64 = one_report.filament_mm.iter().sum();
            let one_g: f64 = one_report
                .filament_mm
                .iter()
                .enumerate()
                .map(|(t, mm)| grams_of(t, *mm))
                .sum();
            let extra_s = (secs - one_secs).max(0.);
            let extra_pct = 100. * extra_s / one_secs.max(1.);
            out["single_color"] = json!({
                "estimated_time": hours(one_secs),
                "estimated_time_s": one_secs.round(),
                "filament_m": round(one_mm / 1000., 2),
                "filament_g": round(one_g, 1),
            });
            out["color_cost"] = json!({
                "mixes_used": mixes_used,
                "mix_time_cost": out["mix_time_cost"],
                "extra_time": hours(extra_s),
                "extra_time_s": extra_s.round(),
                "extra_time_pct": round(extra_pct, 0),
                "extra_filament_m": round((total_mm - one_mm).max(0.) / 1000., 2),
                "extra_filament_g": round((total_g - one_g).max(0.), 1),
                "tool_changes": report.toolchanges,
            });
            out["advice"] = json!(format!(
                "{} tool changes: {} instead of {} in one color (+{:.0}%), {:.0} g more filament \
                 ({:.0} g purge and tower). Tell the user this cost. {}",
                report.toolchanges,
                hours(secs),
                hours(one_secs),
                extra_pct,
                (total_g - one_g).max(0.),
                tower_g,
                if extra_pct > 50. || report.toolchanges > 100 {
                    "That is a lot: offer cheaper alternatives (whole parts per tool with \
                     set_part_tool, height bands that change color once, fewer and larger \
                     painted areas; small details on many layers cost a tool change per layer)."
                } else {
                    "Offer cheaper alternatives if the user wants a faster print."
                }
            ));
        }
        out["slice_seconds"] = json!(round(t0.elapsed().as_secs_f64(), 1));
        Ok(out)
    }
}

/// `1h 05m`.
fn hours(secs: f64) -> String {
    let secs = secs.max(0.);
    format!(
        "{}h {:02}m",
        (secs / 3600.) as u64,
        ((secs % 3600.) / 60.) as u64
    )
}

fn home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|k| std::env::var_os(k).filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

/// The checks before `save_project` writes `path`: a `.3mf` path in an existing folder, not
/// the opened `input`, not inside Mica's source checkout, and an existing file only with
/// `overwrite`.
pub fn check_save_target(path: &Path, input: Option<&Path>, overwrite: bool) -> Result<(), String> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("3mf"))
    {
        return Err("save_project writes a .3mf file; give a path ending in .3mf".into());
    }
    if input.is_some_and(|i| Scene::same_file(path, i)) {
        return Err("refusing to overwrite the opened input file; save to a new path".into());
    }
    if let Some(root) = source_checkout(path) {
        return Err(format!(
            "{} is inside the Mica source checkout ({}); projects are not saved there. \
             Save to the user's own folder (e.g. ~/Desktop/name.3mf), or call save_project \
             without a path to save to the Desktop",
            path.display(),
            root.display()
        ));
    }
    if path.exists() && !overwrite {
        return Err(format!(
            "{} exists; pass overwrite: true to replace it",
            path.display()
        ));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && !dir.is_dir()
    {
        return Err(format!("folder {} does not exist", dir.display()));
    }
    Ok(())
}

/// Where `save_project` writes: `path` (`~/` is the home folder, a relative path is taken
/// from the server's working folder), or without one a new `<stem>-painted.3mf` in the user's
/// Desktop (Documents without a Desktop, else the home folder).
pub fn resolve_save_path(path: Option<&str>, stem: &str) -> Result<PathBuf, String> {
    let home = home();
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let p = match (p.strip_prefix("~/"), &home) {
                (Some(rest), Some(h)) => h.join(rest),
                _ => PathBuf::from(p),
            };
            Ok(if p.is_absolute() {
                p
            } else {
                std::env::current_dir()
                    .map_err(|e| format!("the working folder: {e}"))?
                    .join(p)
            })
        }
        None => {
            let home =
                home.ok_or("no home folder is known (HOME is not set); give save_project a path")?;
            Ok(unique_path(
                &default_save_dir(&home),
                &format!("{stem}-painted"),
            ))
        }
    }
}

/// Where a project goes without a path: the Desktop, else Documents, else the home folder.
pub fn default_save_dir(home: &Path) -> PathBuf {
    ["Desktop", "Documents"]
        .iter()
        .map(|d| home.join(d))
        .find(|d| d.is_dir())
        .unwrap_or_else(|| home.to_path_buf())
}

/// `dir/stem.3mf`, or `stem-2.3mf`, `stem-3.3mf`, ... when taken.
pub fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.3mf"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.3mf")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// The Mica source checkout `path` would be written into, if any: a folder up the path with
/// this workspace's Cargo.toml and a `.git`, or the checkout this binary was built from.
pub fn source_checkout(path: &Path) -> Option<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    // The deepest folder that exists, resolved (symlinks, `..`).
    let mut dir = abs.parent()?.to_path_buf();
    let dir = loop {
        if let Ok(c) = dir.canonicalize() {
            break c;
        }
        dir = dir.parent()?.to_path_buf();
    };
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .ok()
        .filter(|root| is_mica_checkout(root));
    if let Some(root) = built_from
        && dir.starts_with(&root)
    {
        return Some(root);
    }
    dir.ancestors()
        .find(|d| is_mica_checkout(d))
        .map(Path::to_path_buf)
}

fn is_mica_checkout(dir: &Path) -> bool {
    dir.join(".git").exists()
        && std::fs::read_to_string(dir.join("Cargo.toml"))
            .is_ok_and(|t| t.contains("[workspace]") && t.contains("mica-core"))
}

struct Hit {
    object: usize,
    part: usize,
    facet: usize,
    point: V3,
    dir: V3,
    normal: V3,
    pixel: [u32; 2],
    snapped: bool,
}

impl Hit {
    fn describe(&self, scene: &Scene) -> Value {
        let obj = &scene.project.objects[self.object];
        let tools = scene.facet_tool(self.object, self.part, self.facet);
        let mut v = json!({
            "object": self.object + 1,
            "object_name": obj.name,
            "part": self.part + 1,
            "part_name": obj.parts[self.part].name,
            "facet": self.facet,
            "point_mm": round3(self.point),
            "normal": self.normal.map(|v| round(f64::from(v), 3)),
            "tools_on_facet": tools,
            "pixel": self.pixel,
        });
        if self.snapped {
            v["note"] = json!("the requested pixel was background; used the nearest model pixel");
        }
        v
    }
}

/// A project's embedded settings with `filament_colour` replaced: Orca's JSON
/// (`project_settings.config`) or Prusa's `; key = value` lines (`Slic3r_PE.config`).
fn with_colours(raw: &RawConfig, hexes: &[String]) -> RawConfig {
    let mut out = raw.clone();
    if raw.path.ends_with("project_settings.config") {
        if let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&raw.text) {
            map.insert(
                "filament_colour".into(),
                Value::Array(hexes.iter().cloned().map(Value::String).collect()),
            );
            if let Ok(text) = serde_json::to_string_pretty(&Value::Object(map)) {
                out.text = text;
            }
        }
    } else {
        let line = format!("; filament_colour = {}", hexes.join(";"));
        let mut found = false;
        let lines: Vec<String> = raw
            .text
            .lines()
            .map(|l| {
                if l.trim_start().starts_with("; filament_colour =") {
                    found = true;
                    line.clone()
                } else {
                    l.to_string()
                }
            })
            .collect();
        if found {
            out.text = lines.join("\n") + "\n";
        }
    }
    out
}

pub fn kind_name(k: &PartKind) -> &'static str {
    match k {
        PartKind::Model => "model",
        PartKind::NegativeVolume => "negative volume",
        PartKind::Modifier => "modifier",
        PartKind::SupportEnforcer => "support enforcer",
        PartKind::SupportBlocker => "support blocker",
        PartKind::Unknown(_) => "unknown",
    }
}

/// The session: at most one open scene.
#[derive(Default)]
pub struct Session {
    pub scene: Option<Scene>,
}

impl Session {
    pub fn scene(&mut self) -> Result<&mut Scene, String> {
        self.scene.as_mut().ok_or_else(|| {
            "no model is open; call open with the path of an .stl or .3mf first".into()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("mica-mcp-save-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_save_folder_desktop_then_documents_then_home() {
        let home = Temp::new("home");
        assert_eq!(default_save_dir(&home.0), home.0);
        std::fs::create_dir(home.0.join("Documents")).unwrap();
        assert_eq!(default_save_dir(&home.0), home.0.join("Documents"));
        std::fs::create_dir(home.0.join("Desktop")).unwrap();
        let desk = default_save_dir(&home.0);
        assert_eq!(desk, home.0.join("Desktop"));
        // Names never replace a file already there.
        let first = unique_path(&desk, "3DBenchy-painted");
        assert_eq!(first, desk.join("3DBenchy-painted.3mf"));
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&desk, "3DBenchy-painted");
        assert_eq!(second, desk.join("3DBenchy-painted-2.3mf"));
        std::fs::write(&second, b"x").unwrap();
        assert_eq!(
            unique_path(&desk, "3DBenchy-painted"),
            desk.join("3DBenchy-painted-3.3mf")
        );
        assert_eq!(source_checkout(&first), None);
    }

    #[test]
    fn the_source_checkout_is_recognised() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        // This checkout: a file, a folder that does not exist yet, a path with `..`.
        for p in [
            root.join("tests/models/3DBenchy_U1_painted.3mf"),
            root.join("no/such/folder/x.3mf"),
            root.join("crates/../x.3mf"),
        ] {
            assert_eq!(
                source_checkout(&p).as_deref(),
                Some(root.as_path()),
                "{p:?}"
            );
        }
        // Any other Mica checkout, found by walking up: its Cargo.toml and .git.
        let other = Temp::new("checkout");
        std::fs::write(
            other.0.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\n\
             mica-core = { path = \"crates/mica-core\" }\n",
        )
        .unwrap();
        let inside = other.0.join("tests/models/out.3mf");
        assert_eq!(source_checkout(&inside), None, "no .git: not a checkout");
        std::fs::write(other.0.join(".git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(
            source_checkout(&inside),
            Some(other.0.canonicalize().unwrap())
        );
        // An unrelated Rust workspace is fine.
        std::fs::write(other.0.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert_eq!(source_checkout(&inside), None);
    }
}
