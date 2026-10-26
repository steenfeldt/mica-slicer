// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica GUI project-settings card model.
// Changes: The project's own values are a layer under the edits; differences to today's presets are listed.

//! A 3MF project's own printer/process/filament settings: matching its preset names to
//! Mica's bundles, deciding whether to apply them without asking, and the state of the
//! "This project has its own settings" card. The project's filament colours apply either
//! way (`colour_overlay`); the rest is applied by `App::use_project_settings`: the project
//! slices with its own saved values (`Overlays::project`), which are not edits.
//! `App::update_project_to_current` adopts today's presets, keeping only what the project
//! lists as changed.

use crate::overlays::Overlays;
use mica_profiles::project::ProjectSettings;

pub use mica_profiles::project::{
    Decision, decide, match_filament, match_name, match_printer, strip_copy,
};

/// Where the card is in its life.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Offered, not answered.
    Pending,
    /// Applied; `before` is what "Revert to my settings" restores.
    Applied { before: Box<Selection> },
    /// "Keep mine" (colours stay applied), or reverted.
    Dismissed,
}

/// The selection and edits "Revert to my settings" goes back to.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub vendor: usize,
    pub printer: String,
    pub process: String,
    pub filaments: Vec<String>,
    pub bed: String,
    pub overlays: Overlays,
}

/// The card's model: the project's settings and how they map onto Mica's presets.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub settings: ProjectSettings,
    /// The printer preset the project's printer resolved to: `(vendor index, name)`.
    pub printer: Option<(usize, String)>,
    /// How many of the project's values differ from the current presets (set on use).
    pub changes: usize,
    /// "Update to current profile" was chosen: today's presets plus the listed changes.
    pub updated: bool,
    pub state: State,
}

impl Prompt {
    pub fn new(settings: ProjectSettings, printers: &[(usize, String, String)]) -> Self {
        let printer = match_printer(printers, &settings);
        let changes = settings.changes.count();
        Self {
            settings,
            printer,
            changes,
            updated: false,
            state: State::Pending,
        }
    }

