// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! "Colour print" preview: layers coloured by the colour segments a plate's colour and tool
//! changes create (as PrusaSlicer's ColorPrint view), plus the legend rows for them.
//!
//! The shader reads one value per layer: for each of the first six tools a 4-bit palette index
//! (0 = the tool's own filament colour, `tools + k` = the colour of the k-th colour change).
//! Values stay below 2^24, so they travel exactly in the layer value buffer's `f32`s.

use mica_core::config::Config;
use mica_core::custom_gcode::{self, Dialect, Info, Kind, Mode, Z_EPSILON};

/// Tools the packed per-layer value can describe.
pub const PACKED_TOOLS: usize = 6;
/// Palette slots for colours (the shader's 16-entry tool array).
pub const PALETTE: usize = 16;

/// Orca's layer rule under OrcaSlicer rules (the U1), PrusaSlicer's otherwise (as the exporter).
pub fn dialect(c: &Config) -> Dialect {
    if mica_core::rules::Rules::of(c).is_orca() {
        Dialect::Orca
    } else {
        Dialect::Prusa
    }
}

/// `[r, g, b]` (0..1) → `#RRGGBB`.
pub fn to_hex(c: [f32; 3]) -> String {
    let b = c.map(|v| (v.clamp(0., 1.) * 255.).round() as u8);
    format!("#{:02X}{:02X}{:02X}", b[0], b[1], b[2])
}

/// The layer an item at `z` is shown on: the lowest layer reaching it (the last layer when
/// it is above the print).
pub fn layer_for_z(zs: &[f64], z: f64) -> usize {
    zs.iter()
        .position(|&lz| lz >= z - Z_EPSILON)
        .unwrap_or(zs.len().saturating_sub(1))
}

/// Which item lands on each layer, as the exporter decides it (every tool counts as printing).
pub fn applied(
    info: &Info,
    zs: &[f64],
    tools: usize,
    object_tools: usize,
    dialect: Dialect,
) -> Vec<Option<usize>> {
    let all: Vec<usize> = (0..tools.max(1)).collect();
    let layers: Vec<(f64, Vec<usize>)> = zs.iter().map(|&z| (z, all.clone())).collect();
    custom_gcode::assign(info, &layers, tools, object_tools, dialect)
}

/// A legend swatch: a tool's own colour or a colour change's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Swatch {
    Tool(usize),
    Change(usize),
}

/// One legend row: a colour segment from `first` to `last` layer (inclusive).
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub swatch: Swatch,
    pub first: usize,
    pub last: usize,
    /// The tool the segment concerns when colour changes are per tool (several tools).
    pub tool: Option<usize>,
}

/// A pause or custom G-code at a layer (legend entry).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub layer: usize,
    pub kind: Kind,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorPrint {
    /// Palette offset: palette index `tools + k` is `colors[k]`.
    pub tools: usize,
    pub colors: Vec<[f32; 3]>,
    /// One packed value per preview layer.
    pub packed: Vec<f32>,
    pub segments: Vec<Segment>,
    pub events: Vec<Event>,
}

