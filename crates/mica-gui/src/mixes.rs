// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mixed colors (ColorMix): a color the printer makes by alternating two or three loaded
//! filaments. The project's list (numbered after the physical tools, as PrusaSlicer numbers
//! its virtual extruders), the "Mixed colors" section of the Filaments card with its editor,
//! the predicted swatches (Prusa's measured color model, `mica_core::fdm_mixer`), the tool
//! names and colors every tool picker shows, the renumbering when a mix is removed or moved,
//! and the project file text.
//!
//! The slicer sees the list through `scene::Object::mixes` on the copies a slice job gets
//! (`App::slice_job`), so the GUI slices exactly what the CLI slices from the saved project.

use crate::app::App;
use crate::panels::widgets::{self, Glyph};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::config::Config;
use mica_core::fdm_mixer;
use mica_core::full_spectrum::{Component, FullSpectrum, Pattern, VirtualExtruder};
use mica_core::print_job::ToolAssignments;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Layer heights above this blend visibly worse (Prusa's ColorMix guidance).
pub const GOOD_LAYER_HEIGHT: f64 = 0.15;

/// The project's mixed colors and the editor's state.
#[derive(Clone, Debug, Default)]
pub struct MixState {
    /// The mixes, sorted by tool number; numbers follow the physical tools without gaps.
    pub list: Vec<VirtualExtruder>,
    /// The opened or saved project's text and the list it describes: written back verbatim
    /// while the list (with its predicted colors) is unchanged.
    pub original: Option<(Vec<VirtualExtruder>, String)>,
    /// The mix being created or edited in the Filaments card.
    pub draft: Option<Draft>,
}

/// The editor's working copy of one mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The tool number being edited (one-based), or `None` for a new mix.
    pub editing: Option<usize>,
    /// Zero-based physical tools, two or three.
    pub tools: Vec<usize>,
    /// Shares in percent at the bottom (the mix's ratio), one per tool.
    pub percents: Vec<f64>,
    /// Shares in percent at the top of a gradient.
    pub end: Vec<f64>,
    pub pattern: Pattern,
    pub name: String,
    /// The user typed a name: it no longer follows the color.
    pub named: bool,
}

/// What the section asks the app to do after drawing.
#[derive(Clone, Debug, PartialEq)]
pub enum MixAct {
    Save(Draft),
    Delete(usize),
    Swap(usize, usize),
}

impl ToolChoice {
    /// The swatch colour with its alpha.
    pub fn rgba(&self) -> [u8; 4] {
        let [r, g, b] = self.colour;
        [r, g, b, self.alpha]
    }
}

/// A tool picker entry: zero-based tool index, its color and its label.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolChoice {
    pub tool: usize,
    pub colour: [u8; 3],
    /// The filament's alpha (255 = opaque; mixes are opaque).
    pub alpha: u8,
    pub label: String,
    /// The tool chip text: `T3`, or `M1` for the first mix.
    pub short: String,
}

/// The shares the exact cycle really prints, in percent: what the sliders snap to.
pub fn snapped(percents: &[f64]) -> Vec<f64> {
    recipe(0, percents).snapped_percents()
}

fn recipe(id: usize, percents: &[f64]) -> VirtualExtruder {
    VirtualExtruder {
        id,
        components: percents
            .iter()
            .enumerate()
            .map(|(i, p)| Component {
                extruder: i + 1,
                ratio: p.max(0.) / 100.,
            })
            .collect(),
        ..Default::default()
    }
}

/// Letters for a draft's tools: A, B, C.
pub fn letter(i: usize) -> char {
    (b'A' + i as u8) as char
}

