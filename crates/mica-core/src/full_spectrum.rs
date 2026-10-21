// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Feature/FullSpectrum/VirtualExtruder.cpp :: quantise_component_counts, build_canonical_cycle, VirtualExtruder::build_sequence, normalize_virtual_extruders, source_virtual_extruder_in_region_config, remap_virtual_extruders_to_physical, remap_virtual_region_slices_to_physical, serialize_virtual_extruders_to_json
// Source: prusaslicer@b028299c77 src/libslic3r/Feature/FullSpectrum/VirtualExtruder.hpp :: VirtualExtruderComponent, VirtualExtruder
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/MixedFilament.cpp :: MixedFilamentManager::resolve_perimeter, MixedFilamentManager::ordered_perimeter_extruders
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/MixedFilament.hpp :: MixedFilament::gradient_start, MixedFilament::gradient_end
// Changes: Blend engine with validated Rust definitions, cached cycles, existing JSON codec and strict diagnostics instead of silently dropping malformed recipes. Mica patterns on every printer: Layers is Prusa's exact cycle; Outer wall first takes Snapmaker's per-wall groups (walls vs. inside) but derives both sequences from the exact cycle so the overall share stays exact; Gradient interpolates start→end shares over the mix's own layer span by error diffusion (not Snapmaker's Local-Z passes or Prusa's banded sub-cycles). Mica-only fields live in a "mica" object PrusaSlicer ignores. Original shared snapping/prediction wrappers for GUI and MCP. No printer-change ID migration. main_colors_only (each mix as its main color alone) is Mica's, for ColorMix turned off in Preferences.

//! Mixed colours (PrusaSlicer 2.9.6 ColorMix "virtual extruders"). IDs are one-based and follow
//! the physical tools; the cycle phase is the object-local layer index.
use crate::{config::Config, threemf::json};
use std::collections::BTreeMap;
use std::ops::Range;

pub const ARCHIVE_PATH: &str = "Metadata/Prusa_Slicer_full_spectrum.json";

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub extruder: usize,
    pub ratio: f64,
}

/// How a mixed colour spreads its tools over the layers (Mica; PrusaSlicer prints every
/// pattern as Layers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pattern {
    /// Each layer prints in one tool, following the exact cycle (PrusaSlicer's behaviour).
    #[default]
    Layers,
    /// The walls use the main colour more often and the inside (infill, top and bottom) makes
    /// up the difference, so the whole mix keeps its exact share.
    OuterWallFirst,
    /// The share changes linearly from the start ratios to `end_ratios` over the height the
    /// mix covers in each object.
    Gradient,
}

