// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PresetBundle.cpp :: s_project_options
// Source: orca@8500fcdcca src/slic3r/GUI/PresetComboBoxes.cpp :: PresetComboBox::update_ams_color
// Changes: Only the rule that filament_colour is project state (not a filament preset option) and what a preset switch does to it; the overlay model is original.
// Changes: An opened project's own values are a layer under the edits, not edits themselves.

//! Unsaved edits on top of the selected presets: the settings editor writes here, the
//! slicer reads the composed result. Values are PrusaSlicer text, like user profiles.
//!
//! Filament colours are not filament edits: as in OrcaSlicer (`filament_colour` is one of
//! `PresetBundle`'s project options, and not a filament preset option), the colour of each
//! tool belongs to the project. It lives in `Overlays::colours`, is saved with the project,
//! never shows a changed marker, never counts as a change of the filament profile and is never
//! written into one. Edits of `filament_colour` that arrive through a filament overlay (the
//! settings editor, a colour picker) are moved there by [`Overlays::normalize`].
//!
//! An opened project's own values (`Overlays::project`, its embedded configuration where it
//! differs from the presets it names) lie between the presets and the edits: they are what the
//! project slices with, but they are the project's, so they never show a changed marker and
//! never count as edits. Choosing another preset drops the project's values of its scope.

use mica_core::config::{Config, ConfigValue, serialize_value};
use mica_profiles::project::Layer;
use std::collections::BTreeMap;
use std::sync::Arc;

pub type Overlay = BTreeMap<String, String>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlays {
    pub print: Overlay,
    pub printer: Overlay,
    /// One overlay per tool.
    pub filaments: Vec<Overlay>,
    /// The project's colour of each tool (`#RRGGBB`), `None` for the filament preset's own.
    pub colours: Vec<Option<String>>,
    /// The opened project's own values, under the edits (shared: undo steps keep copies).
    pub project: Arc<Layer>,
}

/// The option that holds the per-tool filament colour.
pub const COLOUR_KEY: &str = "filament_colour";

/// What a settings edit applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Print,
    Printer,
    Filament(usize),
}

impl Overlays {
    pub fn new(tools: usize) -> Self {
        Self {
            print: Overlay::new(),
            printer: Overlay::new(),
            filaments: vec![Overlay::new(); tools.max(1)],
            colours: vec![None; tools.max(1)],
            project: Arc::default(),
        }
    }

    pub fn resize(&mut self, tools: usize) {
        self.filaments.resize(tools.max(1), Overlay::new());
        self.colours.resize(tools.max(1), None);
    }

    /// Sets (or with `None` clears) the project colour of `tool`.
    pub fn set_colour(&mut self, tool: usize, colour: Option<String>) {
        if self.colours.len() <= tool {
            self.colours.resize(tool + 1, None);
        }
        self.colours[tool] = colour;
    }

    /// The project colour of `tool`, if one is set.
    pub fn colour(&self, tool: usize) -> Option<&str> {
        self.colours.get(tool)?.as_deref()
    }

    /// Moves `filament_colour` edits out of the filament overlays into the project colours
    /// (the place they belong); returns whether anything moved.
    pub fn normalize(&mut self) -> bool {
        let mut moved = false;
        for tool in 0..self.filaments.len() {
            if let Some(c) = self.filaments[tool].remove(COLOUR_KEY) {
                // A vector value (`#AA0000;#00BB00`) edits this tool's element.
                let c = c.split(';').next().unwrap_or_default().trim().to_string();
                self.set_colour(tool, (!c.is_empty()).then_some(c));
                moved = true;
            }
        }
        moved
    }

    /// Another filament preset was chosen for `tool`: its edits go (they belonged to the old
    /// preset). The colour is the project's and stays, unless the new preset has a colour of
    /// its own, which it then shows (Orca's `update_ams_color` takes the new preset's
    /// `default_filament_colour` and keeps the project colour when that is empty).
    pub fn filament_switched(&mut self, tool: usize, preset_has_colour: bool) {
        if let Some(o) = self.filaments.get_mut(tool) {
            o.clear();
        }
        if self
            .project
            .filaments
            .get(tool)
            .is_some_and(|f| *f != Config::new())
        {
            Arc::make_mut(&mut self.project).drop_tool(tool);
        }
        if preset_has_colour {
            self.set_colour(tool, None);
        }
    }

