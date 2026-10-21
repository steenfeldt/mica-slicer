// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/CustomGCode.hpp :: CustomGCode::Type, CustomGCode::Item, CustomGCode::Mode, CustomGCode::Info
// Source: prusaslicer@b028299c77 src/libslic3r/CustomGCode.cpp :: check_mode_for_custom_gcode_per_print_z, custom_tool_changes, custom_color_changes
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::assign_custom_gcodes, ToolOrdering::collect_extruders
// Source: orca@8500fcdcca src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::assign_custom_gcodes
// Changes: Rust types with one-based extruders as upstream; the layer assignment returns item indices instead of
// Changes: pointers and takes compact (print_z, tools) layers; Orca's midpoint rule is a dialect switch.

// Copyright (c) Prusa Research 2020 - 2022 Oleksandra Iushchenko, Lukáš Matěna, Vojtěch Bubník, Enrico Turri.
// Upstream is released under the AGPLv3 or higher; this port is AGPL-3.0-only.

//! Per-layer custom G-code: colour changes, pauses, custom and template G-code and per-layer
//! tool switches (PrusaSlicer's `CustomGCode::Info`, Orca's per-plate `custom_gcode_per_layer`).
//! A plate stores one [`Info`]; the exporter asks [`assign`] which item lands on which layer.

use crate::print::LayerSource;

/// libslic3r's `EPSILON`, used by the tool ordering comparisons.
pub const EPSILON: f64 = 1e-4;

/// Upstream `CustomGCode::epsilon()`: the permissible print_z error (G-code Z has 3 decimals).
pub const Z_EPSILON: f64 = 0.0011;

/// Colours PrusaSlicer assigns to colour changes converted from old projects (`ColorPrintColors`).
pub const COLOR_PRINT_COLORS: [&str; 7] = [
    "#C0392B", "#E67E22", "#F1C40F", "#27AE60", "#1ABC9C", "#2980B9", "#9B59B6",
];

/// Upstream `CustomGCode::Type`; the discriminants are the 3MF `type` attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// `M600` (`color_change_gcode`) for the item's extruder.
    ColorChange,
    /// `pause_print_gcode` with an optional message.
    PausePrint,
    /// Switch every object to another extruder from this layer on (single-extruder plates on
    /// multi-tool printers); a colour change on a single-extruder printer.
    ToolChange,
    /// `template_custom_gcode`.
    Template,
    /// The item's own G-code text.
    Custom,
}

impl Kind {
    pub fn index(self) -> i32 {
        match self {
            Self::ColorChange => 0,
            Self::PausePrint => 1,
            Self::ToolChange => 2,
            Self::Template => 3,
            Self::Custom => 4,
        }
    }
    pub fn from_index(i: i64) -> Option<Self> {
        Some(match i {
            0 => Self::ColorChange,
            1 => Self::PausePrint,
            2 => Self::ToolChange,
            3 => Self::Template,
            4 => Self::Custom,
            _ => return None,
        })
    }
    /// Pre-2.2 projects stored the G-code instead of the type.
    pub fn from_legacy_gcode(gcode: &str) -> Self {
        match gcode {
            "M600" => Self::ColorChange,
            "M601" => Self::PausePrint,
            "tool_change" => Self::ToolChange,
            _ => Self::Custom,
        }
    }
}

/// Upstream `CustomGCode::Item`.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub print_z: f64,
    pub kind: Kind,
    /// One based. Colour change: the extruder whose filament is changed; tool change: the
    /// extruder used from here on.
    pub extruder: usize,
    /// `#RRGGBB` for colour and tool changes (preview colour); empty otherwise.
    pub color: String,
    /// The pause message (`PausePrint`) or the G-code text (`Custom`).
    pub extra: String,
}

