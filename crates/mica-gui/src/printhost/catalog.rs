// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The printer models the Add printer flow offers: every system printer profile of the loaded
//! bundles, grouped into one entry per physical model ("Prusa XL") with its variants (tool
//! count, MMU, nozzle size and type, input shaper). The Gallery shows one card per model and
//! the Setup step picks the variant; a printer found on the network is matched to a model and
//! variant from what it reports (model name, tool count, nozzle diameter).
//!
//! Every Orca vendor (RatRig, Voron, Creality, Bambu Lab…) is listed without loading it:
//! `GalleryIndex` reads only the vendors' printer files, on a thread, and caches the result;
//! a vendor's full bundle is loaded when one of its printers is picked.
//!
//! Images: PrusaSlicer's bundle names a `thumbnail` per `[printer_model]`; Orca bundles ship
//! `<model name>_cover.png`. Both are looked up at runtime (`scene::bed_assets::asset_roots`):
//! the pinned upstream checkouts, the Orca subset a packaged Mica ships, the Prusa assets it
//! downloaded, and installed slicers.

use crate::panels::profiles::{Profiles, Vendor};
use mica_profiles::{orca, project::split_printer_name, prusa::Kind};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// A nozzle as the profiles name it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Nozzle {
    pub diameter: f64,
    /// `HF`, `HT` or empty (a standard nozzle).
    pub flavour: String,
    /// What the profile calls it (`HF0.4`, `0.4`, `HT Hotend 0.4`).
    pub raw: String,
}
impl Nozzle {
    pub fn from_variant(raw: &str, fallback: Option<f64>) -> Self {
        let r = raw.trim();
        let number: String = r
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let diameter = number.parse().ok().or(fallback).unwrap_or(0.4);
        let upper = r.to_ascii_uppercase();
        let flavour = if r.contains('+') {
            // Mixed nozzles per tool ("0.4+0.6").
            "+"
        } else if upper.starts_with("HF") {
            "HF"
        } else if upper.starts_with("HT") {
            "HT"
        } else {
            ""
        };
        Self {
            diameter,
            flavour: flavour.into(),
            raw: r.into(),
        }
    }
    /// `0.4 mm`, `HF 0.4 mm`.
    pub fn label(&self) -> String {
        let d = trim_number(self.diameter);
        if self.flavour == "+" {
            return format!("{} mm", self.raw.replace('+', " + "));
        }
        if self.flavour.is_empty() {
            format!("{d} mm")
        } else {
            format!("{} {d} mm", self.flavour)
        }
    }
    /// `0.4`, `HF 0.4` (the gallery chips).
    pub fn short(&self) -> String {
        let d = trim_number(self.diameter);
        if self.flavour == "+" {
            return self.raw.clone();
        }
        if self.flavour.is_empty() {
            d
        } else {
            format!("{} {d}", self.flavour)
        }
    }
    fn sort_key(&self) -> (i64, u8) {
        (
            (self.diameter * 1000.).round() as i64,
            match self.flavour.as_str() {
                "" => 0,
                "HF" => 1,
                "HT" => 2,
                _ => 3,
            },
        )
    }
}

fn trim_number(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// How many tools, and whether they are an MMU's filament slots on one nozzle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Tools {
    pub count: usize,
    /// `MMU3` for a multi-material unit, else `None`.
    pub mmu: Option<String>,
}
impl Tools {
    pub fn label(&self) -> String {
        match &self.mmu {
            Some(m) => m.clone(),
            None => self.count.to_string(),
        }
    }
}

/// One printer profile of a model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    /// The vendor's key in `Profiles::vendors` (`Prusa`, `Snapmaker`, `Ratrig`, `BBL`); the
    /// bundle is loaded when this variant is added (`Profiles::ensure_vendor`).
    pub vendor: String,
    pub preset: String,
    pub tools: Tools,
    pub nozzle: Nozzle,
    pub input_shaper: bool,
    /// The build size the model comes in (`300` of "RatRig V-Core 4 300", `250` of "Voron
    /// 2.4 250"); empty when the model has one size.
    #[serde(default)]
    pub size: String,
    /// Another variant of the same model and size: an IDEX printer's `Copy` / `Mirror` mode,
    /// Raise3D's `Left` / `Right` / `Dual` head; empty for the plain one.
    #[serde(default)]
    pub mode: String,
    /// The bundle's image file name for this variant's printer model.
    pub image: Option<String>,
    pub volume: Option<[f64; 3]>,
    /// Position of the nozzle in the printer model's own `variants` list (its preferred
    /// order: the CORE One lists HF0.4 first, the XL 0.4).
    pub nozzle_rank: usize,
}

/// One physical printer model with all its profiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    /// "Prusa XL", "Snapmaker U1", "RatRig V-Core 4".
    pub name: String,
    /// The vendor's key (`Prusa`, `Ratrig`, `BBL`).
    pub vendor: String,
    /// The brand as people say it ("Prusa", "RatRig", "Bambu Lab").
    pub brand: String,
    /// The folder of the vendor's pictures and bed models (`PrusaResearch`, `Ratrig`).
    pub asset_dir: String,
    pub variants: Vec<Variant>,
}

/// What the Setup step has chosen.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Choice {
    pub tools: Tools,
    pub nozzle: String,
    pub input_shaper: bool,
    pub size: String,
    pub mode: String,
}
impl Default for Tools {
    fn default() -> Self {
        Self {
            count: 1,
            mmu: None,
        }
    }
}

/// Sizes in numeric order ("200" < "1000").
fn size_key(s: &str) -> (u64, String) {
    (s.parse().unwrap_or(u64::MAX), s.to_string())
}

impl Variant {
    fn choice(&self) -> Choice {
        Choice {
            tools: self.tools.clone(),
            nozzle: self.nozzle.raw.clone(),
            input_shaper: self.input_shaper,
            size: self.size.clone(),
            mode: self.mode.clone(),
        }
    }
}