    /// Another process preset was chosen: its edits and the project's process values go.
    pub fn process_switched(&mut self) {
        self.print.clear();
        if self.project.process != Config::new() {
            Arc::make_mut(&mut self.project).drop_process();
        }
    }

    /// Another printer was chosen: its edits and all of the project's values go (they were
    /// made for the project's printer).
    pub fn printer_switched(&mut self) {
        self.printer.clear();
        self.project = Arc::default();
    }

    /// The same overlays apart from the colours (whether only colours differ).
    pub fn same_but_colours(&self, other: &Self) -> bool {
        self.print == other.print
            && self.printer == other.printer
            && self.filaments == other.filaments
            && self.project == other.project
    }

    pub fn get(&self, target: Target) -> &Overlay {
        match target {
            Target::Print => &self.print,
            Target::Printer => &self.printer,
            Target::Filament(i) => &self.filaments[i.min(self.filaments.len() - 1)],
        }
    }

    pub fn get_mut(&mut self, target: Target) -> &mut Overlay {
        let last = self.filaments.len() - 1;
        match target {
            Target::Print => &mut self.print,
            Target::Printer => &mut self.printer,
            Target::Filament(i) => &mut self.filaments[i.min(last)],
        }
    }

    /// Applies every overlay to `config`; returns the keys that failed to parse.
    pub fn apply(&self, config: &mut Config) -> Vec<String> {
        self.project.apply(config);
        let mut bad = Vec::new();
        for (k, v) in self.print.iter().chain(&self.printer) {
            if config.set_raw(k, v).is_err() {
                bad.push(k.clone());
            }
        }
        for (tool, overlay) in self.filaments.iter().enumerate() {
            for (k, v) in overlay {
                if set_tool_value(config, k, v, tool).is_err() {
                    bad.push(k.clone());
                }
            }
        }
        for (tool, colour) in self.colours.iter().enumerate() {
            if let Some(c) = colour
                && set_tool_value(config, COLOUR_KEY, c, tool).is_err()
            {
                bad.push(COLOUR_KEY.into());
            }
        }
        bad
    }
}

pub use mica_profiles::project::set_tool_value;

/// The element of printer option `key` the printer editor shows and edits while extruder
/// `tool` is selected: that extruder's for the per-extruder options (the Extruder page),
/// the first (normal mode of the machine limits) for other vectors, and `None` (the whole
/// value) for point lists. A bed shape is one polygon, not one point per extruder (GUI
/// monkey test, seed 3004: a bed shape typed with extruder 2 selected became
/// `0x0,0x0,360x360,0x360`).
pub fn printer_element(key: &str, tool: usize) -> Option<usize> {
    let ty = mica_core::config::option_defs().get(key).map(|d| d.ty);
    if crate::settings::is_per_extruder(key) {
        Some(tool)
    } else if ty == Some(mica_core::config::OptType::Points) {
        None
    } else {
        Some(0)
    }
}

/// The bed's corners (OrcaSlicer's `printable_area`).
pub const BED_SHAPE_KEY: &str = "bed_shape";

/// Why the bed shape `raw` cannot be a bed (`None` when it can; see
/// `mica_core::safety::bed_shape_problem`).
pub fn bed_shape_problem(raw: &str) -> Option<String> {
    let mut c = Config::new();
    if c.set_raw(BED_SHAPE_KEY, raw).is_err() {
        return Some(
            "write the corners as XxY, separated by commas (0x0,250x0,250x210,0x210)".into(),
        );
    }
    mica_core::safety::bed_shape_problem(&c.points(BED_SHAPE_KEY).unwrap_or_default())
}