impl Pattern {
    pub const ALL: [Pattern; 3] = [Pattern::Layers, Pattern::OuterWallFirst, Pattern::Gradient];
    /// The 3MF value.
    pub fn key(self) -> &'static str {
        match self {
            Pattern::Layers => "layers",
            Pattern::OuterWallFirst => "outer_wall_first",
            Pattern::Gradient => "gradient",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }
    /// The user-visible name.
    pub fn label(self) -> &'static str {
        match self {
            Pattern::Layers => "Layers",
            Pattern::OuterWallFirst => "Outer wall first",
            Pattern::Gradient => "Gradient",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VirtualExtruder {
    pub id: usize,
    pub color: Option<String>,
    pub components: Vec<Component>,
    /// Mica: the user's name for the mix.
    pub name: Option<String>,
    /// Mica: how the tools alternate.
    pub pattern: Pattern,
    /// Mica, [`Pattern::Gradient`] only: the shares at the top, one per component (the
    /// components' `ratio` are the shares at the bottom).
    pub end_ratios: Vec<f64>,
}

/// The physical tools of one layer of a mixed region: walls, and inside (infill, solid infill).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    pub walls: usize,
    pub inside: usize,
}

/// Upstream's count quantisation (2..64 layers, ≤ 3 % error per component) and GCD reduction.
fn quantise(ratios: &[f64]) -> Vec<usize> {
    let total: f64 = ratios.iter().sum();
    let mut counts = vec![];
    for candidate in 2..=64 {
        counts = ratios
            .iter()
            .map(|r| ((r / total * f64::from(candidate)).round() as usize).max(1))
            .collect();
        let sum = counts.iter().sum::<usize>() as f64;
        let error = ratios
            .iter()
            .zip(&counts)
            .map(|(r, n)| (r / total - *n as f64 / sum).abs())
            .fold(0., f64::max);
        if error <= 0.03 {
            break;
        }
    }
    fn gcd(mut a: usize, mut b: usize) -> usize {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    }
    let divisor = counts.iter().copied().reduce(gcd).unwrap_or(1);
    for n in &mut counts {
        *n /= divisor;
    }
    counts
}

/// Upstream's deficit interleave: slot by slot, the component furthest behind its share
/// (strict `>`, so ties go to the first component).
fn interleave(counts: &[usize]) -> Vec<usize> {
    let length: usize = counts.iter().sum();
    let mut emitted = vec![0; counts.len()];
    (0..length)
        .map(|slot| {
            let mut best = 0;
            let mut deficit = f64::NEG_INFINITY;
            for (i, count) in counts.iter().enumerate() {
                let d = ((slot + 1) * count) as f64 / length as f64 - emitted[i] as f64;
                if d > deficit {
                    best = i;
                    deficit = d;
                }
            }
            emitted[best] += 1;
            best
        })
        .collect()
}

impl VirtualExtruder {
    /// The mix's main color: the component with the largest share (the first on a tie).
    pub fn main_extruder(&self) -> usize {
        self.components
            .iter()
            .fold(None::<&Component>, |best, c| match best {
                Some(b) if b.ratio >= c.ratio => Some(b),
                _ => Some(c),
            })
            .map_or(1, |c| c.extruder)
    }

    /// The active components (positive share) and their quantised layer counts.
    fn counts(&self) -> (Vec<usize>, Vec<usize>) {
        let active: Vec<usize> = self
            .components
            .iter()
            .filter(|c| c.ratio > 0.)
            .map(|c| c.extruder)
            .collect();
        if active.len() < 2 {
            return (active.clone(), vec![1; active.len()]);
        }
        let ratios: Vec<f64> = self
            .components
            .iter()
            .filter(|c| c.ratio > 0.)
            .map(|c| c.ratio)
            .collect();
        (active, quantise(&ratios))
    }

    /// Exact upstream quantisation and deficit tie-breaking, including zero-weight filtering.
    pub fn build_sequence(&self) -> Vec<usize> {
        let (active, counts) = self.counts();
        if active.len() < 2 {
            return active;
        }
        interleave(&counts).into_iter().map(|i| active[i]).collect()
    }

    /// The display swatch shared by GUI and MCP: a gradient's midpoint, else its ratio.
    /// Uses the existing filament predictor; never changes the slicing recipe.
    pub fn predicted_rgb(&self, physical: &[[u8; 3]]) -> [u8; 3] {
        let parts = self
            .components
            .iter()
            .enumerate()
            .map(|(i, c)| crate::fdm_mixer::Part {
                rgb: physical
                    .get(c.extruder.saturating_sub(1))
                    .copied()
                    .unwrap_or([128; 3]),
                ratio: if self.pattern == Pattern::Gradient
                    && self.end_ratios.len() == self.components.len()
                {
                    (c.ratio + self.end_ratios[i]) / 2.
                } else {
                    c.ratio
                },
            })
            .collect::<Vec<_>>();
        crate::fdm_mixer::mix_rgb(&parts).unwrap_or([128; 3])
    }

    /// The shares actually printed by the exact cycle, in percent and component order.
    pub fn snapped_percents(&self) -> Vec<f64> {
        let cycle = self.build_sequence();
        let len = cycle.len().max(1) as f64;
        self.components
            .iter()
            .map(|c| cycle.iter().filter(|t| **t == c.extruder).count() as f64 * 100. / len)
            .collect()
    }