impl Model {
    /// The variant shown on the gallery card and preselected: input shaper, a single tool
    /// (fewest), the plain mode, the smallest size, a 0.4 mm nozzle of the model's preferred
    /// type.
    pub fn default_variant(&self) -> &Variant {
        self.variants
            .iter()
            .min_by_key(|v| {
                (
                    !v.input_shaper,
                    v.tools.mmu.is_some(),
                    v.tools.count,
                    !v.mode.is_empty(),
                    size_key(&v.size),
                    (v.nozzle.diameter - 0.4).abs() > 1e-3,
                    v.nozzle_rank,
                )
            })
            .expect("a model has variants")
    }
    pub fn default_choice(&self) -> Choice {
        self.default_variant().choice()
    }
    /// The tool setups this model comes in, fewest first, MMUs last.
    pub fn tool_options(&self) -> Vec<Tools> {
        let set: BTreeSet<(bool, usize, Option<String>)> = self
            .variants
            .iter()
            .map(|v| (v.tools.mmu.is_some(), v.tools.count, v.tools.mmu.clone()))
            .collect();
        set.into_iter()
            .map(|(_, count, mmu)| Tools { count, mmu })
            .collect()
    }
    /// Every nozzle of the model, by diameter (standard, HF, HT).
    pub fn nozzle_options(&self) -> Vec<Nozzle> {
        let mut out: Vec<Nozzle> = vec![];
        for v in &self.variants {
            if !out.iter().any(|n| n.raw == v.nozzle.raw) {
                out.push(v.nozzle.clone());
            }
        }
        out.sort_by_key(Nozzle::sort_key);
        out
    }
    /// The build sizes ("300", "400", "500"), smallest first; empty for a one-size model.
    pub fn size_options(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .variants
            .iter()
            .map(|v| v.size.clone())
            .filter(|s| !s.is_empty())
            .collect();
        out.sort_by_key(|s| size_key(s));
        out.dedup();
        out
    }
    /// The modes ("" for the plain one first, then "Copy", "Mirror"…); empty when the model
    /// has only the plain one.
    pub fn mode_options(&self) -> Vec<String> {
        let mut out: Vec<String> = self.variants.iter().map(|v| v.mode.clone()).collect();
        out.sort_by_key(|m| (!m.is_empty(), m.clone()));
        out.dedup();
        if out.len() < 2 { vec![] } else { out }
    }
    /// Whether the model comes both with and without input shaper.
    pub fn has_shaper_choice(&self) -> bool {
        self.variants.iter().any(|v| v.input_shaper)
            && self.variants.iter().any(|v| !v.input_shaper)
    }
    /// The profile for a choice, if that combination exists.
    pub fn resolve(&self, c: &Choice) -> Option<&Variant> {
        self.variants.iter().find(|v| {
            v.tools == c.tools
                && v.nozzle.raw == c.nozzle
                && v.size == c.size
                && v.mode == c.mode
                && (v.input_shaper == c.input_shaper || !self.has_shaper_choice())
        })
    }
    /// Whether `nozzle` exists with the chosen tools (and shaper, size, mode).
    pub fn nozzle_valid(&self, c: &Choice, nozzle: &str) -> bool {
        self.resolve(&Choice {
            nozzle: nozzle.to_string(),
            ..c.clone()
        })
        .is_some()
    }
    /// Makes `c` valid after one of its parts changed: keeps the tools, then the size and
    /// mode, then the nozzle diameter (closest flavour), then the shaper, else the first
    /// that fits.
    pub fn fix(&self, c: &mut Choice) {
        if self.resolve(c).is_some() {
            return;
        }
        let want = Nozzle::from_variant(&c.nozzle, None);
        let mut fits: Vec<&Variant> = self
            .variants
            .iter()
            .filter(|v| v.tools == c.tools)
            .collect();
        if fits.is_empty() {
            fits = self.variants.iter().collect();
        }
        if let Some(v) = fits.iter().min_by_key(|v| {
            (
                v.size != c.size,
                v.mode != c.mode,
                (v.nozzle.diameter - want.diameter).abs() > 1e-3,
                v.nozzle.flavour != want.flavour,
                v.input_shaper != c.input_shaper,
                v.nozzle_rank,
            )
        }) {
            *c = v.choice();
        }
    }
    /// "360 × 360 × 360 mm" from the default variant (empty when unknown).
    pub fn volume_text(&self) -> String {
        self.default_variant()
            .volume
            .map(|[x, y, z]| format!("{x:.0} × {y:.0} × {z:.0} mm"))
            .unwrap_or_default()
    }
    /// The gallery card's chips: tool counts, sizes, nozzles, features.
    pub fn chips(&self) -> Vec<(String, bool)> {
        let mut chips = vec![];
        let tools = self.tool_options();
        let plain: Vec<usize> = tools
            .iter()
            .filter(|t| t.mmu.is_none())
            .map(|t| t.count)
            .collect();
        match (plain.first(), plain.last()) {
            (Some(1), Some(1)) | (None, _) => {}
            (Some(a), Some(b)) if a == b => chips.push((format!("{a} tools"), true)),
            (Some(a), Some(b)) => chips.push((format!("{a}–{b} tools"), true)),
            _ => {}
        }
        let sizes = self.size_options();
        if sizes.len() > 1 {
            chips.push((sizes.join(" · "), true));
        }
        let nozzles = self.nozzle_options();
        let mut shown: Vec<String> = vec![];
        for n in &nozzles {
            let s = n.short();
            if !shown.contains(&s) {
                shown.push(s);
            }
        }
        let text = if shown.len() > 4 {
            format!("{} · +{}", shown[..4].join(" · "), shown.len() - 4)
        } else {
            shown.join(" · ")
        };
        if !text.is_empty() {
            chips.push((text, false));
        }
        if self.variants.iter().any(|v| v.input_shaper) {
            chips.push(("Input shaper".into(), false));
        }
        let modes: Vec<String> = self
            .mode_options()
            .into_iter()
            .filter(|m| !m.is_empty())
            .collect();
        if !modes.is_empty() {
            chips.push((modes.join(" / "), false));
        }
        for t in tools.iter().filter_map(|t| t.mmu.clone()) {
            chips.push((t, false));
        }
        chips
    }
}

/// Lower-case tokens of a model name without vendor words; `+` stays part of a token so
/// "XL+" is not "XL".
pub fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '+'))
        .map(|t| t.to_lowercase())
        .filter(|t| {
            !t.is_empty()
                && !matches!(
                    t.as_str(),
                    "original" | "prusa" | "snapmaker" | "input" | "shaper" | "nozzle" | "-"
                )
        })
        .collect()
}

/// A tool count named in a model string ("XL 5T", "XL - 2T").
fn tool_token(t: &str) -> Option<usize> {
    t.strip_suffix('t')
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|n| n.parse().ok())
}