/// `A A B A` for a cycle of component positions (one-based, as `build_sequence` numbers a
/// recipe whose components are tools 1, 2, 3).
pub fn cycle_text(cycle: &[usize]) -> String {
    cycle
        .iter()
        .map(|t| letter(t.saturating_sub(1)).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The predicted color of `ratios` (shares summing to 1) of the tools' colors.
pub fn predict(colours: &[[u8; 3]], tools: &[usize], ratios: &[f64]) -> [u8; 3] {
    let parts: Vec<fdm_mixer::Part> = tools
        .iter()
        .zip(ratios)
        .map(|(t, r)| fdm_mixer::Part {
            rgb: colours.get(*t).copied().unwrap_or([128; 3]),
            ratio: *r,
        })
        .collect();
    fdm_mixer::mix_rgb(&parts).unwrap_or([128; 3])
}

/// A mix's colors from the bottom to the top (one for plain mixes, the two ends for a gradient).
pub fn mix_colours(colours: &[[u8; 3]], ve: &VirtualExtruder) -> Vec<[u8; 3]> {
    let tools: Vec<usize> = ve
        .components
        .iter()
        .map(|c| c.extruder.saturating_sub(1))
        .collect();
    let start: Vec<f64> = ve.components.iter().map(|c| c.ratio).collect();
    let mut out = vec![predict(colours, &tools, &start)];
    if ve.pattern == Pattern::Gradient && ve.end_ratios.len() == tools.len() {
        out.push(predict(colours, &tools, &ve.end_ratios));
    }
    out
}

/// The single color the 3D view draws a mix with (a gradient's middle).
pub fn display_colour(colours: &[[u8; 3]], ve: &VirtualExtruder) -> [u8; 3] {
    ve.predicted_rgb(colours)
}

/// Plain color names for labels (hex values never show in text).
const NAMES: [(&str, [u8; 3]); 28] = [
    ("Black", [20, 20, 20]),
    ("Charcoal", [60, 62, 64]),
    ("Gray", [128, 128, 128]),
    ("Silver", [192, 192, 192]),
    ("White", [245, 245, 245]),
    ("Red", [200, 30, 35]),
    ("Dark red", [120, 20, 25]),
    ("Coral", [250, 110, 90]),
    ("Orange", [245, 130, 30]),
    ("Amber", [250, 175, 20]),
    ("Yellow", [245, 215, 40]),
    ("Olive", [128, 128, 30]),
    ("Lime", [160, 210, 50]),
    ("Green", [40, 160, 70]),
    ("Dark green", [20, 90, 45]),
    ("Teal", [20, 130, 130]),
    ("Cyan", [0, 170, 210]),
    ("Sky blue", [110, 180, 235]),
    ("Blue", [30, 90, 200]),
    ("Navy", [25, 35, 95]),
    ("Indigo", [75, 50, 140]),
    ("Purple", [125, 45, 150]),
    ("Violet", [160, 110, 210]),
    ("Magenta", [205, 40, 140]),
    ("Pink", [240, 150, 185]),
    ("Brown", [120, 70, 35]),
    ("Tan", [200, 160, 110]),
    ("Beige", [225, 210, 175]),
];

/// The closest plain color name (CIEDE2000).
pub fn colour_name(rgb: [u8; 3]) -> &'static str {
    let lab = fdm_mixer::rgb_to_lab(rgb);
    NAMES
        .iter()
        .map(|(n, c)| (n, fdm_mixer::delta_e_2000(lab, fdm_mixer::rgb_to_lab(*c))))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or("Mix", |(n, _)| n)
}

/// Average tool changes per layer of a region printed alone, with the tool ordering's
/// last-tool-first rule (a layer that starts with the previous layer's last tool saves one).
pub fn changes_per_layer(walls: &[usize], inside: &[usize]) -> f64 {
    let len = walls.len().max(inside.len()).max(1);
    let mut last = walls.first().copied().unwrap_or(0);
    let mut total = 0usize;
    // Two passes: the first settles the starting tool, the second counts.
    for pass in 0..2 {
        for i in 0..len {
            let mut tools = vec![walls[i % walls.len()]];
            if inside[i % inside.len()] != tools[0] {
                tools.push(inside[i % inside.len()]);
            }
            let changes = if tools.contains(&last) {
                tools.len() - 1
            } else {
                tools.len()
            };
            last = *tools.iter().find(|t| **t != last).unwrap_or(&tools[0]);
            if pass == 1 {
                total += changes;
            }
        }
    }
    total as f64 / len as f64
}

/// Seconds one tool change adds: the printer's tool change time (Klipper toolchangers), else
/// the filaments' load and unload times, plus a short prime; 15 s when the profile says nothing.
pub fn seconds_per_change(config: Option<&Config>, tools: &[usize]) -> f64 {
    const PRIME_S: f64 = 4.;
    let Some(c) = config else { return 15. };
    let machine = c
        .float("machine_tool_change_time")
        .or_else(|| {
            c.extra("machine_tool_change_time")
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0.);
    if machine > 0. {
        return machine + PRIME_S;
    }
    let swap: f64 = tools
        .iter()
        .map(|t| {
            c.float_at("filament_load_time", *t).unwrap_or(0.)
                + c.float_at("filament_unload_time", *t).unwrap_or(0.)
        })
        .sum::<f64>()
        / tools.len().max(1) as f64;
    if swap > 0. { swap + PRIME_S } else { 15. }
}

/// "Mix 2 · 50 % T1 + 50 % T2": what a mix is made of, in words.
pub fn recipe_text(ve: &VirtualExtruder, names: &[String]) -> String {
    let part = |t: usize, r: f64| {
        let name = names
            .get(t.saturating_sub(1))
            .cloned()
            .unwrap_or_else(|| format!("T{t}"));
        format!("{:.0} % {name}", r * 100.)
    };
    let start = ve
        .components
        .iter()
        .map(|c| part(c.extruder, c.ratio))
        .collect::<Vec<_>>()
        .join(" + ");
    if ve.pattern == Pattern::Gradient && ve.end_ratios.len() == ve.components.len() {
        let end = ve
            .components
            .iter()
            .zip(&ve.end_ratios)
            .map(|(c, r)| part(c.extruder, *r))
            .collect::<Vec<_>>()
            .join(" + ");
        format!("{start} → {end}")
    } else {
        start
    }
}

impl Draft {
    fn new(tools: usize) -> Self {
        Self {
            editing: None,
            tools: vec![0, 1.min(tools.saturating_sub(1))],
            percents: vec![50., 50.],
            end: vec![50., 50.],
            pattern: Pattern::Layers,
            name: String::new(),
            named: false,
        }
    }

    fn of(ve: &VirtualExtruder) -> Self {
        let percents: Vec<f64> = ve.components.iter().map(|c| c.ratio * 100.).collect();
        Self {
            editing: Some(ve.id),
            tools: ve
                .components
                .iter()
                .map(|c| c.extruder.saturating_sub(1))
                .collect(),
            end: if ve.end_ratios.len() == percents.len() {
                ve.end_ratios.iter().map(|r| r * 100.).collect()
            } else {
                percents.iter().rev().copied().collect()
            },
            percents,
            pattern: ve.pattern,
            name: ve.name.clone().unwrap_or_default(),
            named: ve.name.is_some(),
        }
    }

    /// The mix this draft describes, numbered `id`, with snapped shares.
    pub fn to_mix(&self, id: usize) -> VirtualExtruder {
        let ratios = snapped(&self.percents);
        let end = if self.pattern == Pattern::Gradient {
            let e = normalise(&self.end);
            e.iter().map(|v| v / 100.).collect()
        } else {
            vec![]
        };
        VirtualExtruder {
            id,
            color: None,
            components: self
                .tools
                .iter()
                .zip(&ratios)
                .map(|(t, r)| Component {
                    extruder: t + 1,
                    ratio: r / 100.,
                })
                .collect(),
            name: (!self.name.trim().is_empty()).then(|| self.name.trim().to_string()),
            pattern: self.pattern,
            end_ratios: end,
        }
    }

    /// What is wrong with the draft, if anything (the Add button stays off).
    fn problem(&self) -> Option<&'static str> {
        let mut seen = self.tools.clone();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() != self.tools.len() {
            return Some("Pick a different tool for each color.");
        }
        if self.pattern == Pattern::Gradient && normalise(&self.end) == normalise(&self.percents) {
            return Some("A gradient needs a different mix at the top.");
        }
        None
    }
}

/// Shares rescaled to sum to 100.
fn normalise(v: &[f64]) -> Vec<f64> {
    let s: f64 = v.iter().sum();
    if s <= 0. {
        return vec![100. / v.len().max(1) as f64; v.len()];
    }
    v.iter().map(|x| x * 100. / s).collect()
}

/// After slider `i` moved, the others share the rest in their old proportion (each keeps at
/// least 1 %).
fn rebalance(v: &mut [f64], i: usize) {
    let n = v.len();
    v[i] = v[i].clamp(1., 100. - (n as f64 - 1.));
    let rest = 100. - v[i];
    let others: f64 = (0..n).filter(|&k| k != i).map(|k| v[k]).sum();
    for k in (0..n).filter(|&k| k != i) {
        v[k] = if others > 0. {
            v[k] * rest / others
        } else {
            rest / (n as f64 - 1.)
        };
        v[k] = v[k].max(1.);
    }
}

/// The paint card chip's tooltip.
pub fn paint_tip(choice: &ToolChoice) -> String {
    if choice.short.starts_with('T') {
        format!("Paint with tool {}", choice.tool + 1)
    } else {
        format!("Paint with the mixed color {}", choice.label)
    }
}

/// The tool chip text of a mix: `M1` for the first.
pub fn short_name(index: usize) -> String {
    format!("M{}", index + 1)
}

impl App {
    /// The project's mixes as the engine takes them, validated for the current printer.
    pub(crate) fn mix_spectrum(&self) -> Result<FullSpectrum, String> {
        FullSpectrum::new(self.mixes.list.clone(), self.tool_count())
    }

    /// Tool colors with the mixes' predicted colors after the physical ones (index = tool
    /// number − 1), for the 3D view, painted facets and tool pickers.
    pub(crate) fn model_colours(&self) -> Vec<[u8; 3]> {
        let mut out = self.tool_colours();
        let physical = out.clone();
        for m in &self.mixes.list {
            // Numbers follow the physical tools; after a printer change they may not.
            if out.len() < m.id {
                out.resize(m.id, [128; 3]);
            }
            // With ColorMix off a mix prints (and shows) in its main color.
            out[m.id - 1] = if self.prefs.features.color_mix {
                display_colour(&physical, m)
            } else {
                physical
                    .get(m.main_extruder().saturating_sub(1))
                    .copied()
                    .unwrap_or([128; 3])
            };
        }
        out
    }

    /// The list position of mix `id` (its one-based tool number).
    pub(crate) fn mix_index(&self, id: usize) -> Option<usize> {
        self.mixes.list.iter().position(|m| m.id == id)
    }

    /// The tool number a new mix gets: right after the physical tools and the other mixes.
    fn next_mix_id(&self) -> usize {
        self.tool_count()
            .max(self.mixes.list.iter().map(|m| m.id).max().unwrap_or(0))
            + 1
    }

    /// [`Self::model_colours`] for drawing (near-black lifted, as `tool_colors_f32`); an
    /// opened G-code file keeps its own colors.
    pub(crate) fn scene_colours_f32(&self) -> Vec<[f32; 3]> {
        let mut out = self.tool_colors_f32();
        if self.gcode.is_none() {
            let physical = self.tool_count();
            out.extend(
                self.model_colours()
                    .into_iter()
                    .skip(physical)
                    .map(|c| crate::app::display_colour(c.map(|v| f32::from(v) / 255.))),
            );
        }
        out
    }

    /// The name of mix `index` (zero-based): the user's, else its predicted color's name.
    pub(crate) fn mix_name(&self, index: usize) -> String {
        match self.mixes.list.get(index) {
            Some(m) => m.name.clone().unwrap_or_else(|| self.auto_name(index, m)),
            None => format!("Mix {}", index + 1),
        }
    }

    /// The nearest color name of `mix`, or `Mix N` when an earlier mix already shows that name.
    fn auto_name(&self, index: usize, mix: &VirtualExtruder) -> String {
        let colours = self.tool_colours();
        let name = colour_name(display_colour(&colours, mix));
        let taken = (0..index.min(self.mixes.list.len())).any(|k| self.mix_name(k) == name);
        if taken {
            format!("Mix {}", index + 1)
        } else {
            name.to_string()
        }
    }

    /// Every tool an object, part or brush can use: the physical tools, then the mixes.
    pub(crate) fn tool_choices(&self) -> Vec<ToolChoice> {
        let colours = self.model_colours();
        let alphas: Vec<u8> = self.tool_colours_rgba().iter().map(|c| c[3]).collect();
        let physical = self.tool_count();
        // With ColorMix off only the physical tools are offered.
        let count = if self.prefs.features.color_mix {
            colours.len()
        } else {
            colours.len().min(physical)
        };
        (0..count)
            .map(|t| {
                let mix = self.mix_index(t + 1).filter(|_| t >= physical);
                if let Some(k) = mix {
                    ToolChoice {
                        tool: t,
                        colour: colours[t],
                        alpha: 255,
                        label: self.mix_name(k),
                        short: short_name(k),
                    }
                } else if t < physical {
                    ToolChoice {
                        tool: t,
                        colour: colours[t],
                        alpha: alphas.get(t).copied().unwrap_or(255),
                        label: format!("Tool {}", t + 1),
                        short: format!("T{}", t + 1),
                    }
                } else {
                    ToolChoice {
                        tool: t,
                        colour: colours[t],
                        alpha: 255,
                        label: format!("Tool {} (not on this printer)", t + 1),
                        short: format!("T{}", t + 1),
                    }
                }
            })
            .collect()
    }

    /// The physical tools' labels for recipes: `T1 Red`.
    fn tool_names(&self) -> Vec<String> {
        self.tool_colours()
            .iter()
            .enumerate()
            .map(|(t, c)| format!("T{} {}", t + 1, colour_name(*c)))
            .collect()
    }

    /// The project file text of the mixes, `None` without any.
    pub(crate) fn mixes_json(&self) -> Option<String> {
        if self.mixes.list.is_empty() {
            return None;
        }
        let colours = self.tool_colours();
        let list = self.list_with_colours();
        // The opened file's own text, while it still fits the printer.
        if let Some((original, text)) = &self.mixes.original
            && *original == list
            && FullSpectrum::from_json(text, colours.len()).is_ok()
        {
            return Some(text.clone());
        }
        let spectrum = FullSpectrum::new(list, colours.len()).ok()?;
        let hex: Vec<String> = colours.iter().map(|c| fdm_mixer::rgb_to_hex(*c)).collect();
        Some(spectrum.to_json(&hex))
    }

    /// The list with each mix's predicted color stored (what PrusaSlicer shows for it).
    fn list_with_colours(&self) -> Vec<VirtualExtruder> {
        let colours = self.tool_colours();
        self.mixes
            .list
            .iter()
            .map(|m| VirtualExtruder {
                color: Some(fdm_mixer::rgb_to_hex(display_colour(&colours, m))),
                ..m.clone()
            })
            .collect()
    }

    /// Takes an opened project's mixes (`None`: the project has none).
    pub(crate) fn load_mixes(&mut self, loaded: Option<(FullSpectrum, String)>) {
        self.mixes.draft = None;
        match loaded {
            Some((spectrum, text)) => {
                self.mixes.list = spectrum.extruders().to_vec();
                self.mixes.original = Some((self.mixes.list.clone(), text));
            }
            None => {
                self.mixes.list.clear();
                self.mixes.original = None;
            }
        }
    }

    /// After a save: the written text is the new verbatim original.
    pub(crate) fn mixes_saved(&mut self, text: Option<String>) {
        self.mixes.original = text.map(|t| {
            (
                FullSpectrum::from_json(&t, self.tool_count())
                    .map(|s| s.extruders().to_vec())
                    .unwrap_or_default(),
                t,
            )
        });
    }

    /// Whether the project has mixed colors (its own list, or an object's imported ones).
    pub(crate) fn project_uses_mixes(&self) -> bool {
        !self.mixes.list.is_empty()
            || self.plates.plates.iter().any(|p| {
                p.objects.iter().any(|o| {
                    o.region
                        .as_ref()
                        .is_some_and(|r| !r.full_spectrum.extruders().is_empty())
                })
            })
    }

    /// The objects of a slice job with the mixes they slice with.
    pub(crate) fn with_mixes(
        &self,
        mut objects: Vec<crate::scene::Object>,
    ) -> Vec<crate::scene::Object> {
        if self.mixes.list.is_empty() {
            // An opened project's mixes were all deleted: its objects no longer slice with them.
            let empty = Arc::new(FullSpectrum::default());
            for o in &mut objects {
                if o.region
                    .as_ref()
                    .is_some_and(|r| !r.full_spectrum.extruders().is_empty())
                {
                    o.mixes = Some(empty.clone());
                }
            }
            return objects;
        }
        if let Ok(spectrum) = self.mix_spectrum() {
            let spectrum = Arc::new(spectrum);
            for o in &mut objects {
                o.mixes = Some(spectrum.clone());
            }
        }
        objects
    }

    /// After a printer change: the mixes are numbered again right after the new printer's
    /// physical tools (M1 = T5 on a 4-tool U1, T6 on a 5-tool XL), and every object, part and
    /// painted area that used one follows. GUI monkey test, seed 302: a mix made on the U1
    /// kept number 5 on the XL, where 5 is a physical tool; the saved project then could not
    /// be opened again ("invalid or duplicate virtual extruder ID 5"). A mix that uses a tool
    /// the new printer lacks is reported.
    pub(crate) fn fit_mixes_to_printer(&mut self) {
        if self.mixes.list.is_empty() {
            return;
        }
        let physical = self.tool_count();
        let mut ids: Vec<usize> = self.mixes.list.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        let map: BTreeMap<usize, usize> = ids
            .iter()
            .enumerate()
            .map(|(k, &id)| (id, physical + 1 + k))
            .filter(|(old, new)| old != new)
            .collect();
        if !map.is_empty() {
            self.renumber_tools(&map);
            for m in &mut self.mixes.list {
                m.id = map.get(&m.id).copied().unwrap_or(m.id);
            }
            if let Some(d) = &mut self.mixes.draft
                && let Some(e) = d.editing
            {
                d.editing = map.get(&e).copied().or(Some(e));
            }
            self.plates.invalidate_all();
        }
        if let Err(e) = self.mix_spectrum() {
            self.notify_warning(format!(
                "The mixed colors do not fit this printer ({e}); edit them in Mixed colors                  before slicing or saving"
            ));
        }
    }

    /// Mixes changed: every plate slices again.
    fn mixes_changed(&mut self) {
        self.plates.invalidate_all();
        self.stop_worker();
        self.slice_queue.clear();
        self.changed();
    }

    /// Renumbers one-based tools everywhere they are used: object and part assignments,
    /// layer ranges and color painting.
    fn renumber_tools(&mut self, map: &BTreeMap<usize, usize>) {
        if map.is_empty() {
            return;
        }
        let one = |t: usize| map.get(&t).copied().unwrap_or(t);
        let keys = [
            "extruder",
            "perimeter_extruder",
            "infill_extruder",
            "solid_infill_extruder",
        ];
        let remap_overrides = |o: &mut BTreeMap<String, String>| {
            for k in keys {
                if let Some(v) = o.get_mut(k)
                    && let Ok(t) = v.trim().parse::<usize>()
                    && t > 0
                {
                    *v = one(t).to_string();
                }
            }
        };
        let remap_config = |c: &mut Config| {
            for k in keys {
                if let Some(t) = c.raw(k).and_then(|v| v.trim().parse::<usize>().ok())
                    && t > 0
                    && map.contains_key(&t)
                {
                    let _ = c.set_raw(k, &one(t).to_string());
                }
            }
        };
        let states = |s: u8| map.get(&usize::from(s)).map_or(s, |t| *t as u8);
        for plate in &mut self.plates.plates {
            for o in &mut plate.objects {
                for role in o.tools.0.iter_mut().take(3).flatten() {
                    *role = one(*role + 1) - 1;
                }
                remap_overrides(&mut o.overrides);
                for v in &mut o.volumes {
                    remap_overrides(&mut v.overrides);
                }
                for r in &mut o.height_ranges {
                    remap_overrides(&mut r.overrides);
                }
                if !o.painting.material.is_empty() {
                    let mut p = (*o.painting).clone();
                    p.material = p.material.map_states(states);
                    o.painting = Arc::new(p);
                }
                if let Some(region) = &mut o.region {
                    let r = Arc::make_mut(region);
                    for v in &mut r.volumes {
                        remap_config(&mut v.config);
                        v.painting.material = v.painting.material.map_states(states);
                    }
                }
            }
        }
    }

    pub(crate) fn apply_mix_acts(&mut self, acts: Vec<MixAct>) {
        let physical = self.tool_count();
        for act in acts {
            match act {
                MixAct::Save(draft) => {
                    let id = draft.editing.unwrap_or_else(|| self.next_mix_id());
                    let mix = draft.to_mix(id);
                    let mut list = self.mixes.list.clone();
                    match list.iter_mut().find(|m| m.id == id) {
                        Some(m) => *m = mix,
                        None => list.push(mix),
                    }
                    if let Err(e) = FullSpectrum::new(list.clone(), physical) {
                        self.notify_error("Mixed color not saved", e);
                        continue;
                    }
                    self.record(if draft.editing.is_some() {
                        "Edit mixed color"
                    } else {
                        "Add mixed color"
                    });
                    self.mixes.list = list;
                    self.mixes.draft = None;
                    self.mixes_changed();
                }
                MixAct::Delete(id) => {
                    let name = self.mix_name(self.mix_index(id).unwrap_or(usize::MAX));
                    let result = self.mix_spectrum().and_then(|s| s.without(id, physical));
                    match result {
                        Ok((rest, map)) => {
                            self.record("Delete mixed color");
                            let used = self.mix_users(id);
                            let target = map.get(&id).copied().unwrap_or(1);
                            self.renumber_tools(&map);
                            self.mixes.list = rest.extruders().to_vec();
                            if self
                                .mixes
                                .draft
                                .as_ref()
                                .is_some_and(|d| d.editing == Some(id))
                            {
                                self.mixes.draft = None;
                            }
                            if used > 0 {
                                self.notify_info(format!(
                                    "{name} deleted: its {used} object{} and painted areas now print with tool {target}",
                                    if used == 1 { "" } else { "s" }
                                ));
                            }
                            self.mixes_changed();
                        }
                        Err(e) => self.notify_error("Mixed color not deleted", e),
                    }
                }
                MixAct::Swap(a, b) => {
                    match self.mix_spectrum().and_then(|s| s.swapped(a, b, physical)) {
                        Ok((list, map)) => {
                            self.record("Move mixed color");
                            self.renumber_tools(&map);
                            self.mixes.list = list.extruders().to_vec();
                            self.mixes_changed();
                        }
                        Err(e) => self.notify_error("Mixed color not moved", e),
                    }
                }
            }
        }
    }

    /// Adds a mix from `1:50,2:50[/outer|/gradient:20,80][@Name]` (one-based tools, percent),
    /// as the editor's Add button does (screenshots and tests).
    pub(crate) fn add_mix_spec(&mut self, spec: &str) -> Result<(), String> {
        let usage = "--mix 1:50,2:50[/outer|/gradient:20,80][@Name]";
        let (spec, name) = match spec.split_once('@') {
            Some((s, n)) => (s, Some(n.trim().to_string())),
            None => (spec, None),
        };
        let (parts, pattern) = spec.split_once('/').unwrap_or((spec, "layers"));
        let mut d = Draft::new(self.tool_count());
        d.tools.clear();
        d.percents.clear();
        for part in parts.split(',') {
            let (t, pct) = part.split_once(':').ok_or(usage)?;
            let t: usize = t.trim().parse().map_err(|_| usage)?;
            d.tools.push(t.checked_sub(1).ok_or(usage)?);
            d.percents.push(pct.trim().parse().map_err(|_| usage)?);
        }
        d.percents = snapped(&normalise(&d.percents));
        d.end = d.percents.iter().rev().copied().collect();
        d.pattern = match pattern.split_once(':').map_or(pattern, |(p, _)| p) {
            "layers" => Pattern::Layers,
            "outer" | "outer_wall_first" => Pattern::OuterWallFirst,
            "gradient" => {
                if let Some((_, end)) = pattern.split_once(':') {
                    d.end = end
                        .split(',')
                        .map(|v| v.trim().parse::<f64>().map_err(|_| usage.to_string()))
                        .collect::<Result<_, _>>()?;
                }
                Pattern::Gradient
            }
            _ => return Err(usage.into()),
        };
        if let Some(n) = name {
            d.name = n;
            d.named = true;
        }
        if let Some(why) = d.problem() {
            return Err(why.into());
        }
        let before = self.mixes.list.len();
        self.apply_mix_acts(vec![MixAct::Save(d)]);
        if self.mixes.list.len() == before {
            return Err(format!("{usage}: the mix was not added"));
        }
        Ok(())
    }

    /// Opens the editor on mix `which` (one-based) or `new`.
    pub(crate) fn open_mix_editor(&mut self, which: &str) -> Result<(), String> {
        self.mixes.draft = Some(if which == "new" {
            Draft::new(self.tool_count())
        } else {
            let k: usize = which
                .parse()
                .map_err(|_| "--mix-editor N (one-based) or new")?;
            Draft::of(
                self.mixes
                    .list
                    .get(k.saturating_sub(1))
                    .ok_or("--mix-editor: no such mix")?,
            )
        });
        Ok(())
    }

    /// How many objects use mix `id` (assigned or painted).
    fn mix_users(&self, id: usize) -> usize {
        self.plates
            .plates
            .iter()
            .flat_map(|p| &p.objects)
            .filter(|o| {
                o.tools.0[..3].iter().flatten().any(|t| t + 1 == id)
                    || o.painting.material.states().contains(&(id as u8))
                    || o.volumes.iter().any(|v| {
                        v.overrides
                            .get("extruder")
                            .is_some_and(|e| e.trim() == id.to_string())
                    })
            })
            .count()
    }

    /// Objects using mix `id` with flat tops or bottoms above the bed (they show stripes).
    fn flat_surfaces(&self, id: usize) -> Vec<String> {
        self.plates
            .current()
            .objects
            .iter()
            .filter(|o| {
                o.tools.0[..3].iter().flatten().any(|t| t + 1 == id)
                    || o.painting.material.states().contains(&(id as u8))
            })
            .filter(|o| has_flat_surfaces(&o.placed_mesh()))
            .map(|o| {
                std::path::Path::new(&o.name)
                    .file_stem()
                    .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned())
            })
            .collect()
    }

    /// The "Mixed colors" section of the Filaments card. Changes come back as acts.
    pub(crate) fn mixed_colors_section(
        &self,
        ui: &mut egui::Ui,
        draft: &mut Option<Draft>,
        acts: &mut Vec<MixAct>,
    ) {
        let physical = self.tool_count();
        if physical < 2 {
            return;
        }
        let p = theme::of(ui);
        let colours = self.tool_colours();
        let names = self.tool_names();
        ui.add_space(10.);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(widgets::spaced_upper("Mixed colors"))
                    .font(theme::semibold(11.))
                    .color(p.muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if draft.is_none()
                    && widgets::icon_button(
                        ui,
                        Glyph::Plus,
                        "Add a mixed color: alternate two or three loaded filaments to print a color you don't have",
                        true,
                    )
                    .clicked()
                {
                    *draft = Some(Draft::new(physical));
                }
            });
        });
        if self.mixes.list.is_empty() && draft.is_none() {
            ui.label(
                egui::RichText::new(
                    "None yet. Mix loaded filaments layer by layer to print other colors.",
                )
                .font(theme::sans(11.))
                .color(p.muted_alt),
            );
        }
        let count = self.mixes.list.len();
        for (k, m) in self.mixes.list.iter().enumerate() {
            ui.add_space(4.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.;
                gradient_swatch(ui, &mix_colours(&colours, m), 16.);
                ui.label(
                    egui::RichText::new(short_name(k))
                        .font(theme::mono(12.))
                        .color(p.muted),
                );
                let name = self.mix_name(k);
                let text = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(&name)
                                .font(theme::sans(13.))
                                .color(p.text),
                        )
                        .truncate(),
                    )
                    .on_hover_text(format!(
                        "{name} (tool {}): {}\nPattern: {}",
                        m.id,
                        recipe_text(m, &names),
                        m.pattern.label()
                    ));
                let _ = text;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.;
                    if widgets::icon_button_sized(
                        ui,
                        Glyph::Close,
                        &format!("Delete {name}"),
                        true,
                        22.,
                    )
                    .clicked()
                    {
                        acts.push(MixAct::Delete(m.id));
                    }
                    if count > 1 {
                        if widgets::icon_button_sized(
                            ui,
                            Glyph::Down,
                            "Move down",
                            k + 1 < count,
                            22.,
                        )
                        .clicked()
                        {
                            acts.push(MixAct::Swap(m.id, m.id + 1));
                        }
                        if widgets::icon_button_sized(ui, Glyph::Up, "Move up", k > 0, 22.)
                            .clicked()
                        {
                            acts.push(MixAct::Swap(m.id, m.id - 1));
                        }
                    }
                    if widgets::icon_button_sized(
                        ui,
                        Glyph::Pencil,
                        &format!("Edit {name}"),
                        true,
                        22.,
                    )
                    .clicked()
                    {
                        *draft = Some(Draft::of(m));
                    }
                });
            });
            ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    recipe_text(m, &names),
                    m.pattern.label()
                ))
                .font(theme::sans(11.))
                .color(p.muted_alt),
            );
        }
        if let Some(d) = draft.as_mut() {
            ui.add_space(8.);
            let mut close = false;
            egui::Frame::new()
                .fill(p.field)
                .stroke(Stroke::new(1., p.border_raised))
                .corner_radius(6.)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    self.mix_editor(ui, d, &colours, &mut close, acts);
                });
            if close {
                *draft = None;
            }
        }
    }

    fn mix_editor(
        &self,
        ui: &mut egui::Ui,
        d: &mut Draft,
        colours: &[[u8; 3]],
        close: &mut bool,
        acts: &mut Vec<MixAct>,
    ) {
        let p = theme::of(ui);
        let physical = self.tool_count();
        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
        let index = d
            .editing
            .and_then(|id| self.mix_index(id))
            .unwrap_or(self.mixes.list.len());
        ui.label(
            egui::RichText::new(if d.editing.is_some() {
                format!("Edit {}", self.mix_name(index))
            } else {
                "New mixed color".to_string()
            })
            .font(theme::semibold(13.))
            .color(p.text),
        );
        // Colors and shares.
        let gradient = d.pattern == Pattern::Gradient;
        let mut changed = None;
        let mut end_changed = None;
        let mut remove = None;
        for i in 0..d.tools.len() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.;
                ui.label(
                    egui::RichText::new(letter(i).to_string())
                        .font(theme::mono(12.))
                        .color(p.muted),
                );
                let t = d.tools[i];
                egui::ComboBox::from_id_salt(("mix-tool", i))
                    .width(96.)
                    .selected_text(format!("T{} {}", t + 1, colour_name(colours[t])))
                    .show_ui(ui, |ui| {
                        for (k, c) in colours.iter().enumerate() {
                            ui.horizontal(|ui| {
                                crate::panels::swatch(ui, *c, 12.);
                                if ui
                                    .selectable_label(
                                        k == t,
                                        format!("T{} {}", k + 1, colour_name(*c)),
                                    )
                                    .clicked()
                                {
                                    d.tools[i] = k;
                                }
                            });
                        }
                    });
                let mut v = d.percents[i];
                let r = ui.add(
                    egui::Slider::new(&mut v, 1.0..=99.0)
                        .suffix(" %")
                        .fixed_decimals(0)
                        .show_value(true),
                );
                if r.changed() {
                    d.percents[i] = v;
                    changed = Some(i);
                }
                if r.drag_stopped() || r.lost_focus() {
                    // Snap to what the exact cycle prints.
                    d.percents = snapped(&d.percents);
                }
                if d.tools.len() > 2
                    && widgets::icon_button_sized(ui, Glyph::Minus, "Remove this color", true, 20.)
                        .clicked()
                {
                    remove = Some(i);
                }
            });
            if gradient {
                ui.horizontal(|ui| {
                    ui.add_space(14.);
                    ui.label(
                        egui::RichText::new("at the top")
                            .font(theme::sans(11.))
                            .color(p.muted_alt),
                    );
                    let mut v = d.end[i];
                    if ui
                        .add(
                            egui::Slider::new(&mut v, 0.0..=100.0)
                                .suffix(" %")
                                .fixed_decimals(0),
                        )
                        .changed()
                    {
                        d.end[i] = v;
                        end_changed = Some(i);
                    }
                });
            }
        }
        if let Some(i) = changed {
            rebalance(&mut d.percents, i);
        }
        if let Some(i) = end_changed {
            let n = d.end.len();
            d.end[i] = d.end[i].clamp(0., 100.);
            let rest = 100. - d.end[i];
            let others: f64 = (0..n).filter(|&k| k != i).map(|k| d.end[k]).sum();
            for k in (0..n).filter(|&k| k != i) {
                d.end[k] = if others > 0. {
                    d.end[k] * rest / others
                } else {
                    rest / (n as f64 - 1.)
                };
            }
        }
        if let Some(i) = remove {
            d.tools.remove(i);
            d.percents.remove(i);
            d.end.remove(i);
            d.percents = snapped(&normalise(&d.percents));
            d.end = normalise(&d.end);
        }
        if d.tools.len() < 3
            && physical > d.tools.len()
            && widgets::link(ui, "Add a third color").clicked()
        {
            let next = (0..physical).find(|t| !d.tools.contains(t)).unwrap_or(0);
            d.tools.push(next);
            d.percents = snapped(&[100. / 3.; 3]);
            d.end.push(0.);
            d.end = normalise(&d.end);
        }
        // Pattern.
        ui.add_space(2.);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
            for pattern in Pattern::ALL {
                let tip = match pattern {
                    Pattern::Layers => "Each layer prints with one tool, in a repeating cycle (as PrusaSlicer does)",
                    Pattern::OuterWallFirst => "The walls you see use the main color more often; the infill inside makes up the rest, so the whole mix keeps its ratio",
                    Pattern::Gradient => "The mix changes smoothly from the bottom ratio to the top ratio over the height the color covers",
                };
                if widgets::chip(ui, None, pattern.label(), d.pattern == pattern, true)
                    .on_hover_text(tip)
                    .clicked()
                {
                    if pattern == Pattern::Gradient && d.pattern != Pattern::Gradient {
                        d.end = d.percents.iter().rev().copied().collect();
                    }
                    d.pattern = pattern;
                }
            }
        });
        let mix = d.to_mix(d.editing.unwrap_or_else(|| self.next_mix_id()));
        let spectrum = FullSpectrum::new(vec![mix.clone()], physical);
        // Predicted color and the cycle.
        let predicted = mix_colours(colours, &mix);
        let auto = colour_name(display_colour(colours, &mix));
        let auto_name = self.auto_name(index, &mix);
        ui.add_space(4.);
        ui.horizontal(|ui| {
            gradient_swatch(ui, &predicted, 34.);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.;
                ui.label(
                    egui::RichText::new(format!("Predicted: {auto}"))
                        .font(theme::sans(12.))
                        .color(p.text),
                )
                .on_hover_text(format!(
                    "Approximate, from Prusa's measured color model ({}).",
                    predicted
                        .iter()
                        .map(|c| fdm_mixer::rgb_to_hex(*c))
                        .collect::<Vec<_>>()
                        .join(" → ")
                ));
                let shares = snapped(&d.percents);
                let cycle = recipe(0, &d.percents).build_sequence();
                ui.label(
                    egui::RichText::new(format!(
                        "Cycle {} · {}",
                        cycle_text(&cycle),
                        shares
                            .iter()
                            .enumerate()
                            .map(|(i, s)| format!("{} {:.0} %", letter(i), s))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .font(theme::mono(11.))
                    .color(p.muted_alt),
                )
                .on_hover_text(
                    "The layers repeat in this order; the shares are what this cycle prints",
                );
            });
        });
        if let Ok(s) = &spectrum
            && let Some((walls, inside)) = s.cycle(mix.id)
        {
            let pos = |seq: &[usize]| -> Vec<usize> {
                seq.iter()
                    .map(|t| {
                        mix.components
                            .iter()
                            .position(|c| c.extruder == *t)
                            .unwrap_or(0)
                            + 1
                    })
                    .collect()
            };
            if d.pattern == Pattern::OuterWallFirst {
                for (label, seq) in [("Walls ", walls), ("Inside", inside)] {
                    ui.label(
                        egui::RichText::new(format!("{label} {}", cycle_text(&pos(seq))))
                            .font(theme::mono(11.))
                            .color(p.muted_alt),
                    );
                }
            }
            if d.pattern == Pattern::Gradient {
                let end_cycle = recipe(0, &normalise(&d.end)).build_sequence();
                ui.label(
                    egui::RichText::new(format!(
                        "Bottom {} → top {}",
                        cycle_text(&pos(walls)),
                        cycle_text(&end_cycle)
                    ))
                    .font(theme::mono(11.))
                    .color(p.muted_alt),
                );
            }
            // Cost and hints.
            let per_layer = changes_per_layer(walls, inside);
            let config = self.effective_config();
            let layer = config.and_then(|c| c.float("layer_height")).unwrap_or(0.2);
            let seconds =
                seconds_per_change(config, &d.tools) * per_layer * (10. / layer.max(0.01));
            ui.add_space(2.);
            note(
                ui,
                p.label,
                &format!(
                    "About {} tool change{} per layer: roughly +{} per 10 mm of height",
                    trim(per_layer),
                    if (per_layer - 1.).abs() < 1e-9 {
                        ""
                    } else {
                        "s"
                    },
                    duration(seconds)
                ),
                "An estimate for this color alone; other colors on the same layers add their own changes, and each change also primes on the tower",
            );
            if layer > GOOD_LAYER_HEIGHT + 1e-9 {
                note(
                    ui,
                    p.warn_text,
                    &format!(
                        "Layers are {layer:.2} mm: colors blend better at {GOOD_LAYER_HEIGHT:.2} mm or thinner"
                    ),
                    "Thinner layers make the alternating colors read as one; Prusa recommends 0.10 to 0.15 mm",
                );
            }
            if let Some(id) = d.editing {
                let flat = self.flat_surfaces(id);
                if !flat.is_empty() {
                    note(
                        ui,
                        p.muted_alt,
                        &format!(
                            "{} has flat tops or bottoms: they print one tool per layer and look striped",
                            flat.join(", ")
                        ),
                        "Mixing works on walls; flat surfaces show whichever tool their layer uses",
                    );
                }
            }
        } else if let Err(e) = &spectrum {
            note(ui, p.warn_text, e, "");
        }
        // Name and buttons.
        ui.add_space(2.);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Name")
                    .font(theme::sans(12.))
                    .color(p.label),
            );
            let r = ui.add(
                egui::TextEdit::singleline(&mut d.name)
                    .hint_text(&auto_name)
                    .desired_width(ui.available_width()),
            );
            if r.changed() {
                d.named = !d.name.trim().is_empty();
            }
            r.on_hover_text("Leave empty to name the mix after its predicted color");
        });
        let problem = d.problem();
        if let Some(why) = problem {
            note(ui, p.warn_text, why, "");
        }
        ui.horizontal(|ui| {
            if widgets::primary_button(
                ui,
                if d.editing.is_some() {
                    "Apply"
                } else {
                    "Add mix"
                },
                problem.is_none() && spectrum.is_ok(),
            )
            .clicked()
            {
                acts.push(MixAct::Save(d.clone()));
            }
            if widgets::button(ui, "Cancel", true).clicked() {
                *close = true;
            }
        });
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn duration(seconds: f64) -> String {
    if seconds < 90. {
        format!("{:.0} s", seconds)
    } else {
        format!("{:.0} min", seconds / 60.)
    }
}