    /// [`Pattern::OuterWallFirst`]: `(walls, inside)` sequences of equal length.
    ///
    /// With a main colour (one component with the most layers in the exact cycle of length
    /// L), both sequences span 2L layers: every other colour gives half of its wall layers to
    /// the main colour and prints them inside instead. Per component, wall and inside layers
    /// together are exactly twice its cycle count, so the share stays exact. Without a main
    /// colour (50/50, 33/33/33) the walls follow the cycle and the inside the cycle shifted so
    /// that the two differ on as many layers as possible.
    pub fn wall_sequences(&self) -> (Vec<usize>, Vec<usize>) {
        let (active, counts) = self.counts();
        if active.len() < 2 {
            return (active.clone(), active);
        }
        let max = *counts.iter().max().expect("two components");
        let main: Vec<usize> = (0..counts.len()).filter(|&i| counts[i] == max).collect();
        let map = |seq: Vec<usize>| -> Vec<usize> { seq.into_iter().map(|i| active[i]).collect() };
        if main.len() > 1 {
            let walls = interleave(&counts);
            let len = walls.len();
            let shift = (1..len)
                .max_by_key(|&s| {
                    let differ = (0..len)
                        .filter(|&i| walls[i] != walls[(i + s) % len])
                        .count();
                    (differ, std::cmp::Reverse(s))
                })
                .unwrap_or(0);
            let inside = (0..len).map(|i| walls[(i + shift) % len]).collect();
            return (map(walls), map(inside));
        }
        let d = main[0];
        let given: usize = (0..counts.len())
            .filter(|&i| i != d)
            .map(|i| counts[i])
            .sum();
        let walls: Vec<usize> = (0..counts.len())
            .map(|i| {
                if i == d {
                    2 * counts[i] + given
                } else {
                    counts[i]
                }
            })
            .collect();
        let inside: Vec<usize> = (0..counts.len())
            .map(|i| {
                if i == d {
                    2 * counts[i] - given
                } else {
                    3 * counts[i]
                }
            })
            .collect();
        (map(interleave(&walls)), map(interleave(&inside)))
    }

    /// [`Pattern::Gradient`] over `len` layers: the share moves linearly from the start ratios
    /// (first layer) to `end_ratios` (last layer); each layer takes the component furthest
    /// behind its accumulated target share (ties to the first component), so every prefix of
    /// the span is within one layer of the ideal count.
    pub fn gradient_sequence(&self, len: usize) -> Vec<usize> {
        let n = self.components.len();
        let start: Vec<f64> = self.components.iter().map(|c| c.ratio.max(0.)).collect();
        let end: Vec<f64> = if self.end_ratios.len() == n {
            self.end_ratios.iter().map(|r| r.max(0.)).collect()
        } else {
            start.clone()
        };
        let norm = |v: &[f64]| {
            let s: f64 = v.iter().sum();
            if s > 0. {
                v.iter().map(|x| x / s).collect::<Vec<_>>()
            } else {
                let mut out = vec![0.; v.len()];
                out[0] = 1.;
                out
            }
        };
        let (start, end) = (norm(&start), norm(&end));
        let mut target = vec![0.; n];
        let mut emitted = vec![0.; n];
        (0..len)
            .map(|j| {
                let t = if len > 1 {
                    j as f64 / (len - 1) as f64
                } else {
                    0.
                };
                let mut best = 0;
                let mut deficit = f64::NEG_INFINITY;
                for i in 0..n {
                    target[i] += start[i] + (end[i] - start[i]) * t;
                    let d = target[i] - emitted[i];
                    if d > deficit + 1e-12 {
                        best = i;
                        deficit = d;
                    }
                }
                emitted[best] += 1.;
                self.components[best].extruder
            })
            .collect()
    }
}

/// Per mix: walls and inside sequences (equal for Layers and Gradient's plain fallback).
#[derive(Clone, Debug, Default, PartialEq)]
struct Plan {
    walls: Vec<usize>,
    inside: Vec<usize>,
}

/// Validated recipes and their cached cycles. Never adds physical filament/profile slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FullSpectrum {
    extruders: Vec<VirtualExtruder>,
    plans: Vec<Plan>,
}