/// The model part of a profile name with the variant words removed: "Original Prusa XL - 5T
/// Input Shaper" → ("Prusa XL", input shaper, MMU name).
pub fn base_name(model: &str) -> (String, bool, Option<String>) {
    let mut words: Vec<&str> = model.split_whitespace().collect();
    if words.first() == Some(&"Original") {
        words.remove(0);
    }
    let mut shaper = false;
    let mut mmu = None;
    let mut out: Vec<&str> = vec![];
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let lower = w.to_ascii_lowercase();
        if w == "Input" && words.get(i + 1) == Some(&"Shaper") {
            shaper = true;
            i += 2;
            continue;
        }
        if w == "-"
            && words
                .get(i + 1)
                .is_some_and(|n| tool_token(&n.to_ascii_lowercase()).is_some())
        {
            i += 2;
            continue;
        }
        if tool_token(&lower).is_some() || lower == "dual" {
            i += 1;
            continue;
        }
        if lower.starts_with("mmu") {
            mmu = Some(w.to_string());
            i += 1;
            continue;
        }
        out.push(w);
        i += 1;
    }
    (out.join(" "), shaper, mmu)
}

/// An Orca printer model name split into the model, its build size and its IDEX mode:
/// "RatRig V-Core 4 IDEX 300 COPY MODE" → ("RatRig V-Core 4 IDEX", "300", "Copy"),
/// "Voron 2.4 350" → ("Voron 2.4", "350", ""). A size is a last word of three digits after at
/// least two words, so "Comgrow T300", "SeeMeCNC BOSSdelta 500 0505" and "Vzbot 235 AWD" stay
/// models of their own.
pub fn split_size(model: &str) -> (String, String, String) {
    let mut words: Vec<&str> = model.split_whitespace().collect();
    let mut mode = String::new();
    if words.len() > 2 && words[words.len() - 1].eq_ignore_ascii_case("mode") {
        let m = words[words.len() - 2];
        if m.eq_ignore_ascii_case("copy") || m.eq_ignore_ascii_case("mirror") {
            let mut c = m.to_ascii_lowercase();
            c[..1].make_ascii_uppercase();
            mode = c;
            words.truncate(words.len() - 2);
        }
    }
    let mut size = String::new();
    if words.len() > 2
        && let Some(last) = words.last()
        && last.len() == 3
        && last.bytes().all(|b| b.is_ascii_digit())
    {
        size = last.to_string();
        words.pop();
    }
    if size.is_empty() && !mode.is_empty() {
        // A mode without a size word: keep the name whole.
        return (model.to_string(), String::new(), String::new());
    }
    (words.join(" "), size, mode)
}

/// All models of the loaded bundles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub models: Vec<Model>,
}

/// Models listed first in the gallery (current machines), in this order; the rest follow
/// by brand and name.
const FEATURED: [&str; 7] = [
    "Prusa CORE One",
    "Prusa XL",
    "Prusa MK4S",
    "Snapmaker U1",
    "Prusa CORE One L",
    "Prusa MINI",
    "Prusa MK3.9S",
];

/// Brands listed first, in this order; the rest alphabetically.
pub const POPULAR: [&str; 6] = [
    "Prusa",
    "Snapmaker",
    "Bambu Lab",
    "Creality",
    "RatRig",
    "Voron",
];

/// Orca vendor indexes the gallery leaves out: Orca's copy of the Prusa profiles (Mica has
/// PrusaSlicer's own) and Orca's test printer.
const SKIPPED_VENDORS: [&str; 2] = ["Prusa", "OrcaArena"];

fn brand_rank(brand: &str) -> (usize, String) {
    (
        POPULAR
            .iter()
            .position(|b| *b == brand)
            .unwrap_or(usize::MAX),
        brand.to_lowercase(),
    )
}

/// The brand of an Orca vendor index as people say it.
pub fn brand_name(key: &str, index_name: Option<&str>) -> String {
    match key {
        "BBL" => "Bambu Lab".into(),
        "Custom" => "Generic".into(),
        "Ratrig" => "RatRig".into(),
        _ => index_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(key)
            .trim()
            .to_string(),
    }
}

fn parse_volume(points: &[String], height: Option<f64>) -> Option<[f64; 3]> {
    let mut min = [f64::MAX; 2];
    let mut max = [f64::MIN; 2];
    for p in points {
        let (x, y) = p.trim().split_once('x')?;
        let (x, y): (f64, f64) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
        min = [min[0].min(x), min[1].min(y)];
        max = [max[0].max(x), max[1].max(y)];
    }
    if points.is_empty() {
        return None;
    }
    Some([
        (max[0] - min[0]).round(),
        (max[1] - min[1]).round(),
        height?.round(),
    ])
}