    /// One line for the card: `printer Snapmaker U1, process 0.20 Standard (+3 changes),
    /// 4 filaments`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = &self.settings.printer {
            let shown = self
                .printer
                .as_ref()
                .map_or_else(|| strip_copy(p), |(_, name)| name.clone());
            let (model, variant) = crate::panels::sidebar::split_printer_name(&strip_copy(&shown));
            parts.push(format!(
                "printer {}",
                if variant.is_empty() {
                    model
                } else {
                    format!("{model} {variant}")
                }
            ));
        }
        if let Some(p) = &self.settings.process {
            let head = crate::preset_names::display_name(p);
            let changes = self.settings.changes.process.len();
            parts.push(if changes > 0 {
                format!("process {head} (+{changes} change{})", plural(changes))
            } else {
                format!("process {head}")
            });
        }
        let tools = self.settings.tool_count();
        if tools > 0 {
            let changed: usize = self
                .settings
                .changes
                .filaments
                .iter()
                .map(|f| usize::from(!f.is_empty()))
                .sum();
            parts.push(if changed > 0 {
                format!("{tools} filament{} ({changed} changed)", plural(tools))
            } else {
                format!("{tools} filament{}", plural(tools))
            });
        }
        if !self.settings.changes.printer.is_empty() {
            parts.push(format!(
                "{} printer change{}",
                self.settings.changes.printer.len(),
                plural(self.settings.changes.printer.len())
            ));
        }
        parts.join(", ")
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `#RRGGBB` text of a colour, as the `filament_colour` option stores it.
pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Puts the project's filament colours into the project colours (`Overlays::colours`: the
/// swatches and the models follow them; they are not filament edits). Tools beyond the
/// overlays' count are skipped.
pub fn colour_overlay(overlays: &mut Overlays, colours: &[Option<String>]) {
    let tools = overlays.filaments.len();
    for (tool, colour) in colours.iter().enumerate().take(tools) {
        if let Some(c) = colour {
            overlays.set_colour(tool, Some(c.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::threemf::RawConfig;
    use mica_profiles::project::same_machine;

    fn settings() -> ProjectSettings {
        ProjectSettings::parse(&RawConfig {
            path: "Metadata/project_settings.config".into(),
            text: r##"{"printer_settings_id":"Snapmaker U1 (0.4 nozzle) - Copy","printer_model":"Snapmaker U1","printer_variant":"0.4","print_settings_id":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","filament_settings_id":["Generic PLA Silk @System","Generic PLA @System"],"filament_colour":["#CB8F29","#000000"],"filament_type":["PLA","PLA"],"different_settings_to_system":["sparse_infill_density","","nozzle_temperature",""],"sparse_infill_density":"8%","nozzle_temperature":["215","225"]}"##.into(),
        })
        .unwrap()
    }

    fn printers() -> Vec<(usize, String, String)> {
        vec![
            (0, "Prusa".into(), "Prusa CORE One HF0.4 nozzle".into()),
            (1, "Snapmaker".into(), "Snapmaker U1 (0.2 nozzle)".into()),
            (1, "Snapmaker".into(), "Snapmaker U1 (0.4 nozzle)".into()),
        ]
    }

    #[test]
    fn printers_match_by_name_without_copy_suffix_then_by_model_and_nozzle() {
        let s = settings();
        assert_eq!(
            match_printer(&printers(), &s),
            Some((1, "Snapmaker U1 (0.4 nozzle)".into()))
        );
        let mut renamed = s.clone();
        renamed.printer = Some("My U1".into());
        assert_eq!(
            match_printer(&printers(), &renamed),
            Some((1, "Snapmaker U1 (0.4 nozzle)".into()))
        );
        renamed.printer_variant = Some("0.6".into());
        assert_eq!(match_printer(&printers(), &renamed), None);
        assert_eq!(strip_copy("X - Copy - Copy"), "X");
        assert_eq!(strip_copy("Copy"), "Copy");
    }

    #[test]
    fn decision_uses_the_same_printer_and_asks_otherwise() {
        let u1 = (1usize, "Snapmaker U1 (0.4 nozzle)".to_string());
        assert_eq!(
            decide(Some(&u1), 1, "Snapmaker U1 (0.4 nozzle)"),
            Decision::Use
        );
        assert_eq!(
            decide(Some(&u1), 1, "Snapmaker U1 (0.2 nozzle)"),
            Decision::Ask
        );
        let copy = (1usize, "Snapmaker U1 (0.4 nozzle) - Copy".to_string());
        assert_eq!(
            decide(Some(&copy), 1, "Snapmaker U1 (0.4 nozzle)"),
            Decision::Use
        );
        assert!(same_machine(
            "Prusa CORE One HF0.4 nozzle",
            "Prusa CORE One HF0.4 nozzle - Copy"
        ));
        assert!(!same_machine(
            "Prusa CORE One HF0.4 nozzle",
            "Prusa CORE One 0.4 nozzle"
        ));
        assert_eq!(
            decide(Some(&u1), 0, "Snapmaker U1 (0.4 nozzle)"),
            Decision::Ask
        );
        assert_eq!(decide(None, 1, "Snapmaker U1 (0.4 nozzle)"), Decision::Ask);
    }

    #[test]
    fn names_and_filaments_fall_back_sensibly() {
        let processes = vec![
            "0.12 Fine @Snapmaker U1 (0.4 nozzle)".to_string(),
            "0.20 Standard @Snapmaker U1 (0.4 nozzle)".to_string(),
        ];
        assert_eq!(
            match_name(&processes, "0.20 Standard @Snapmaker U1").as_deref(),
            Some("0.20 Standard @Snapmaker U1 (0.4 nozzle)")
        );
        assert_eq!(match_name(&processes, "0.28 Draft @Snapmaker U1"), None);
        let materials = vec![
            "Generic PETG @System".to_string(),
            "Generic PLA @System".to_string(),
            "Snapmaker PLA @System".to_string(),
        ];
        assert_eq!(
            match_filament(&materials, "Generic PLA @System", Some("PLA")).as_deref(),
            Some("Generic PLA @System")
        );
        assert_eq!(
            match_filament(&materials, "Generic PLA Silk @System", Some("PLA")).as_deref(),
            Some("Generic PLA @System")
        );
        assert_eq!(
            match_filament(&materials, "Fancy @System", Some("PETG")).as_deref(),
            Some("Generic PETG @System")
        );
        assert_eq!(
            match_filament(&materials, "Fancy @System", Some("ABS")),
            None
        );
        assert_eq!(match_filament(&materials, "Fancy @System", None), None);
    }

    #[test]
    fn prompt_summary_and_overlays_carry_changes_and_colours() {
        let p = Prompt::new(settings(), &printers());
        assert_eq!(p.changes, 2);
        assert_eq!(
            p.summary(),
            "printer Snapmaker U1 0.4, process 0.20 Standard (+1 change), 2 filaments (1 changed)"
        );
        let mut base = mica_core::config::Config::defaults();
        base.set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4").unwrap();
        let (layer, _) = p
            .settings
            .layer(&base, None, mica_profiles::project::Mode::Embedded);
        let mut o = Overlays::new(4);
        colour_overlay(&mut o, &p.settings.colours);
        o.project = std::sync::Arc::new(layer);
        // The project's values are not edits (no changed markers) but are sliced with.
        assert!(o.print.is_empty() && o.filaments.iter().all(|f| f.is_empty()));
        let mut c = base.clone();
        assert!(o.apply(&mut c).is_empty());
        assert_eq!(c.float("fill_density"), Some(8.));
        assert_eq!(c.float_at("temperature", 1), Some(225.));
        // Colours are project state, not filament edits.
        assert_eq!(o.colour(0), Some("#CB8F29"));
        assert_eq!(o.colour(1), Some("#000000"));
        // Colours alone, onto an existing overlay set with fewer tools than the project.
        let mut mine = Overlays::new(1);
        mine.print.insert("perimeters".into(), "3".into());
        colour_overlay(&mut mine, &p.settings.colours);
        assert_eq!(mine.filaments.len(), 1);
        assert_eq!(mine.colour(0), Some("#CB8F29"));
        assert!(mine.filaments[0].is_empty());
        assert_eq!(mine.print.len(), 1);
    }

    /// A project saved by Snapmaker Orca keeps its flow-variant process options as lists
    /// (`"enable_overhang_speed": ["1", "1"]`): it opens with those values, which the settings
    /// show and the slicer reads as single values, without a rejection notice.
    #[test]
    fn snapmaker_orca_flow_variant_lists_open_as_their_values() {
        let settings = ProjectSettings::parse(&RawConfig {
            path: "Metadata/project_settings.config".into(),
            text: r##"{"printer_settings_id":"Snapmaker U1 (0.4 nozzle)","print_settings_id":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","filament_settings_id":["Generic PLA @System","Generic PLA @System"],"process_flow_support":["standard","high_flow"],"filament_volume_type":["standard","standard"],"enable_overhang_speed":["0","0"],"slow_down_layers":["3","3"],"outer_wall_speed":["123","500"],"overhang_2_4_speed":["40%","40%"]}"##.into(),
        })
        .unwrap();
        let mut base = mica_core::config::Config::defaults();
        base.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        base.set_raw("enable_overhang_speed", "1").unwrap();
        let (layer, notes) = settings.layer(
            &base,
            Some(mica_profiles::orca::Dialect::Orca242),
            mica_profiles::project::Mode::Embedded,
        );
        assert!(notes.is_empty(), "{notes:?}");
        let mut o = Overlays::new(2);
        o.project = std::sync::Arc::new(layer);
        let mut c = base.clone();
        assert!(o.apply(&mut c).is_empty());
        for tool in 0..2 {
            assert_eq!(
                crate::overlays::tool_raw(&c, "enable_overhang_speed", tool).as_deref(),
                Some("0")
            );
        }
        // The text the G-code reads is the value, not the list.
        assert_eq!(c.extra("enable_overhang_speed"), Some("0"));
        assert_eq!(c.extra("overhang_2_4_speed"), Some("40%"));
        assert_eq!(c.raw("slow_down_layers").as_deref(), Some("3"));
        // Standard-flow filaments read the standard (first) value.
        assert_eq!(c.float("external_perimeter_speed"), Some(123.));
    }
}