impl FullSpectrum {
    pub fn new(mut extruders: Vec<VirtualExtruder>, physical_count: usize) -> Result<Self, String> {
        let mut ids = std::collections::BTreeSet::new();
        for ve in &mut extruders {
            if ve.id <= physical_count || ve.id > 255 || !ids.insert(ve.id) {
                return Err(format!(
                    "invalid or duplicate virtual extruder ID {} (physical tools: {physical_count})",
                    ve.id
                ));
            }
            if ve.components.iter().any(|c| !c.ratio.is_finite())
                || ve.end_ratios.iter().any(|r| !r.is_finite() || *r < 0.)
            {
                return Err(format!("non-finite ratio for virtual extruder {}", ve.id));
            }
            if ve.pattern == Pattern::Gradient {
                if ve.end_ratios.len() != ve.components.len() {
                    return Err(format!(
                        "mixed color {} needs one end ratio per component for its gradient",
                        ve.id
                    ));
                }
                if ve.components.iter().any(|c| c.ratio < 0.) {
                    return Err(format!("negative ratio in mixed color {}", ve.id));
                }
            } else {
                ve.end_ratios.clear();
            }
            ve.components.retain(|c| c.ratio >= 0.);
            if !(2..=3).contains(&ve.components.len())
                || ve
                    .components
                    .iter()
                    .any(|c| c.extruder == 0 || c.extruder > physical_count || !c.ratio.is_finite())
            {
                return Err(format!(
                    "virtual extruder {} requires 2–3 finite components referencing physical tools 1..={physical_count}",
                    ve.id
                ));
            }
            let sum: f64 = ve.components.iter().map(|c| c.ratio).sum();
            if !sum.is_finite() {
                return Err(format!("invalid ratio sum for virtual extruder {}", ve.id));
            }
            if (sum - 1.).abs() > 1e-6 && sum > 0. {
                for c in &mut ve.components {
                    c.ratio /= sum;
                }
            }
            let end: f64 = ve.end_ratios.iter().sum();
            if ve.pattern == Pattern::Gradient && end <= 0. {
                return Err(format!("mixed color {} has no end ratio", ve.id));
            }
            if (end - 1.).abs() > 1e-6 && end > 0. {
                for r in &mut ve.end_ratios {
                    *r /= end;
                }
            }
        }
        let plans = extruders
            .iter()
            .map(|ve| {
                let cycle = ve.build_sequence();
                // Upstream resolve_all_layers falls back to the first component for all-zero ratios.
                let cycle = if cycle.is_empty() {
                    vec![ve.components[0].extruder]
                } else {
                    cycle
                };
                match ve.pattern {
                    Pattern::OuterWallFirst if cycle.len() > 1 => {
                        let (walls, inside) = ve.wall_sequences();
                        Plan { walls, inside }
                    }
                    _ => Plan {
                        walls: cycle.clone(),
                        inside: cycle,
                    },
                }
            })
            .collect();
        Ok(Self { extruders, plans })
    }

    /// Reads `Metadata/Prusa_Slicer_full_spectrum.json`, including Mica's own fields.
    pub fn from_json(text: &str, physical_count: usize) -> Result<Self, String> {
        Self::parse_json(text, physical_count).map_err(|e| format!("{ARCHIVE_PATH}: {e}"))
    }