/// The variants of the PrusaSlicer bundle: (model name, variant).
fn prusa_variants(
    profiles: &Profiles,
    key: &str,
    b: &mica_profiles::prusa::Bundle,
) -> Vec<(String, Variant)> {
    let mut variants = vec![];
    for name in b.preset_names(Kind::Printer) {
        if profiles.is_user_preset(crate::panels::profiles::PresetKind::Printer, name) {
            continue;
        }
        let Ok(flat) = b.flatten(Kind::Printer, name) else {
            continue;
        };
        if flat.get("printer_technology").is_some_and(|t| t == "SLA") {
            continue;
        }
        let model_id = flat.get("printer_model").cloned().unwrap_or_default();
        if model_id.is_empty() {
            continue;
        }
        let section = b.printer_models.get(&model_id);
        let nozzles: Vec<String> = flat
            .get("nozzle_diameter")
            .map(|n| n.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();
        let raw_variant = flat
            .get("printer_variant")
            .cloned()
            .unwrap_or_else(|| split_printer_name(name).1);
        let nozzle =
            Nozzle::from_variant(&raw_variant, nozzles.first().and_then(|n| n.parse().ok()));
        let rank = section
            .and_then(|s| s.get("variants"))
            .and_then(|v| v.split(';').position(|x| x.trim() == raw_variant.trim()))
            .unwrap_or(99);
        // The preset is "<model> <variant>[ nozzle]"; the variant can be several words
        // ("Prusa CORE One HT Hotend 0.4").
        let model_text = name
            .strip_suffix(" nozzle")
            .unwrap_or(name)
            .strip_suffix(raw_variant.trim())
            .map(|n| n.trim_end_matches([' ', '-']).trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| split_printer_name(name).0);
        let (base, shaper, mmu) = base_name(&model_text);
        let mmu = mmu.filter(|_| {
            flat.get("single_extruder_multi_material")
                .is_some_and(|v| v == "1")
        });
        let shape: Vec<String> = flat
            .get("bed_shape")
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_default();
        variants.push((
            base,
            Variant {
                vendor: key.to_string(),
                preset: name.to_string(),
                tools: Tools {
                    count: nozzles.len().max(1),
                    mmu,
                },
                nozzle,
                input_shaper: shaper || model_id.ends_with("IS"),
                size: String::new(),
                mode: String::new(),
                image: section.and_then(|s| s.get("thumbnail")).cloned(),
                volume: parse_volume(
                    &shape,
                    flat.get("max_print_height").and_then(|h| h.parse().ok()),
                ),
                nozzle_rank: rank,
            },
        ));
    }
    variants
}

/// The system printers of an Orca vendor bundle (a full one or `load_vendor_machines`'):
/// (model name, variant).
pub fn orca_variants(key: &str, b: &orca::Bundle) -> Vec<(String, Variant)> {
    let mut variants = vec![];
    for name in b.preset_names(orca::Kind::Machine) {
        if b.preset(orca::Kind::Machine, name)
            .is_some_and(|p| p.is_user)
        {
            continue;
        }
        let Ok(flat) = b.flatten(orca::Kind::Machine, name) else {
            continue;
        };
        let text = |k: &str| -> Option<String> {
            let v = flat.get(k)?;
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_array()?.first()?.as_str().map(str::to_string))
        };
        let list = |k: &str| -> Vec<String> {
            match flat.get(k) {
                Some(serde_json::Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                Some(serde_json::Value::String(s)) => s.split(',').map(str::to_string).collect(),
                _ => vec![],
            }
        };
        let model_name = text("printer_model").unwrap_or_default();
        if model_name.is_empty() {
            continue;
        }
        let nozzles = list("nozzle_diameter");
        let raw_variant = text("printer_variant").unwrap_or_else(|| split_printer_name(name).1);
        let nozzle =
            Nozzle::from_variant(&raw_variant, nozzles.first().and_then(|n| n.parse().ok()));
        let rank = b
            .machine_model(&model_name)
            .and_then(|m| m.get("nozzle_diameter"))
            .and_then(|v| v.as_str())
            .and_then(|v| v.split(';').position(|x| x.trim() == raw_variant.trim()))
            .unwrap_or(99);
        let (base, shaper, mmu) = base_name(&model_name);
        let (base, size, mode) = split_size(&base);
        variants.push((
            base,
            Variant {
                vendor: key.to_string(),
                preset: name.to_string(),
                tools: Tools {
                    count: nozzles.len().max(1),
                    mmu: mmu
                        .filter(|_| text("single_extruder_multi_material").as_deref() == Some("1")),
                },
                nozzle,
                input_shaper: shaper,
                size,
                mode,
                image: Some(format!("{model_name}_cover.png")),
                volume: parse_volume(
                    &list("printable_area"),
                    text("printable_height").and_then(|h| h.parse().ok()),
                ),
                nozzle_rank: rank,
            },
        ));
    }
    variants
}

/// Groups one vendor's variants into models. A model name without the brand gets it in
/// front ("EXO42" → "Volumic EXO42"); presets that would choose alike ("Raise3D Pro3 0.4
/// nozzle (Left)" / "(Right)") are told apart by their mode.
fn group(key: &str, brand: &str, asset_dir: &str, variants: Vec<(String, Variant)>) -> Vec<Model> {
    let mut models: Vec<Model> = vec![];
    for (base, v) in variants {
        let lower = base.to_lowercase();
        let name = if lower.starts_with(&brand.to_lowercase())
            || lower.starts_with(&key.to_lowercase())
            || key == "Prusa"
        {
            base
        } else {
            format!("{brand} {base}")
        };
        match models.iter_mut().find(|m| m.name == name) {
            Some(m) => m.variants.push(v),
            None => models.push(Model {
                name,
                vendor: key.to_string(),
                brand: brand.to_string(),
                asset_dir: asset_dir.to_string(),
                variants: vec![v],
            }),
        }
    }
    for m in &mut models {
        let mut seen: Vec<Choice> = vec![];
        let collides: Vec<bool> = m
            .variants
            .iter()
            .map(|v| {
                let c = v.choice();
                let dup = seen.contains(&c);
                seen.push(c);
                dup
            })
            .collect();
        if !collides.iter().any(|c| *c) {
            continue;
        }
        let name = m.name.clone();
        for v in &mut m.variants {
            let others: Vec<&str> = name.split_whitespace().collect();
            let rest: Vec<String> = v
                .preset
                .split_whitespace()
                .filter(|w| {
                    !others.contains(w)
                        && !w.eq_ignore_ascii_case("nozzle")
                        && *w != v.nozzle.raw
                        && *w != v.size
                })
                .map(|w| w.trim_matches(['(', ')']).to_string())
                .filter(|w| !w.is_empty())
                .collect();
            let label = rest.join(" ");
            if !label.is_empty() {
                v.mode = if v.mode.is_empty() {
                    label
                } else {
                    format!("{} {label}", v.mode)
                };
            }
        }
    }
    models
}

/// The gallery's list of every vendor that is not loaded yet: their printers read from the
/// vendor indexes and machine files only (`orca::Bundle::load_vendor_machines`), off the UI
/// thread, and kept in a cache file next to Mica's settings until the profiles change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GalleryIndex {
    pub models: Vec<Model>,
    /// Vendors whose index could not be read, with the reason.
    #[serde(default)]
    pub problems: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    root: PathBuf,
    /// Each vendor index file's name, size and modification time (seconds).
    stamp: Vec<(String, u64, u64)>,
    index: GalleryIndex,
}

const CACHE_VERSION: u32 = 1;

fn stamp(root: &Path) -> Vec<(String, u64, u64)> {
    let mut out: Vec<(String, u64, u64)> = orca::discovery::vendor_indexes(root)
        .into_iter()
        .filter_map(|v| {
            let meta = std::fs::metadata(root.join(format!("{v}.json"))).ok()?;
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some((v, meta.len(), modified))
        })
        .collect();
    out.sort();
    out
}

impl GalleryIndex {
    /// Reads every printer vendor of an Orca `profiles` folder (except `SKIPPED_VENDORS`).
    pub fn build(root: &Path) -> Self {
        let mut index = Self::default();
        for key in orca::discovery::vendor_indexes(root) {
            if SKIPPED_VENDORS.contains(&key.as_str()) {
                continue;
            }
            let b = match orca::Bundle::load_vendor_machines(
                root.join(format!("{key}.json")),
                orca::Dialect::Orca242,
            ) {
                Ok(b) => b,
                Err(e) => {
                    index.problems.push(format!("{key}: {e}"));
                    continue;
                }
            };
            let brand = brand_name(
                &key,
                b.vendors
                    .get(&key)
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str()),
            );
            let variants = orca_variants(&key, &b);
            index.models.extend(group(&key, &brand, &key, variants));
        }
        index
    }

    /// The cached index when it was made from the same vendor files, else a fresh one (saved
    /// to `cache` when given; a cache that cannot be written is only slower next time).
    pub fn load_or_build(root: &Path, cache: Option<&Path>) -> Self {
        let now = stamp(root);
        if let Some(path) = cache
            && let Ok(text) = std::fs::read_to_string(path)
            && let Ok(file) = serde_json::from_str::<CacheFile>(&text)
            && file.version == CACHE_VERSION
            && file.root == root
            && file.stamp == now
        {
            return file.index;
        }
        let index = Self::build(root);
        if let Some(path) = cache {
            let file = CacheFile {
                version: CACHE_VERSION,
                root: root.to_path_buf(),
                stamp: now,
                index,
            };
            // Another Mica process may write the same cache: each writes its own temporary file.
            if let Ok(text) = serde_json::to_string(&file) {
                let _ = mica_core::shared_file::write(path, text.as_bytes());
            }
            return file.index;
        }
        index
    }
}