impl ColorPrint {
    /// `None` when the plate has no items.
    pub fn build(
        info: &Info,
        zs: &[f64],
        tools: usize,
        object_tools: usize,
        dialect: Dialect,
    ) -> Option<Self> {
        if info.items.is_empty() || zs.is_empty() {
            return None;
        }
        let tools = tools.max(1);
        let applied = applied(info, zs, tools, object_tools, dialect);
        let mut out = Self {
            tools,
            ..Self::default()
        };
        let mut generation = [0u32; PACKED_TOOLS];
        let multi_extruder =
            tools > 1 && Mode::of_print(tools, object_tools) == Mode::MultiExtruder;
        // Whole-plate segments start at colour changes (single tool) and tool switches.
        let mut starts: Vec<(usize, Swatch, Option<usize>)> = vec![(0, Swatch::Tool(0), None)];
        let switches = info.extruder_switches(tools, object_tools);
        if let Some(&(_, first_tool)) = switches.0.first().filter(|(z, _)| *z <= zs[0] + Z_EPSILON)
        {
            starts[0].1 = Swatch::Tool(first_tool.saturating_sub(1));
        }
        for (z, tool) in &switches.0 {
            let layer = layer_for_z(zs, *z);
            starts.push((layer, Swatch::Tool(tool.saturating_sub(1)), None));
        }
        for (li, item) in applied.iter().enumerate() {
            if let Some(i) = *item {
                let item = &info.items[i];
                match item.kind {
                    Kind::ColorChange | Kind::ToolChange => {
                        let tool = if tools == 1 {
                            0
                        } else {
                            item.extruder.saturating_sub(1).min(tools - 1)
                        };
                        let k = out.colors.len();
                        if tools + k < PALETTE {
                            out.colors.push(
                                mica_core::color::parse_hex_f32(&item.color).unwrap_or_else(|| {
                                    mica_core::color::parse_hex_f32(
                                        custom_gcode::COLOR_PRINT_COLORS[k % 7],
                                    )
                                    .unwrap()
                                }),
                            );
                            if tool < PACKED_TOOLS {
                                generation[tool] = (tools + k) as u32;
                            }
                        }
                        let swatch = if tools + k < PALETTE {
                            Swatch::Change(k)
                        } else {
                            Swatch::Tool(tool)
                        };
                        starts.push((li, swatch, multi_extruder.then_some(tool)));
                    }
                    kind => out.events.push(Event {
                        layer: li,
                        kind,
                        label: item.describe(),
                    }),
                }
            }
            let mut packed = 0u32;
            for (t, g) in generation.iter().enumerate() {
                packed |= g << (4 * t);
            }
            out.packed.push(packed as f32);
        }
        starts.sort_by_key(|s| s.0);
        let last_layer = zs.len() - 1;
        if multi_extruder {
            // Per tool: each change starts its own row; rows end at the next change of that tool.
            for (i, &(first, swatch, tool)) in starts.iter().enumerate() {
                let last = starts[i + 1..]
                    .iter()
                    .find(|s| s.2 == tool)
                    .map_or(last_layer, |s| s.0.saturating_sub(1));
                out.segments.push(Segment {
                    swatch,
                    first,
                    last: last.max(first),
                    tool,
                });
            }
        } else {
            for (i, &(first, swatch, tool)) in starts.iter().enumerate() {
                let last = starts
                    .get(i + 1)
                    .map_or(last_layer, |s| s.0.saturating_sub(1));
                if i + 1 < starts.len() && starts[i + 1].0 == first {
                    continue; // replaced on the same layer
                }
                out.segments.push(Segment {
                    swatch,
                    first,
                    last: last.max(first),
                    tool,
                });
            }
        }
        Some(out)
    }

    /// The 16-entry shader palette: tool colours, then the colour changes.
    pub fn palette(&self, tool_colors: &[[f32; 3]]) -> [[f32; 4]; 16] {
        let mut out = [[1.; 4]; 16];
        for (i, slot) in out.iter_mut().enumerate() {
            let c = if i < self.tools {
                tool_colors
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| super::legend::tool_color(i))
            } else if let Some(c) = self.colors.get(i - self.tools) {
                *c
            } else {
                continue;
            };
            *slot = [c[0], c[1], c[2], 1.];
        }
        out
    }

    pub fn swatch_color(&self, s: Swatch, tool_colors: &[[f32; 3]]) -> [f32; 3] {
        match s {
            Swatch::Tool(t) => tool_colors
                .get(t)
                .copied()
                .unwrap_or_else(|| super::legend::tool_color(t)),
            Swatch::Change(k) => self.colors.get(k).copied().unwrap_or([1.; 3]),
        }
    }
}

/// A colour for a new colour change: the next upstream colour-print colour that differs from
/// the colour already printing (`current`, `#RRGGBB`) and from the colours already used.
pub fn next_color(info: &Info, current: &str) -> String {
    let used: Vec<String> = info
        .items
        .iter()
        .map(|i| i.color.to_ascii_uppercase())
        .collect();
    let current = current.to_ascii_uppercase();
    custom_gcode::COLOR_PRINT_COLORS
        .iter()
        .find(|c| **c != current && !used.contains(&c.to_string()))
        .or_else(|| {
            custom_gcode::COLOR_PRINT_COLORS
                .iter()
                .find(|c| **c != current)
        })
        .unwrap()
        .to_string()
}