    fn parse_json(text: &str, physical_count: usize) -> Result<Self, String> {
        use crate::threemf::invalid;
        let parse = || -> Result<Vec<VirtualExtruder>, crate::threemf::ThreeMfError> {
            let root = json::parse(text)?;
            if root.get("version")?.num::<usize>()? != 1 {
                return Err(invalid("unsupported Full Spectrum version (expected 1)"));
            }
            let mut out = vec![];
            for entry in root.get("virtual_extruders")?.array()? {
                let components = entry.get("components")?.array()?;
                if entry.get("kind").ok().and_then(|v| v.text().ok()) == Some("gradient")
                    || components.iter().any(|c| c.get("position").is_ok())
                {
                    return Err(invalid(
                        "PrusaSlicer ColorMix gradients are not supported (use a Mica gradient mix)",
                    ));
                }
                let mica = entry.get("mica").ok();
                let field = |key: &str| mica.and_then(|m| m.get(key).ok());
                let pattern = match field("pattern") {
                    None => Pattern::Layers,
                    Some(v) => Pattern::from_key(v.text()?).ok_or_else(|| {
                        invalid(format!("unknown mixed color pattern {:?}", v.text().ok()))
                    })?,
                };
                let end_ratios = match field("end_ratios") {
                    None => vec![],
                    Some(v) => v
                        .array()?
                        .iter()
                        .map(|r| r.num())
                        .collect::<Result<_, _>>()?,
                };
                out.push(VirtualExtruder {
                    id: entry.get("id")?.num()?,
                    color: entry
                        .get("color")
                        .ok()
                        .and_then(|v| v.text().ok())
                        .map(str::to_owned),
                    components: components
                        .iter()
                        .map(|c| {
                            Ok(Component {
                                extruder: c.get("extruder")?.num()?,
                                ratio: c.get("ratio").map_or(Ok(0.), |v| v.num())?,
                            })
                        })
                        .collect::<Result<_, crate::threemf::ThreeMfError>>()?,
                    name: field("name")
                        .map(|v| v.text().map(str::to_owned))
                        .transpose()?,
                    pattern,
                    end_ratios,
                });
            }
            Ok(out)
        };
        Self::new(parse().map_err(|e| e.to_string())?, physical_count)
    }