/// Text value of `key` as one tool sees it (first element for scalars).
pub fn tool_raw(config: &Config, key: &str, tool: usize) -> Option<String> {
    match config.get(key)? {
        ConfigValue::Vector(items) => {
            let item = items.get(tool).or_else(|| items.first())?;
            Some(serialize_value(&ConfigValue::Vector(vec![item.clone()])))
        }
        scalar => Some(serialize_value(scalar)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlays_apply_per_tool_and_globally() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4,0.4").unwrap();
        c.set_raw("temperature", "210,210,210").unwrap();
        let mut o = Overlays::new(3);
        o.print.insert("perimeters".into(), "4".into());
        o.filaments[1].insert("temperature".into(), "245".into());
        o.filaments[2].insert("filament_type".into(), "PETG".into());
        o.printer
            .insert("retract_length".into(), "0.9,1.2,1.5".into());
        assert!(o.apply(&mut c).is_empty());
        assert_eq!(c.int("perimeters"), Some(4));
        assert_eq!(c.float_at("temperature", 0), Some(210.));
        assert_eq!(c.float_at("temperature", 1), Some(245.));
        assert_eq!(c.string_at("filament_type", 2), Some("PETG"));
        assert_eq!(c.float_at("retract_length", 2), Some(1.5));
        assert_eq!(tool_raw(&c, "temperature", 1).as_deref(), Some("245"));
        assert_eq!(tool_raw(&c, "perimeters", 7).as_deref(), Some("4"));
        // Colours are project state, applied per tool like a filament edit.
        o.set_colour(2, Some("#123456".into()));
        assert!(o.apply(&mut c).is_empty());
        assert_eq!(c.string_at("filament_colour", 2), Some("#123456"));
        let mut bad = Overlays::new(1);
        bad.print.insert("perimeters".into(), "many".into());
        assert_eq!(bad.apply(&mut c), ["perimeters"]);
    }

    #[test]
    fn colour_edits_move_out_of_the_filament_overlay() {
        let mut o = Overlays::new(2);
        o.filaments[1].insert("filament_colour".into(), "#00FF00".into());
        o.filaments[1].insert("temperature".into(), "215".into());
        let before = o.clone();
        assert!(o.normalize());
        assert!(!o.filaments[1].contains_key("filament_colour"));
        assert_eq!(o.colour(1), Some("#00FF00"));
        assert_eq!(o.colour(0), None);
        assert!(!o.normalize());
        assert!(!o.same_but_colours(&before));
        let mut p = o.clone();
        p.set_colour(0, Some("#FFFFFF".into()));
        assert!(p.same_but_colours(&o) && p != o);
        // A preset switch drops the tool's edits; the colour stays unless the preset has one.
        p.filament_switched(1, false);
        assert!(p.filaments[1].is_empty());
        assert_eq!(p.colour(1), Some("#00FF00"));
        p.filament_switched(1, true);
        assert_eq!(p.colour(1), None);
    }

    #[test]
    fn project_values_lie_under_the_edits_and_follow_preset_switches() {
        let mut base = Config::defaults();
        base.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        base.set_raw("temperature", "210,210").unwrap();
        let mut source = Config::new();
        source.set_raw("perimeters", "5").unwrap();
        source.set_raw("fill_density", "8%").unwrap();
        source.set_raw("temperature", "220,230").unwrap();
        source.set_raw("retract_length", "1.5,1.5").unwrap();
        let mut o = Overlays::new(2);
        o.project = Arc::new(Layer::between(&source, &base, 2, None, |_, _, _| true));
        o.print.insert("perimeters".into(), "3".into());
        let mut c = base.clone();
        assert!(o.apply(&mut c).is_empty());
        // The edit wins over the project's value; the project's other values apply.
        assert_eq!(c.int("perimeters"), Some(3));
        assert_eq!(c.float("fill_density"), Some(8.));
        assert_eq!(c.raw("temperature").as_deref(), Some("220,230"));
        // Not edits: no changed markers, nothing for "Discard changes" to discard.
        assert_eq!(o.print.len(), 1);
        assert!(o.filaments.iter().all(Overlay::is_empty));
        let before = o.clone();
        o.filament_switched(1, false);
        assert_ne!(o, before);
        let mut c = base.clone();
        o.apply(&mut c);
        assert_eq!(c.raw("temperature").as_deref(), Some("220,210"));
        o.process_switched();
        let mut c = base.clone();
        o.apply(&mut c);
        assert_eq!(c.float("fill_density"), base.float("fill_density"));
        assert_eq!(c.float_at("retract_length", 1), Some(1.5));
        assert!(!o.project.is_empty());
        o.printer_switched();
        assert!(o.project.is_empty());
    }
}