impl Item {
    pub fn new(print_z: f64, kind: Kind, extruder: usize) -> Self {
        Self {
            print_z,
            kind,
            extruder,
            color: String::new(),
            extra: String::new(),
        }
    }
    /// Short plain-language label, for tooltips and the CLI.
    pub fn describe(&self) -> String {
        match self.kind {
            Kind::ColorChange => format!("Color change (tool {})", self.extruder.max(1)),
            Kind::ToolChange => format!("Change to tool {}", self.extruder.max(1)),
            Kind::PausePrint if self.extra.is_empty() => "Pause".into(),
            Kind::PausePrint => format!("Pause: {}", self.extra),
            Kind::Template => "Template G-code".into(),
            Kind::Custom => {
                let first = self.extra.lines().next().unwrap_or("").trim();
                if self.extra.lines().count() > 1 {
                    format!("Custom G-code: {first} …")
                } else {
                    format!("Custom G-code: {first}")
                }
            }
        }
    }
}

/// Upstream `CustomGCode::Mode`: how the items were entered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Undef,
    /// Single-extruder printer.
    SingleExtruder,
    /// Multi-tool printer, every object on one tool: tool changes switch the whole plate.
    MultiAsSingle,
    /// Multi-tool printer, objects on several tools: colour changes are per tool.
    MultiExtruder,
}

impl Mode {
    /// The 3MF `mode` value.
    pub fn name(self) -> &'static str {
        match self {
            Self::SingleExtruder => "SingleExtruder",
            Self::MultiAsSingle => "MultiAsSingle",
            Self::Undef | Self::MultiExtruder => "MultiExtruder",
        }
    }
    /// Upstream reads anything that is not one of the two other names as `MultiExtruder`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "SingleExtruder" => Self::SingleExtruder,
            "MultiAsSingle" => Self::MultiAsSingle,
            _ => Self::MultiExtruder,
        }
    }
    /// The mode of a print (upstream `assign_custom_gcodes`): `tools` is the printer's extruder
    /// (Orca: filament) count, `object_tools` how many distinct extruders the objects use.
    pub fn of_print(tools: usize, object_tools: usize) -> Self {
        if tools <= 1 {
            Self::SingleExtruder
        } else if object_tools == 1 {
            Self::MultiAsSingle
        } else {
            Self::MultiExtruder
        }
    }
}

/// Upstream `CustomGCode::Info`: the items of one plate, sorted by `print_z`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub mode: Mode,
    pub items: Vec<Item>,
}