    /// The project file text: PrusaSlicer's format (`kind` fullspectrum, start ratios), with
    /// Mica's name, pattern and gradient end in a `mica` object PrusaSlicer ignores, so
    /// PrusaSlicer opens every mix as plain layer mixing. `physical` are the tools' colours.
    pub fn to_json(&self, physical: &[String]) -> String {
        use json::Json;
        let number = |r: f64| Json::number(r);
        let physical = physical
            .iter()
            .enumerate()
            .map(|(i, c)| {
                Json::object([
                    ("id", Json::number(i + 1)),
                    ("color", Json::from(c.as_str())),
                ])
            })
            .collect();
        let virtuals = self
            .extruders
            .iter()
            .map(|ve| {
                let mut entry = vec![
                    ("id", Json::number(ve.id)),
                    ("kind", Json::from("fullspectrum")),
                    (
                        "components",
                        Json::Array(
                            ve.components
                                .iter()
                                .map(|c| {
                                    Json::object([
                                        ("extruder", Json::number(c.extruder)),
                                        ("ratio", number(c.ratio)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ];
                if let Some(c) = &ve.color {
                    entry.push(("color", Json::from(c.as_str())));
                }
                let mut mica = vec![];
                if let Some(name) = &ve.name {
                    mica.push(("name", Json::from(name.as_str())));
                }
                if ve.pattern != Pattern::Layers {
                    mica.push(("pattern", Json::from(ve.pattern.key())));
                }
                if ve.pattern == Pattern::Gradient {
                    mica.push((
                        "end_ratios",
                        Json::Array(ve.end_ratios.iter().map(|r| number(*r)).collect()),
                    ));
                }
                if !mica.is_empty() {
                    entry.push(("mica", Json::object(mica)));
                }
                Json::object(entry)
            })
            .collect();
        let mut text = Json::object([
            ("version", Json::number(1)),
            ("physical_extruders", Json::Array(physical)),
            ("virtual_extruders", Json::Array(virtuals)),
        ])
        .encode();
        text.push('\n');
        text
    }

    /// Every mix printed in its main color alone (ColorMix turned off in Settings ›
    /// Features): the mix numbers stay defined, so parts, layer ranges and paint on a mix print
    /// with its main filament and the other filaments of its recipe are not used.
    pub fn main_colors_only(&self) -> Self {
        let extruders: Vec<VirtualExtruder> = self
            .extruders
            .iter()
            .map(|ve| VirtualExtruder {
                components: vec![Component {
                    extruder: ve.main_extruder(),
                    ratio: 1.,
                }],
                pattern: Pattern::Layers,
                end_ratios: vec![],
                ..ve.clone()
            })
            .collect();
        let plans = extruders
            .iter()
            .map(|ve| Plan {
                walls: vec![ve.components[0].extruder],
                inside: vec![ve.components[0].extruder],
            })
            .collect();
        Self { extruders, plans }
    }

    pub fn extruders(&self) -> &[VirtualExtruder] {
        &self.extruders
    }
    pub fn get(&self, id: usize) -> Option<&VirtualExtruder> {
        self.extruders.iter().find(|ve| ve.id == id)
    }
    pub fn contains(&self, id: usize) -> bool {
        self.extruders.iter().any(|ve| ve.id == id)
    }
    pub fn max_id(&self) -> usize {
        self.extruders.iter().map(|ve| ve.id).max().unwrap_or(0)
    }
    /// The walls' physical tool on `layer` (the whole region's for Layers; a gradient's
    /// start-ratio cycle, which is also what PrusaSlicer prints).
    pub fn resolve(&self, id: usize, layer: usize) -> usize {
        self.roles(id, layer, None).walls
    }
    /// The physical tools of `layer` (object-local index). `span` is the object's layer range
    /// holding the mix, which a gradient spreads over; without it a gradient prints its
    /// start-ratio cycle.
    pub fn roles(&self, id: usize, layer: usize, span: Option<&Range<usize>>) -> Roles {
        let Some(i) = self.extruders.iter().position(|ve| ve.id == id) else {
            return Roles {
                walls: id,
                inside: id,
            };
        };
        let ve = &self.extruders[i];
        if ve.pattern == Pattern::Gradient
            && let Some(span) = span.filter(|s| s.contains(&layer))
        {
            let tool = ve.gradient_sequence(span.len())[layer - span.start];
            return Roles {
                walls: tool,
                inside: tool,
            };
        }
        let plan = &self.plans[i];
        Roles {
            walls: plan.walls[layer % plan.walls.len()],
            inside: plan.inside[layer % plan.inside.len()],
        }
    }
    /// The repeating `(walls, inside)` sequences of a mix (one cycle; a gradient's is its
    /// start-ratio cycle).
    pub fn cycle(&self, id: usize) -> Option<(&[usize], &[usize])> {
        let i = self.extruders.iter().position(|ve| ve.id == id)?;
        Some((&self.plans[i].walls, &self.plans[i].inside))
    }
    pub fn expand(&self, id: usize) -> Vec<usize> {
        self.extruders.iter().find(|ve| ve.id == id).map_or_else(
            || vec![id],
            |ve| ve.components.iter().map(|c| c.extruder).collect(),
        )
    }
    /// Painted mixes resolved before the region booleans (exactly as upstream): Layers mixes.
    /// Mica's other patterns stay virtual regions and resolve per role in [`Self::remap_config`].
    fn remaps_paint(&self, id: usize) -> bool {
        self.get(id).is_some_and(|ve| ve.pattern == Pattern::Layers)
    }
    /// The mix a logical region prints with (upstream: by `perimeter_extruder`).
    pub(crate) fn source(&self, logical: &Config) -> Option<usize> {
        logical
            .int("perimeter_extruder")
            .filter(|id| *id > 0)
            .map(|id| id as usize)
            .filter(|id| self.contains(*id))
    }
    /// Each gradient mix's span of object-local layers, from the layers its regions use.
    pub(crate) fn spans(
        &self,
        used: impl IntoIterator<Item = (usize, usize)>,
    ) -> BTreeMap<usize, Range<usize>> {
        let mut out: BTreeMap<usize, Range<usize>> = BTreeMap::new();
        for (id, layer) in used {
            if self
                .get(id)
                .is_some_and(|ve| ve.pattern == Pattern::Gradient)
            {
                out.entry(id)
                    .and_modify(|r| {
                        r.start = r.start.min(layer);
                        r.end = r.end.max(layer + 1);
                    })
                    .or_insert(layer..layer + 1);
            }
        }
        out
    }
    pub(crate) fn remap_config(
        &self,
        logical: &Config,
        layer: usize,
        spans: Option<&BTreeMap<usize, Range<usize>>>,
    ) -> (Config, Option<usize>) {
        let mut config = logical.clone();
        // Upstream identifies whole virtual regions by perimeter_extruder and assigns all
        // three model roles to the same physical tool, even if the other roles were explicit.
        let source = self.source(logical);
        if let Some(id) = source {
            let roles = self.roles(id, layer, spans.and_then(|s| s.get(&id)));
            for (key, tool) in [
                ("perimeter_extruder", roles.walls),
                ("infill_extruder", roles.inside),
                ("solid_infill_extruder", roles.inside),
            ] {
                config
                    .set_raw(key, &tool.to_string())
                    .expect("integer tool");
            }
        }
        (config, source)
    }
    pub(crate) fn remap_paint(&self, layers: &mut [crate::painting::segmentation::LayerColors]) {
        for (i, layer) in layers.iter_mut().enumerate() {
            for ve in &self.extruders {
                if !self.remaps_paint(ve.id) {
                    continue;
                }
                let target = self.resolve(ve.id, i);
                let polygons = std::mem::take(&mut layer[ve.id]);
                if !polygons.is_empty() {
                    layer[target].extend(polygons);
                }
            }
        }
    }

    /// The tool numbers after removing mix `id`: later mixes move down by one so the numbers
    /// stay consecutive after the physical tools (as PrusaSlicer numbers them), and the removed
    /// mix's own parts and painting go to its main colour. Returns the new mixes and the
    /// renumbering of every affected one-based tool.
    pub fn without(
        &self,
        id: usize,
        physical_count: usize,
    ) -> Result<(Self, BTreeMap<usize, usize>), String> {
        let main = self
            .get(id)
            .ok_or_else(|| format!("no mixed color {id}"))?
            .main_extruder();
        let mut map = BTreeMap::from([(id, main)]);
        let mut kept = vec![];
        for ve in &self.extruders {
            if ve.id == id {
                continue;
            }
            let mut ve = ve.clone();
            if ve.id > id {
                map.insert(ve.id, ve.id - 1);
                ve.id -= 1;
            }
            kept.push(ve);
        }
        Ok((Self::new(kept, physical_count)?, map))
    }

    /// Mixes `a` and `b` trade numbers (moving a mix up or down the list).
    pub fn swapped(
        &self,
        a: usize,
        b: usize,
        physical_count: usize,
    ) -> Result<(Self, BTreeMap<usize, usize>), String> {
        if !self.contains(a) || !self.contains(b) {
            return Err(format!("no mixed color {a} or {b}"));
        }
        let mut list: Vec<VirtualExtruder> = self.extruders.clone();
        for ve in &mut list {
            if ve.id == a {
                ve.id = b;
            } else if ve.id == b {
                ve.id = a;
            }
        }
        list.sort_by_key(|ve| ve.id);
        Ok((
            Self::new(list, physical_count)?,
            BTreeMap::from([(a, b), (b, a)]),
        ))
    }
}