/// Builds the gallery index on a thread: `poll` hands it over once, `None` meanwhile.
#[derive(Default)]
pub struct IndexLoader {
    rx: Option<std::sync::mpsc::Receiver<GalleryIndex>>,
    pub index: Option<std::sync::Arc<GalleryIndex>>,
    /// The time the last build took (for the startup measurements).
    pub took: Option<std::time::Duration>,
    started: Option<std::time::Instant>,
}

impl IndexLoader {
    /// Starts reading the vendors (once); `repaint` is called when it is done.
    pub fn start(
        &mut self,
        root: Option<PathBuf>,
        cache: Option<PathBuf>,
        repaint: impl FnOnce() + Send + 'static,
    ) {
        if self.rx.is_some() || self.index.is_some() {
            return;
        }
        let Some(root) = root else {
            self.index = Some(Default::default());
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.started = Some(std::time::Instant::now());
        let spawned = std::thread::Builder::new()
            .name("mica-printer-gallery".into())
            .spawn(move || {
                let _ = tx.send(GalleryIndex::load_or_build(&root, cache.as_deref()));
                repaint();
            });
        if let Err(e) = spawned {
            self.rx = None;
            self.index = Some(std::sync::Arc::new(GalleryIndex {
                models: vec![],
                problems: vec![format!("The printer list could not be read: {e}")],
            }));
        }
    }
    /// True once when the index arrived.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(index) => {
                self.rx = None;
                self.took = self.started.map(|s| s.elapsed());
                self.index = Some(std::sync::Arc::new(index));
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                self.index = Some(std::sync::Arc::new(GalleryIndex {
                    models: vec![],
                    problems: vec!["Reading the printer list stopped unexpectedly.".into()],
                }));
                true
            }
        }
    }
    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }
    /// Waits for the index (tests and screenshots).
    pub fn wait(&mut self) {
        while self.loading() {
            if !self.poll() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

impl Catalog {
    /// Groups every system printer profile of the loaded bundles (user profiles are left to
    /// "All printer profiles…").
    pub fn build(profiles: &Profiles) -> Self {
        Self::build_with(profiles, None)
    }

    /// The loaded bundles' printers plus the index's of every vendor not loaded yet.
    pub fn build_with(profiles: &Profiles, index: Option<&GalleryIndex>) -> Self {
        let mut models: Vec<Model> = vec![];
        for (key, vendor) in &profiles.vendors {
            match vendor {
                Vendor::Prusa(b) => {
                    let v = prusa_variants(profiles, key, b);
                    models.extend(group(key, "Prusa", vendor.asset_dir(), v));
                }
                Vendor::Orca(b) => {
                    let brand = brand_name(
                        key,
                        b.vendors
                            .get(b.primary_vendor())
                            .and_then(|v| v.get("name"))
                            .and_then(|v| v.as_str()),
                    );
                    let v = orca_variants(key, b);
                    models.extend(group(key, &brand, vendor.asset_dir(), v));
                }
            }
        }
        if let Some(index) = index {
            models.extend(
                index
                    .models
                    .iter()
                    .filter(|m| profiles.vendor_index(&m.vendor).is_none())
                    .cloned(),
            );
        }
        for m in &mut models {
            m.variants.sort_by(|a, b| {
                (
                    a.tools.mmu.is_some(),
                    a.tools.count,
                    size_key(&a.size),
                    a.mode.clone(),
                    a.nozzle.sort_key(),
                    !a.input_shaper,
                )
                    .partial_cmp(&(
                        b.tools.mmu.is_some(),
                        b.tools.count,
                        size_key(&b.size),
                        b.mode.clone(),
                        b.nozzle.sort_key(),
                        !b.input_shaper,
                    ))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        models.sort_by(|a, b| {
            let rank = |m: &Model| {
                FEATURED
                    .iter()
                    .position(|f| *f == m.name)
                    .unwrap_or(usize::MAX)
            };
            (rank(a), brand_rank(&a.brand), a.name.to_lowercase()).cmp(&(
                rank(b),
                brand_rank(&b.brand),
                b.name.to_lowercase(),
            ))
        });
        Self { models }
    }

    /// The brands, popular ones first (`POPULAR`), then alphabetically.
    pub fn brands(&self) -> Vec<String> {
        let mut out: Vec<String> = self.models.iter().map(|m| m.brand.clone()).collect();
        out.sort_by_key(|b| brand_rank(b));
        out.dedup();
        out
    }

    /// The models matching a search (every word somewhere in the brand, the vendor's key or
    /// the model name) and brand.
    pub fn search(&self, query: &str, brand: Option<&str>) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.models
            .iter()
            .enumerate()
            .filter(|(_, m)| brand.is_none_or(|b| m.brand == b))
            .filter(|(_, m)| {
                let sizes = m.size_options().join(" ");
                let hay = format!("{} {} {} {sizes}", m.brand, m.vendor, m.name).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The model and variant a printer most likely is, from its vendor family (`None`: any
    /// brand), its reported (or guessed) model name, tool count and nozzle diameter. `None`:
    /// no model fits.
    pub fn match_printer(
        &self,
        vendor: Option<&str>,
        model: &str,
        tools: Option<usize>,
        nozzle: Option<f64>,
    ) -> Option<(usize, Choice)> {
        let got = tokens(model);
        let tools = tools.or_else(|| got.iter().find_map(|t| tool_token(t)));
        let (mi, _) = self
            .models
            .iter()
            .enumerate()
            .filter(|(_, m)| vendor.is_none_or(|v| m.brand.eq_ignore_ascii_case(v)))
            .filter_map(|(i, m)| {
                let (base, _, _) = base_name(&m.name);
                let want = tokens(&base);
                (!want.is_empty() && want.is_subset(&got)).then_some((i, want.len()))
            })
            .max_by_key(|(_, score)| *score)?;
        let m = &self.models[mi];
        // Several reported tools on a model that has no such tool head count: its MMU.
        let plain: Vec<usize> = m
            .variants
            .iter()
            .filter(|v| v.tools.mmu.is_none())
            .map(|v| v.tools.count)
            .collect();
        let prefer_mmu = tools.is_some_and(|t| t > 1 && !plain.contains(&t));
        let v = m.variants.iter().min_by_key(|v| {
            (
                tools.is_some_and(|t| v.tools.count != t) && !(prefer_mmu && v.tools.mmu.is_some()),
                v.tools.mmu.is_some() != prefer_mmu,
                !v.input_shaper,
                !v.mode.is_empty(),
                // A size the name states ("Voron 2.4 350").
                !(v.size.is_empty() || got.contains(&v.size)),
                // Unknown nozzle: the common 0.4 mm.
                (v.nozzle.diameter - nozzle.unwrap_or(0.4)).abs() > 1e-3,
                v.nozzle_rank,
            )
        })?;
        Some((mi, v.choice()))
    }

    /// The model a profile belongs to, and its choice.
    pub fn locate(&self, vendor: &str, preset: &str) -> Option<(usize, Choice)> {
        self.models.iter().enumerate().find_map(|(i, m)| {
            let v = m
                .variants
                .iter()
                .find(|v| v.vendor == vendor && v.preset == preset)?;
            Some((i, v.choice()))
        })
    }
}

/// Where a vendor's image file is (pinned upstream checkouts, installed slicers).
pub fn image_path(asset_dir: &str, file: &str) -> Option<PathBuf> {
    let roots = crate::scene::bed_assets::asset_roots(asset_dir);
    crate::scene::bed_assets::find_in(&roots, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_profiles::user::Store;

    /// The catalog of the pinned Prusa and Orca profiles; `None` (a reported skip) when they
    /// are not fetched.
    fn catalog() -> Option<(Profiles, Catalog)> {
        if !(mica_testkit::require_prusa_profiles() && mica_testkit::require_orca_profiles()) {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("mica-catalog-{}", std::process::id()));
        let profiles = Profiles::discover_with_store(Store::open(dir));
        assert!(profiles.vendors.len() >= 2, "the Prusa and Orca vendors");
        let c = Catalog::build(&profiles);
        Some((profiles, c))
    }

    fn model<'a>(c: &'a Catalog, name: &str) -> &'a Model {
        c.models.iter().find(|m| m.name == name).unwrap_or_else(|| {
            panic!(
                "no model {name}: {:?}",
                c.models.iter().map(|m| &m.name).collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn profile_names_lose_their_variant_words() {
        assert_eq!(
            base_name("Original Prusa XL - 5T Input Shaper"),
            ("Prusa XL".into(), true, None)
        );
        assert_eq!(
            base_name("Original Prusa XL"),
            ("Prusa XL".into(), false, None)
        );
        assert_eq!(
            base_name("Original Prusa MK4S MMU3"),
            ("Prusa MK4S".into(), false, Some("MMU3".into()))
        );
        assert_eq!(
            base_name("Prusa CORE One INDX - 8T").0,
            "Prusa CORE One INDX"
        );
        assert_eq!(
            base_name("Snapmaker A350 Dual QSKit").0,
            "Snapmaker A350 QSKit"
        );
        assert_eq!(base_name("Prusa XL+ 5T").0, "Prusa XL+");
        let n = Nozzle::from_variant("HF0.4", None);
        assert_eq!(
            (n.diameter, n.flavour.as_str(), n.label()),
            (0.4, "HF", "HF 0.4 mm".into())
        );
        assert_eq!(
            Nozzle::from_variant("HT Hotend 0.6", None).short(),
            "HT 0.6"
        );
        assert_eq!(Nozzle::from_variant("0.25", None).label(), "0.25 mm");
        assert_eq!(
            Nozzle::from_variant("0.4+0.6", None).label(),
            "0.4 + 0.6 mm"
        );
        assert!(tokens("Prusa XL+").contains("xl+"));
    }

    #[test]
    fn variants_group_into_one_model_each() {
        let Some((_, c)) = catalog() else { return };
        let xl = model(&c, "Prusa XL");
        let counts: Vec<String> = xl.tool_options().iter().map(Tools::label).collect();
        assert_eq!(counts, ["1", "2", "5"]);
        assert!(xl.has_shaper_choice());
        let nozzles: Vec<String> = xl.nozzle_options().iter().map(Nozzle::short).collect();
        for n in ["0.25", "0.4", "0.6", "0.8", "HF 0.4"] {
            assert!(nozzles.contains(&n.to_string()), "{nozzles:?}");
        }
        let five = Choice {
            tools: Tools {
                count: 5,
                mmu: None,
            },
            nozzle: "0.4".into(),
            input_shaper: true,
            ..Default::default()
        };
        assert_eq!(
            xl.resolve(&five).unwrap().preset,
            "Original Prusa XL - 5T Input Shaper 0.4 nozzle"
        );
        let plain = Choice {
            input_shaper: false,
            ..five.clone()
        };
        assert_eq!(
            xl.resolve(&plain).unwrap().preset,
            "Original Prusa XL - 5T 0.4 nozzle"
        );
        assert_eq!(
            xl.default_variant().preset,
            "Original Prusa XL Input Shaper 0.4 nozzle"
        );
        assert_eq!(c.models.iter().filter(|m| m.name == "Prusa XL").count(), 1);

        let core = model(&c, "Prusa CORE One");
        assert_eq!(core.default_variant().preset, "Prusa CORE One HF0.4 nozzle");
        // "Prusa CORE One HT Hotend 0.4" is a nozzle of the CORE One, not a model.
        assert!(c.models.iter().all(|m| !m.name.contains("HT")));
        assert!(core.nozzle_options().iter().any(|n| n.short() == "HT 0.4"));
        let tools: Vec<String> = core.tool_options().iter().map(Tools::label).collect();
        assert_eq!(tools[0], "1");
        assert!(tools.iter().any(|t| t.starts_with("MMU")), "{tools:?}");
        assert!(
            core.volume_text().starts_with("250 × 220 × 2"),
            "{}",
            core.volume_text()
        );

        let mk4s = model(&c, "Prusa MK4S");
        let tools: Vec<String> = mk4s.tool_options().iter().map(Tools::label).collect();
        assert_eq!(tools, ["1", "MMU3"]);
        // Only the nozzles that exist with the MMU are valid with it.
        let mut mmu = Choice {
            tools: mk4s.tool_options()[1].clone(),
            nozzle: "0.25".into(),
            input_shaper: mk4s.default_variant().input_shaper,
            ..Default::default()
        };
        assert!(!mk4s.nozzle_valid(&mmu, "0.25"));
        assert!(mk4s.nozzle_valid(&mmu, "0.4"));
        mk4s.fix(&mut mmu);
        assert!(mk4s.resolve(&mmu).unwrap().preset.contains("MMU3"));

        let u1 = model(&c, "Snapmaker U1");
        assert_eq!(u1.brand, "Snapmaker");
        let tools: Vec<String> = u1.tool_options().iter().map(Tools::label).collect();
        assert_eq!(tools, ["4"]);
        let nozzles: Vec<String> = u1.nozzle_options().iter().map(Nozzle::short).collect();
        for n in ["0.2", "0.4", "0.6", "0.8"] {
            assert!(nozzles.contains(&n.to_string()), "{nozzles:?}");
        }
        assert_eq!(nozzles[0], "0.2");
        assert_eq!(u1.default_variant().preset, "Snapmaker U1 (0.4 nozzle)");
        assert_eq!(
            u1.default_variant().image.as_deref(),
            Some("Snapmaker U1_cover.png")
        );
        let chips: Vec<String> = u1.chips().into_iter().map(|(t, _)| t).collect();
        assert_eq!(chips[0], "4 tools");
        // Featured models first.
        assert_eq!(c.models[0].name, "Prusa CORE One");
        assert_eq!(c.brands(), ["Prusa", "Snapmaker"]);
        let xls = c.search("xl", Some("Prusa"));
        assert!(!xls.is_empty());
        assert!(xls.iter().all(|&i| c.models[i].name.contains("XL")));
        assert!(c.search("u1", Some("Prusa")).is_empty());
    }

    #[test]
    fn discovered_printers_match_model_tools_and_nozzle() {
        let Some((_, c)) = catalog() else { return };
        let preset = |m: Option<(usize, Choice)>| {
            let (mi, choice) = m.expect("a match");
            c.models[mi].resolve(&choice).unwrap().preset.clone()
        };
        assert_eq!(
            preset(c.match_printer(Some("Prusa"), "Original Prusa XL 5T", None, Some(0.4))),
            "Original Prusa XL - 5T Input Shaper 0.4 nozzle"
        );
        assert_eq!(
            preset(c.match_printer(Some("Prusa"), "Original Prusa XL", Some(2), Some(0.6))),
            "Original Prusa XL - 2T Input Shaper 0.6 nozzle"
        );
        assert_eq!(
            preset(c.match_printer(Some("Prusa"), "Prusa CORE One", Some(1), Some(0.4))),
            "Prusa CORE One HF0.4 nozzle"
        );
        assert_eq!(
            preset(c.match_printer(Some("Prusa"), "Original Prusa MK4S", Some(5), None)),
            "Original Prusa MK4S MMU3 0.4 nozzle"
        );
        assert_eq!(
            preset(c.match_printer(Some("Snapmaker"), "Snapmaker U1", Some(4), Some(0.4))),
            "Snapmaker U1 (0.4 nozzle)"
        );
        assert_eq!(
            preset(c.match_printer(Some("Snapmaker"), "Snapmaker U1", None, Some(0.6))),
            "Snapmaker U1 (0.6 nozzle)"
        );
        assert!(
            c.match_printer(Some("Prusa"), "Voron 2.4", None, None)
                .is_none()
        );
        assert!(
            c.match_printer(Some("Snapmaker"), "Original Prusa XL", None, None)
                .is_none()
        );
        let (mi, choice) = c.locate("Prusa", "Prusa CORE One HF0.4 nozzle").unwrap();
        assert_eq!(c.models[mi].name, "Prusa CORE One");
        assert_eq!(choice.nozzle, "HF0.4");
    }

    /// The pinned OrcaSlicer profiles (`None`, a reported skip: not fetched in this checkout).
    fn orca_root() -> Option<PathBuf> {
        let root = mica_testkit::orca_profiles();
        mica_testkit::require_fixture(
            root.join("Ratrig.json"),
            "OrcaSlicer profiles: scripts/fetch-upstream.sh --profiles orca",
        )
        .then_some(root)
    }

    fn all_brands() -> Option<(Profiles, Catalog, GalleryIndex)> {
        let root = orca_root()?;
        let dir = std::env::temp_dir().join(format!("mica-catalog-all-{}", std::process::id()));
        let profiles = Profiles::discover_with_store(Store::open(dir));
        let index = GalleryIndex::build(&root);
        let c = Catalog::build_with(&profiles, Some(&index));
        Some((profiles, c, index))
    }

    #[test]
    fn sizes_and_idex_modes_split_off_model_names() {
        assert_eq!(
            split_size("RatRig V-Core 4 IDEX 300 COPY MODE"),
            ("RatRig V-Core 4 IDEX".into(), "300".into(), "Copy".into())
        );
        assert_eq!(
            split_size("RatRig V-Core 4 500"),
            ("RatRig V-Core 4".into(), "500".into(), String::new())
        );
        assert_eq!(
            split_size("Voron 2.4 350"),
            ("Voron 2.4".into(), "350".into(), String::new())
        );
        for whole in [
            "Comgrow T300",
            "SeeMeCNC BOSSdelta 500 0505",
            "Vzbot 235 AWD",
            "Creality K1 Max",
            "Wanhao D12-300",
        ] {
            assert_eq!(split_size(whole).0, whole);
        }
        assert_eq!(brand_name("BBL", Some("Bambulab")), "Bambu Lab");
        assert_eq!(brand_name("Ratrig", Some("RatRig")), "RatRig");
        assert_eq!(brand_name("Co Print", Some("Co Print")), "Co Print");
    }

    #[test]
    fn every_brand_is_listed_popular_first() {
        let Some((profiles, c, index)) = all_brands() else {
            return;
        };
        assert!(index.problems.is_empty(), "{:?}", index.problems);
        let brands = c.brands();
        assert_eq!(
            brands[..6],
            [
                "Prusa",
                "Snapmaker",
                "Bambu Lab",
                "Creality",
                "RatRig",
                "Voron"
            ]
        );
        let rest = &brands[6..];
        let mut sorted = rest.to_vec();
        sorted.sort_by_key(|b| b.to_lowercase());
        assert_eq!(rest, sorted);
        assert!(brands.len() > 50, "{brands:?}");
        // Orca's own copy of the Prusa profiles and its test printer stay out; the Snapmaker
        // bundle that is loaded is listed once.
        assert!(!brands.iter().any(|b| b.contains("Orca")));
        assert_eq!(
            c.models.iter().filter(|m| m.name == "Snapmaker U1").count(),
            1
        );
        assert_eq!(c.models.iter().filter(|m| m.name == "Prusa XL").count(), 1);
        // Nothing of RatRig is loaded until a printer is picked.
        assert!(profiles.vendor_index("Ratrig").is_none());
        // Search reaches brand, vendor key and model.
        assert!(
            !c.search("ratrig vcore", None).is_empty() || !c.search("v-core 4", None).is_empty()
        );
        let bbl = c.search("bbl", None);
        assert!(!bbl.is_empty() && bbl.iter().all(|&i| c.models[i].brand == "Bambu Lab"));
        assert!(!c.search("voron 2.4 350", None).is_empty());
        assert!(c.search("k1", Some("Creality")).len() >= 3);
    }

    #[test]
    fn ratrig_voron_and_creality_group_into_models() {
        let Some((_, c, _)) = all_brands() else {
            return;
        };
        let vc4 = model(&c, "RatRig V-Core 4");
        assert_eq!(vc4.brand, "RatRig");
        assert_eq!(vc4.vendor, "Ratrig");
        assert_eq!(vc4.asset_dir, "Ratrig");
        assert_eq!(vc4.size_options(), ["300", "400", "500"]);
        assert!(vc4.mode_options().is_empty());
        let nozzles: Vec<String> = vc4.nozzle_options().iter().map(Nozzle::short).collect();
        assert_eq!(nozzles, ["0.4", "0.5", "0.6", "0.8"]);
        let d = vc4.default_variant();
        assert_eq!(d.preset, "RatRig V-Core 4 300 0.4 nozzle");
        assert_eq!(d.image.as_deref(), Some("RatRig V-Core 4 300_cover.png"));
        assert_eq!(vc4.volume_text(), "300 × 300 × 300 mm");
        let mut big = vc4.default_choice();
        big.size = "500".into();
        big.nozzle = "0.6".into();
        assert_eq!(
            vc4.resolve(&big).unwrap().preset,
            "RatRig V-Core 4 500 0.6 nozzle"
        );
        let chips: Vec<String> = vc4.chips().into_iter().map(|(t, _)| t).collect();
        assert_eq!(chips[0], "300 · 400 · 500", "{chips:?}");
        // HYBRID and IDEX are their own machines; IDEX copy / mirror are modes of the IDEX.
        let idex = model(&c, "RatRig V-Core 4 IDEX");
        assert_eq!(idex.mode_options(), ["", "Copy", "Mirror"]);
        assert_eq!(idex.tool_options()[0].count, 2);
        let mut copy = idex.default_choice();
        assert_eq!(copy.mode, "");
        copy.mode = "Copy".into();
        copy.size = "400".into();
        idex.fix(&mut copy);
        assert_eq!(
            idex.resolve(&copy).unwrap().preset,
            "RatRig V-Core 4 IDEX 400 COPY MODE 0.4 nozzle"
        );
        model(&c, "RatRig V-Core 4 HYBRID");
        assert_eq!(
            model(&c, "RatRig V-Core 3").size_options(),
            ["200", "300", "400", "500"]
        );
        assert!(c.models.iter().all(|m| !m.name.contains("COPY")));

        let v24 = model(&c, "Voron 2.4");
        assert_eq!(v24.size_options(), ["250", "300", "350"]);
        assert!(v24.nozzle_options().len() >= 4);
        assert_eq!(
            model(&c, "Voron Trident").size_options(),
            ["250", "300", "350"]
        );
        model(&c, "Voron 0.1");

        for name in [
            "Creality K1",
            "Creality K1 Max",
            "Creality K1C",
            "Creality K2 Plus",
        ] {
            let m = model(&c, name);
            assert_eq!(m.brand, "Creality");
            assert!(m.size_options().is_empty(), "{name}");
        }
        // Models without the brand in their name get it.
        model(&c, "Volumic EXO42");
        // Presets that would choose alike are told apart (Raise3D's Left / Right / Dual).
        let pro3 = model(&c, "Raise3D Pro3");
        let mut choices: Vec<Choice> = pro3.variants.iter().map(Variant::choice).collect();
        let n = choices.len();
        choices.dedup();
        assert_eq!(choices.len(), n, "{choices:?}");
        // Every variant resolves to itself.
        for m in &c.models {
            for v in &m.variants {
                assert_eq!(
                    m.resolve(&v.choice()).map(|r| &r.preset),
                    Some(&v.preset),
                    "{}",
                    m.name
                );
            }
        }
    }

    #[test]
    fn found_klipper_printers_match_any_brand() {
        let Some((_, c, _)) = all_brands() else {
            return;
        };
        let (mi, choice) = c.match_printer(None, "Voron 2.4 350", None, None).unwrap();
        assert_eq!(c.models[mi].name, "Voron 2.4");
        assert_eq!(choice.size, "350");
        let (mi, _) = c
            .match_printer(None, "RatRig V-Core 4", None, Some(0.6))
            .unwrap();
        assert_eq!(c.models[mi].name, "RatRig V-Core 4");
        assert!(
            c.match_printer(Some("Prusa"), "Voron 2.4", None, None)
                .is_none()
        );
    }

    #[test]
    fn the_gallery_index_loads_lazily_and_is_cached() {
        let Some(root) = orca_root() else { return };
        let dir = std::env::temp_dir().join(format!("mica-gallery-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("cache/printer-gallery.json");
        let mut loader = IndexLoader::default();
        assert!(!loader.loading() && loader.index.is_none());
        loader.start(Some(root.clone()), Some(cache.clone()), || {});
        assert!(loader.loading() || loader.index.is_some());
        loader.wait();
        let first = loader.index.clone().expect("built");
        assert!(first.models.len() > 300, "{}", first.models.len());
        assert!(cache.is_file());
        // A second run reads the cache: the same models, much faster than reading the vendors.
        let start = std::time::Instant::now();
        let again = GalleryIndex::load_or_build(&root, Some(&cache));
        let cached = start.elapsed();
        assert_eq!(again, *first);
        let start = std::time::Instant::now();
        let fresh = GalleryIndex::build(&root);
        let built = start.elapsed();
        assert_eq!(fresh, *first);
        eprintln!("gallery index: built in {built:?}, read from cache in {cached:?}");
        assert!(cached < built, "cache {cached:?} vs build {built:?}");
        // A cache from another folder is not used.
        let other = dir.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        assert!(
            GalleryIndex::load_or_build(&other, Some(&cache))
                .models
                .is_empty()
        );
        // No Orca profiles at all: an empty index, at once.
        let mut none = IndexLoader::default();
        none.start(None, None, || {});
        assert!(!none.loading());
        assert!(none.index.as_ref().unwrap().models.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