impl Info {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Sort by height (stable, as upstream's `std::set<TickCode>` keeps one item per height).
    pub fn sort(&mut self) {
        self.items.sort_by(|a, b| a.print_z.total_cmp(&b.print_z));
    }
    /// The item at `print_z` (within upstream's Z epsilon), if any.
    pub fn at(&self, print_z: f64) -> Option<usize> {
        self.items
            .iter()
            .position(|i| (i.print_z - print_z).abs() < Z_EPSILON)
    }
    /// Insert or replace the item at its height, keeping the list sorted. Returns its index.
    pub fn put(&mut self, item: Item) -> usize {
        if let Some(i) = self.at(item.print_z) {
            self.items[i] = item;
            return i;
        }
        let i = self.items.partition_point(|x| x.print_z < item.print_z);
        self.items.insert(i, item);
        i
    }

    /// Upstream `check_mode_for_custom_gcode_per_print_z`: projects from older slicers have no
    /// mode; derive it from the items.
    pub fn check_mode(&mut self) {
        if self.mode != Mode::Undef {
            return;
        }
        let mut single = true;
        for item in &self.items {
            if item.kind == Kind::ToolChange {
                self.mode = Mode::MultiAsSingle;
                return;
            }
            if item.kind == Kind::ColorChange && item.extruder > 1 {
                single = false;
            }
        }
        self.mode = if single {
            Mode::SingleExtruder
        } else {
            Mode::MultiExtruder
        };
    }

    /// Upstream `custom_tool_changes`: (print_z, one-based extruder) of every tool change; an
    /// extruder beyond the printer's count falls back to the first.
    pub fn tool_changes(&self, tools: usize) -> Vec<(f64, usize)> {
        self.items
            .iter()
            .filter(|i| i.kind == Kind::ToolChange)
            .map(|i| (i.print_z, if i.extruder > tools { 1 } else { i.extruder }))
            .collect()
    }

    /// Upstream `custom_color_changes`: (print_z, one-based extruder) of every colour change
    /// for an extruder the printer has.
    pub fn color_changes(&self, tools: usize) -> Vec<(f64, usize)> {
        self.items
            .iter()
            .filter(|i| i.kind == Kind::ColorChange && i.extruder <= tools)
            .map(|i| (i.print_z, i.extruder))
            .collect()
    }

    /// Per-layer extruder overrides of a `MultiAsSingle` plate (upstream `ToolOrdering`'s
    /// `per_layer_extruder_switches`), empty unless the print and the items agree on that mode.
    pub fn extruder_switches(&self, tools: usize, object_tools: usize) -> Switches {
        Switches(
            if tools > 1 && object_tools == 1 && self.mode == Mode::MultiAsSingle {
                self.tool_changes(tools)
            } else {
                Vec::new()
            },
        )
    }

    /// Colour changes for which the tool changer must pick the extruder first (upstream
    /// `per_layer_color_changes`: multi-tool printers without single-extruder multi-material).
    pub fn picked_color_changes(&self, tools: usize, semm: bool) -> Vec<(f64, usize)> {
        if tools > 1 && self.mode == Mode::MultiExtruder && !semm {
            self.color_changes(tools)
        } else {
            Vec::new()
        }
    }

    /// Upstream `collect_extruders`: the zero-based extruder that must print first on a layer at
    /// `print_z` for a colour change there (exact layer match only).
    pub fn needed_extruder(changes: &[(f64, usize)], print_z: f64) -> Option<usize> {
        changes
            .iter()
            .filter(|(z, _)| *z < print_z + EPSILON && (z - print_z).abs() < EPSILON)
            .map(|&(_, e)| e.saturating_sub(1))
            .next()
    }
}

/// Per-layer extruder overrides: from each `(print_z, extruder)` on, the objects print with that
/// one-based extruder (upstream `collect_extruders`' `extruder_override`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Switches(pub Vec<(f64, usize)>);

impl Switches {
    /// The zero-based override for an object layer at `print_z`, if any.
    pub fn at(&self, print_z: f64) -> Option<usize> {
        self.0
            .iter()
            .take_while(|(z, _)| *z < print_z + EPSILON)
            .last()
            .map(|&(_, e)| e.saturating_sub(1))
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Which slicer's layer rule to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// An item lands on the lowest layer whose print_z reaches it (within half an epsilon).
    Prusa,
    /// Orca: an item lands on the layer whose print_z is nearest, split at the midpoints.
    Orca,
}

/// Upstream `ToolOrdering::assign_custom_gcodes`: the item (index into `info.items`) emitted on
/// each layer. `layers` are (print_z, zero-based extruders printing it) from bottom to top,
/// `tools` the printer's extruder (Orca: filament) count, `object_tools` the number of distinct
/// extruders the objects use. Tool changes are never assigned (they act through
/// [`Info::extruder_switches`]) unless they stand in for colour changes on a single-extruder
/// printer. At most one item per layer; items between layers move up to the next layer.
pub fn assign(
    info: &Info,
    layers: &[(f64, Vec<usize>)],
    tools: usize,
    object_tools: usize,
    dialect: Dialect,
) -> Vec<Option<usize>> {
    let mut out = vec![None; layers.len()];
    if info.items.is_empty() {
        return out;
    }
    let mode = Mode::of_print(tools, object_tools);
    let model_mode = info.mode;
    let mut printing_above = vec![false; tools.max(1)];
    // Tool and colour changes entered for a multi-material plate do not apply to a
    // single-material one, and the other way round.
    let ignore_tool_and_color_changes =
        (mode == Mode::MultiExtruder) != (model_mode == Mode::MultiExtruder);
    // On a single-extruder machine, tool changes trigger colour change (M600) events.
    let tool_changes_as_color_changes =
        mode == Mode::SingleExtruder && model_mode == Mode::MultiAsSingle;
    let mut it = info.items.len();
    let mut print_z_above = f64::MIN;
    for li in (0..layers.len()).rev() {
        let (print_z, extruders) = (&layers[li].0, &layers[li].1);
        let print_z = *print_z;
        for &e in extruders {
            if let Some(flag) = printing_above.get_mut(e) {
                *flag = true;
            }
        }
        // Skip all items above this layer and all extruder switches.
        while it > 0 {
            let item = &info.items[it - 1];
            let above = match dialect {
                Dialect::Prusa => item.print_z > print_z + EPSILON,
                Dialect::Orca => {
                    print_z_above > print_z && item.print_z > 0.5 * (print_z + print_z_above)
                }
            };
            if above || item.kind == Kind::ToolChange {
                it -= 1;
            } else {
                break;
            }
        }
        print_z_above = print_z;
        if it == 0 {
            break;
        }
        let index = it - 1;
        let item = &info.items[index];
        let print_z_below = if li > 0 { layers[li - 1].0 } else { 0. };
        let applies = match dialect {
            Dialect::Prusa => item.print_z > print_z_below + 0.5 * EPSILON,
            Dialect::Orca => item.print_z > 0.5 * (print_z_below + print_z),
        };
        if applies {
            let color_change = item.kind == Kind::ColorChange;
            let tool_change = item.kind == Kind::ToolChange;
            let pause_or_custom = !color_change && !tool_change;
            let apply_color_change = !ignore_tool_and_color_changes
                && if color_change {
                    mode == Mode::SingleExtruder
                        || (item.extruder >= 1
                            && item.extruder <= tools
                            && printing_above[item.extruder - 1])
                } else {
                    tool_change && tool_changes_as_color_changes
                };
            if pause_or_custom || apply_color_change {
                out[li] = Some(index);
            }
            // Consume the item.
            it -= 1;
        }
    }
    out
}

/// A layer source with a plate's custom G-code attached, for sources that cannot carry it
/// themselves (a single prepared object). Every other call goes to the wrapped source.
pub struct WithCustomGCode<'a> {
    pub source: &'a dyn LayerSource,
    pub info: &'a Info,
}

impl LayerSource for WithCustomGCode<'_> {
    fn custom_gcode(&self) -> Option<&Info> {
        Some(self.info)
    }
    fn calibration_control(&self, layer: usize, path: usize) -> Option<crate::calib::Control> {
        self.source.calibration_control(layer, path)
    }
    fn is_cancelled(&self) -> bool {
        self.source.is_cancelled()
    }
    fn emitted(&self, layer: usize, paths: &[crate::print::PathEmission], layer_time_s: f64) {
        self.source.emitted(layer, paths, layer_time_s)
    }
    fn print_job(&self) -> Option<&crate::print_job::PreparedPrint> {
        self.source.print_job()
    }
    fn scheduled_toolpaths(
        &self,
        range: std::ops::Range<usize>,
    ) -> Vec<crate::print_job::ScheduledLayer> {
        self.source.scheduled_toolpaths(range)
    }
    fn first_layer_hull(&self) -> Option<&[crate::geometry::Point]> {
        self.source.first_layer_hull()
    }
    fn layer_outlines(&self, index: usize) -> Option<&[crate::geometry::ExPolygon]> {
        self.source.layer_outlines(index)
    }
    fn support_islands(&self, index: usize) -> Option<Vec<crate::geometry::ExPolygon>> {
        self.source.support_islands(index)
    }
    fn region_slices(&self, index: usize) -> Option<Vec<Vec<crate::geometry::ExPolygon>>> {
        self.source.region_slices(index)
    }
    fn layer_count(&self) -> usize {
        self.source.layer_count()
    }
    fn layer_z(&self, i: usize) -> (f64, f64) {
        self.source.layer_z(i)
    }
    fn first_layer_bbox(&self) -> Option<crate::geometry::BoundingBox> {
        self.source.first_layer_bbox()
    }
    fn toolpaths(&self, range: std::ops::Range<usize>) -> Vec<crate::toolpath::LayerToolpaths> {
        self.source.toolpaths(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layers(n: usize, tools: Vec<usize>) -> Vec<(f64, Vec<usize>)> {
        (0..n)
            .map(|i| (0.2 * (i + 1) as f64, tools.clone()))
            .collect()
    }

    #[test]
    fn items_land_on_the_layer_they_name_or_the_next_one_up() {
        let mut info = Info {
            mode: Mode::SingleExtruder,
            ..Info::default()
        };
        info.put(Item::new(5.0, Kind::ColorChange, 1));
        info.put(Item::new(10.1, Kind::PausePrint, 1));
        let l = layers(100, vec![0]);
        let a = assign(&info, &l, 1, 1, Dialect::Prusa);
        let hits: Vec<_> = a
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.map(|x| (i, x)))
            .collect();
        // Z 5.0 is layer 25 (index 24); Z 10.1 sits between 10.0 and 10.2: the layer above.
        assert_eq!(hits, vec![(24, 0), (50, 1)]);
        // Orca splits at the midpoints between layers.
        let o = assign(&info, &l, 1, 1, Dialect::Orca);
        assert_eq!(o[24], Some(0));
        assert_eq!(o.iter().flatten().count(), 2);
    }

    #[test]
    fn tool_changes_switch_objects_and_colour_changes_need_their_tool() {
        let mut info = Info::default();
        info.put(Item::new(2.0, Kind::ToolChange, 2));
        info.check_mode();
        assert_eq!(info.mode, Mode::MultiAsSingle);
        let s = info.extruder_switches(5, 1);
        assert_eq!(s.at(1.8), None);
        assert_eq!(s.at(2.0), Some(1));
        assert_eq!(s.at(7.0), Some(1));
        // Tool changes are never layer items: upstream's skip loop passes over them before
        // its "tool change as colour change" branch could see one (dead code at 2.9.6).
        for tools in [5, 1] {
            assert!(
                assign(&info, &layers(20, vec![0]), tools, 1, Dialect::Prusa)
                    .iter()
                    .all(Option::is_none)
            );
        }
        let mut multi = Info {
            mode: Mode::MultiExtruder,
            ..Info::default()
        };
        multi.put(Item::new(1.0, Kind::ColorChange, 2));
        multi.put(Item::new(3.0, Kind::ColorChange, 3));
        // Tool 3 never prints, so its colour change is dropped.
        let a = assign(&multi, &layers(20, vec![0, 1]), 5, 2, Dialect::Prusa);
        assert_eq!(a.iter().flatten().count(), 1);
        assert_eq!(
            Info::needed_extruder(&multi.picked_color_changes(5, false), 1.0),
            Some(1)
        );
        assert_eq!(
            Info::needed_extruder(&multi.picked_color_changes(5, false), 1.1),
            None
        );
    }

    #[test]
    fn put_replaces_the_item_at_a_height_and_keeps_order() {
        let mut info = Info::default();
        info.put(Item::new(3.0, Kind::PausePrint, 1));
        info.put(Item::new(1.0, Kind::ColorChange, 1));
        info.put(Item::new(3.0005, Kind::Template, 1));
        assert_eq!(info.items.len(), 2);
        assert_eq!(info.items[0].print_z, 1.0);
        assert_eq!(info.items[1].kind, Kind::Template);
        assert_eq!(Kind::from_index(4), Some(Kind::Custom));
        assert_eq!(Mode::from_name("whatever"), Mode::MultiExtruder);
    }
}