/// The tool after `current` (zero-based), cycling through `tools`.
pub fn next_tool(current: usize, tools: usize) -> usize {
    (current + 1) % tools.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::custom_gcode::Item;

    fn zs(n: usize) -> Vec<f64> {
        (1..=n).map(|i| 0.2 * i as f64).collect()
    }

    #[test]
    fn colour_changes_split_segments_and_pack_palette_indices() {
        let mut info = Info {
            mode: Mode::SingleExtruder,
            ..Info::default()
        };
        info.put(Item {
            color: "#FF0000".into(),
            ..Item::new(5.0, Kind::ColorChange, 1)
        });
        info.put(Item::new(10.0, Kind::PausePrint, 1));
        let z = zs(100);
        let cp = ColorPrint::build(&info, &z, 1, 1, Dialect::Prusa).unwrap();
        assert_eq!(cp.packed.len(), 100);
        assert_eq!(cp.packed[23], 0.);
        assert_eq!(cp.packed[24], 1.); // palette slot 1 = first colour change
        assert_eq!(cp.packed[99], 1.);
        assert_eq!(cp.colors, vec![[1., 0., 0.]]);
        assert_eq!(cp.segments.len(), 2);
        assert_eq!((cp.segments[0].first, cp.segments[0].last), (0, 23));
        assert_eq!((cp.segments[1].first, cp.segments[1].last), (24, 99));
        assert_eq!(cp.events.len(), 1);
        assert_eq!(cp.events[0].layer, 49);
        let pal = cp.palette(&[[0., 0., 1.]]);
        assert_eq!(pal[0], [0., 0., 1., 1.]);
        assert_eq!(pal[1], [1., 0., 0., 1.]);
    }

    #[test]
    fn per_tool_colour_changes_and_tool_switches() {
        let mut multi = Info {
            mode: Mode::MultiExtruder,
            ..Info::default()
        };
        multi.put(Item::new(1.0, Kind::ColorChange, 2));
        let cp = ColorPrint::build(&multi, &zs(20), 5, 2, Dialect::Prusa).unwrap();
        // Tool 2's nibble switches to palette slot 5 (5 tools, first change).
        assert_eq!(cp.packed[4] as u32, 5 << 4);
        assert_eq!(cp.packed[3], 0.);
        let mut single = Info::default();
        single.put(Item::new(2.0, Kind::ToolChange, 3));
        single.check_mode();
        let cp = ColorPrint::build(&single, &zs(20), 5, 1, Dialect::Prusa).unwrap();
        assert!(cp.packed.iter().all(|v| *v == 0.));
        assert_eq!(cp.segments.len(), 2);
        assert_eq!(cp.segments[1].swatch, Swatch::Tool(2));
        assert_eq!(cp.segments[1].first, 9);
    }

    #[test]
    fn helpers() {
        assert_eq!(
            mica_core::color::parse_hex_f32("#0080ff"),
            Some([0., 128. / 255., 1.])
        );
        assert_eq!(mica_core::color::parse_hex_f32("red"), None);
        assert_eq!(to_hex([1., 0.5, 0.]), "#FF8000");
        assert_eq!(layer_for_z(&zs(10), 1.0), 4);
        assert_eq!(layer_for_z(&zs(10), 1.05), 5);
        assert_eq!(layer_for_z(&zs(10), 50.), 9);
        assert_eq!(next_tool(4, 5), 0);
        assert_eq!(next_tool(0, 1), 0);
        let mut info = Info::default();
        assert_eq!(next_color(&info, "#C0392B"), "#E67E22");
        info.put(Item {
            color: "#E67E22".into(),
            ..Item::new(1., Kind::ColorChange, 1)
        });
        assert_eq!(next_color(&info, "#000000"), "#C0392B");
    }
}