fn note(ui: &mut egui::Ui, colour: Color32, text: &str, tip: &str) {
    let r = ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::sans(11.))
                .color(colour),
        )
        .wrap(),
    );
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
}

/// A swatch of one color, or a bottom-to-top gradient of two.
pub fn gradient_swatch(ui: &mut egui::Ui, colours: &[[u8; 3]], size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_gradient(ui.painter(), rect, colours);
    ui.painter().rect_stroke(
        rect,
        3.,
        Stroke::new(1., theme::of(ui).border_raised),
        egui::StrokeKind::Inside,
    );
    response
}

fn paint_gradient(painter: &egui::Painter, rect: Rect, colours: &[[u8; 3]]) {
    let c = |rgb: [u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    match colours {
        [] => {}
        [one] => {
            painter.rect_filled(rect, 3., c(*one));
        }
        [bottom, top, ..] => {
            let steps = 8;
            for s in 0..steps {
                let t = s as f32 / (steps - 1) as f32;
                let mix = [0, 1, 2].map(|i| {
                    (f32::from(bottom[i]) + (f32::from(top[i]) - f32::from(bottom[i])) * t).round()
                        as u8
                });
                let h = rect.height() / steps as f32;
                let band = Rect::from_min_max(
                    Pos2::new(rect.left(), rect.bottom() - h * (s + 1) as f32),
                    Pos2::new(rect.right(), rect.bottom() - h * s as f32),
                );
                painter.rect_filled(band, 0., c(mix));
            }
        }
    }
}

/// Whether a placed mesh has flat (horizontal) faces above the bed, at least 4 mm² together.
pub fn has_flat_surfaces(mesh: &mica_core::mesh::TriangleMesh) -> bool {
    let bottom = mesh
        .vertices
        .iter()
        .map(|v| v[2])
        .fold(f32::INFINITY, f32::min);
    let mut area = 0f64;
    for f in &mesh.faces {
        let [a, b, c] = f.map(|i| mesh.vertices[i as usize].map(f64::from));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len <= 0. || n[2].abs() / len < 0.985 {
            continue;
        }
        let z = (a[2] + b[2] + c[2]) / 3.;
        if z - f64::from(bottom) < 0.05 {
            continue;
        }
        area += len / 2.;
    }
    area >= 4.
}

/// Slice-job objects with ColorMix turned off (Settings › Features): every mix prints in
/// its main color alone, as `mica slice` does (`mica_core::features::without_color_mix`).
pub fn main_colors_only(mut objects: Vec<crate::scene::Object>) -> Vec<crate::scene::Object> {
    for o in &mut objects {
        let spectrum = match (&o.mixes, &o.region) {
            (Some(m), _) => m.as_ref(),
            (None, Some(r)) => &r.full_spectrum,
            (None, None) => continue,
        };
        if !spectrum.extruders().is_empty() {
            o.mixes = Some(Arc::new(spectrum.main_colors_only()));
        }
    }
    objects
}

/// The tools of an imported object: a whole-object mix assignment comes back as the mix
/// (the import resolved it to a physical stand-in for slicing).
pub fn imported_tools(
    tools: ToolAssignments,
    config: &Config,
    region: Option<&mica_core::regions::RegionModel>,
) -> ToolAssignments {
    let Some(spectrum) = region.map(|r| &r.full_spectrum) else {
        return tools;
    };
    let mut out = tools;
    for (i, key) in [
        "perimeter_extruder",
        "infill_extruder",
        "solid_infill_extruder",
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(id) = config.int(key).filter(|id| *id > 0).map(|id| id as usize)
            && spectrum.contains(id)
        {
            out.0[i] = Some(id - 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_snap_to_the_exact_cycle() {
        assert_eq!(snapped(&[50., 50.]), vec![50., 50.]);
        assert_eq!(snapped(&[75., 25.]), vec![75., 25.]);
        let s = snapped(&[70., 30.]);
        assert!((s[0] - 500. / 7.).abs() < 1e-9, "{s:?}");
        let three = snapped(&[100. / 3.; 3]);
        assert!(three.iter().all(|v| (v - 100. / 3.).abs() < 1e-9));
        assert_eq!(
            cycle_text(&recipe(0, &[75., 25.]).build_sequence()),
            "A A B A"
        );
        // A snapped share snaps to itself.
        assert_eq!(snapped(&s), s);
    }

    #[test]
    fn sliders_keep_the_sum_at_100() {
        let mut v = vec![40., 30., 30.];
        v[0] = 60.;
        rebalance(&mut v, 0);
        assert!((v.iter().sum::<f64>() - 100.).abs() < 1e-9);
        assert!((v[1] - 20.).abs() < 1e-9);
        let mut v = vec![50., 50.];
        v[1] = 99.5;
        rebalance(&mut v, 1);
        assert!(v[0] >= 1.);
    }

    #[test]
    fn draft_saves_exact_shares_and_gradient_ends() {
        let mut d = Draft::new(4);
        d.tools = vec![0, 2];
        d.percents = vec![70., 30.];
        d.pattern = Pattern::Gradient;
        d.end = vec![20., 60.];
        let m = d.to_mix(5);
        assert_eq!(m.components[1].extruder, 3);
        assert!((m.components[0].ratio - 5. / 7.).abs() < 1e-9);
        assert!((m.end_ratios[0] - 0.25).abs() < 1e-9);
        assert!(FullSpectrum::new(vec![m], 4).is_ok());
        d.tools = vec![1, 1];
        assert!(d.problem().is_some());
    }

    #[test]
    fn red_and_yellow_predict_orange() {
        let colours = [[200, 30, 35], [245, 215, 40]];
        let c = predict(&colours, &[0, 1], &[0.5, 0.5]);
        assert_eq!(colour_name(c), "Orange", "{c:?}");
        assert_eq!(colour_name([250, 250, 250]), "White");
    }

    #[test]
    fn tool_changes_per_layer() {
        // A B: one change per layer.
        assert_eq!(changes_per_layer(&[1, 2], &[1, 2]), 1.);
        // A A B A: two changes per four layers.
        assert_eq!(changes_per_layer(&[1, 1, 2, 1], &[1, 1, 2, 1]), 0.5);
        // Walls and inside differ on every layer: still one change (last tool first).
        assert_eq!(changes_per_layer(&[1, 2], &[2, 1]), 1.);
    }

    /// A headless editor on the XL (five tools) with a cube.
    /// An app on the XL 5T with a cube; `None` (a reported skip) without the pinned profiles.
    fn xl_app(ctx: &egui::Context) -> Option<App> {
        use std::sync::atomic::{AtomicU64, Ordering};
        if !mica_testkit::require_prusa_profiles() {
            return None;
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mica-mixes-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::create(
            ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(dir)),
                files: vec![],
                headless: true,
            },
        );
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == "Original Prusa XL - 5T Input Shaper 0.4 nozzle")
            .expect("the XL 5T among the loaded printers");
        app.profiles.select_printer(vi, &name);
        app.settings_changed();
        let model = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/cube20.stl"
        ));
        let loaded = crate::app::load_files(vec![model], &app.config, app.bed.center());
        app.finish_load(loaded);
        Some(app)
    }

    /// GUI monkey test, seed 302: mixes follow a printer change, so a saved project opens
    /// again (a mix numbered 5 on the 4-tool U1 collided with the XL's physical tool 5).
    #[test]
    fn mixes_follow_a_printer_change_and_the_project_still_opens() {
        let ctx = egui::Context::default();
        let Some(mut app) = xl_app(&ctx) else {
            return;
        };
        if !mica_testkit::require_orca_profiles() {
            return;
        }
        let (u1, _, u1_name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == "Snapmaker U1 (0.4 nozzle)")
            .expect("the Snapmaker U1 among the loaded printers");
        let (xl, xl_name) = (app.profiles.vendor, app.profiles.printer.clone());
        app.add_mix_spec("1:50,2:50").unwrap();
        app.add_mix_spec("3:25,4:75").unwrap();
        // The cube prints with mix 2.
        app.plates.current_mut().objects[0].tools.0[0] = Some(6);
        let ids = |app: &App| app.mixes.list.iter().map(|m| m.id).collect::<Vec<_>>();
        let opens = |app: &App| {
            let text = app.mixes_json().expect("the mixes are saved");
            FullSpectrum::from_json(&text, app.tool_count()).map(|_| ())
        };
        assert_eq!(ids(&app), [6, 7]);
        app.switch_printer(u1, &u1_name);
        assert_eq!(app.tool_count(), 4);
        assert_eq!(ids(&app), [5, 6]);
        assert_eq!(
            app.plates.current().objects[0].tools.0[0],
            Some(5),
            "still mix 2"
        );
        opens(&app).unwrap();
        app.switch_printer(xl, &xl_name);
        assert_eq!(ids(&app), [6, 7]);
        assert_eq!(app.plates.current().objects[0].tools.0[0], Some(6));
        opens(&app).unwrap();
        // The opened file's own text is only reused while it fits the printer.
        let text = app.mixes_json().unwrap();
        app.mixes.original = Some((app.mixes.list.clone(), text));
        app.switch_printer(u1, &u1_name);
        opens(&app).unwrap();
    }

    #[test]
    fn deleting_a_mix_moves_its_users_and_renumbers_the_rest() {
        let ctx = egui::Context::default();
        let Some(mut app) = xl_app(&ctx) else {
            return;
        };
        assert_eq!(app.tool_count(), 5);
        app.add_mix_spec("1:50,2:50").unwrap();
        app.add_mix_spec("3:25,4:75/outer@Teal fade").unwrap();
        assert_eq!(
            app.mixes.list.iter().map(|m| m.id).collect::<Vec<_>>(),
            [6, 7]
        );
        let choices = app.tool_choices();
        assert_eq!(choices.len(), 7);
        assert_eq!(choices[6].label, "Teal fade");
        assert_eq!(choices[6].short, "M2");
        // The cube on mix 2, a facet painted with mix 1 and one with mix 2.
        let mut codes = vec![];
        mica_core::painting::selector::encode_state(6, &mut codes);
        let mut painting = mica_core::painting::FacetPainting::default();
        painting.material.push_codes(0, &codes).unwrap();
        codes.clear();
        mica_core::painting::selector::encode_state(7, &mut codes);
        painting.material.push_codes(1, &codes).unwrap();
        {
            let o = &mut app.plates.current_mut().objects[0];
            o.set_tool(6);
            o.painting = Arc::new(painting);
            o.painting_edited = true;
        }
        app.apply_mix_acts(vec![MixAct::Delete(6)]);
        // Mix 2 is now tool 6; mix 1's painting went to its main color, tool 1.
        assert_eq!(app.mixes.list.len(), 1);
        assert_eq!(app.mixes.list[0].id, 6);
        assert_eq!(app.mix_name(0), "Teal fade");
        let o = &app.plates.current().objects[0];
        assert_eq!(o.tools.0[0], Some(5));
        assert_eq!(o.painting.material.states(), vec![1, 6]);
        // One undo step brings both back.
        app.undo();
        assert_eq!(app.mixes.list.len(), 2);
        let o = &app.plates.current().objects[0];
        assert_eq!(o.tools.0[0], Some(6));
        assert_eq!(o.painting.material.states(), vec![6, 7]);
        // Swapping keeps every user on its mix.
        app.apply_mix_acts(vec![MixAct::Swap(6, 7)]);
        assert_eq!(app.mix_name(0), "Teal fade");
        let o = &app.plates.current().objects[0];
        assert_eq!(o.tools.0[0], Some(5));
        assert_eq!(o.painting.material.states(), vec![6, 7]);
        // The slice job carries the mixes; the plate's own objects do not.
        let job = app.slice_job(0);
        let crate::slice_cache::Job::Objects(objects, _) = job else {
            panic!("objects job");
        };
        assert!(objects[0].mixes.as_ref().is_some_and(|m| m.contains(7)));
        assert!(app.plates.current().objects[0].mixes.is_none());
    }

    #[test]
    fn flat_tops_are_found() {
        let mut cube = mica_core::mesh_edit::cuboid([10., 10., 10.]);
        assert!(has_flat_surfaces(&cube));
        // A cube's only flat faces are its top and its bed face; without a top, none.
        let top = cube.vertices.iter().map(|v| v[2]).fold(f32::MIN, f32::max);
        let vertices = cube.vertices.clone();
        cube.faces
            .retain(|f| !f.iter().all(|i| vertices[*i as usize][2] == top));
        assert!(!has_flat_surfaces(&cube));
    }
}
