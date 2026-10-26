// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Filament sync: the loaded spools the printer reports become the project's per-tool colour
//! (and material preset, when a matching one exists), and the Filament card warns when the
//! project differs from what is loaded.

use crate::app::App;
use eframe::egui;
use mica_printhost::LoadedFilament;

impl App {
    /// The physical printer the selected profile prints on: the one linked to it, else the
    /// first of the same model, else the first of the same family.
    pub(crate) fn linked_printer(&self) -> Option<usize> {
        self.printers
            .default_for(self.profiles.vendor_name(), &self.profiles.printer)
    }

    /// The linked printer's loaded filaments, once it has answered (monitor or test).
    pub(crate) fn linked_loaded(&self) -> Option<(usize, &[LoadedFilament])> {
        let i = self.linked_printer()?;
        self.printers.loaded_filaments(i).map(|f| (i, f))
    }

    /// Material the project uses on `tool` (`filament_type` of its preset or overlay).
    fn project_material(&self, tool: usize) -> Option<String> {
        self.effective_config()
            .and_then(|c| c.string_at("filament_type", tool))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// What the linked printer has loaded on `tool`, next to the project's filament
    /// (`None` when no printer is linked or it has not said).
    pub(crate) fn printer_filament(&self, tool: usize) -> Option<PrinterFilament> {
        let (i, loaded) = self.linked_loaded()?;
        let f = loaded.iter().find(|f| f.tool as usize == tool)?;
        let printer = self.printers.list.get(i)?;
        let loaded_colour = f.colour.as_deref().and_then(mica_core::color::parse_hex);
        let loaded_material = f
            .material
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(String::from);
        let project_colour = self.tool_colours().get(tool).copied();
        let project_material = self.project_material(tool);
        let colour_differs = matches!((loaded_colour, project_colour),
            (Some(a), Some(b)) if !mica_printhost::colour::same_colour(a, b));
        let type_differs = matches!((&loaded_material, &project_material),
            (Some(a), Some(b)) if !a.eq_ignore_ascii_case(b));
        let signature = loaded_signature(f);
        let ignored = self
            .filament_ignored
            .get(&(printer.id, tool))
            .is_some_and(|s| *s == signature);
        Some(PrinterFilament {
            tool,
            printer: printer.name.clone(),
            slot: if printer.kind == crate::printhost::config::Kind::SnapmakerU1 {
                format!("slot {}", tool + 1)
            } else {
                format!("T{}", tool + 1)
            },
            loaded_colour,
            loaded_material,
            project_colour,
            project_material,
            colour_differs,
            type_differs,
            ignored,
        })
    }

    /// "Ignore": the difference on `tool` is not shown again until the printer reports
    /// another filament there.
    pub(crate) fn ignore_filament_difference(&mut self, tool: usize) {
        let Some((i, loaded)) = self.linked_loaded() else {
            return;
        };
        let Some(f) = loaded.iter().find(|f| f.tool as usize == tool) else {
            return;
        };
        let key = (self.printers.list[i].id, tool);
        let signature = loaded_signature(f);
        self.filament_ignored.insert(key, signature);
    }

    /// "Use printer's color": this tool's color becomes the loaded one (one undo step).
    pub(crate) fn use_printer_colour(&mut self, tool: usize) {
        let Some(c) = self.printer_filament(tool).and_then(|f| f.loaded_colour) else {
            return;
        };
        self.set_filament_colour(
            tool,
            super::colour::Pick {
                colour: c,
                alpha: None,
                commit: true,
            },
        );
    }

    /// "Use printer's filament": this tool's color and material from the printer.
    pub(crate) fn use_printer_filament(&mut self, tool: usize) {
        let Some((i, loaded)) = self.linked_loaded() else {
            return;
        };
        let one: Vec<LoadedFilament> = loaded
            .iter()
            .filter(|f| f.tool as usize == tool)
            .cloned()
            .collect();
        self.apply_loaded_filaments(i, &one);
    }

    /// "Sync from printer": applies the loaded filaments when they are known, else asks the
    /// printer first (a test connection) and applies when the answer comes.
    pub(crate) fn sync_filaments_from_printer(&mut self, ctx: &egui::Context) {
        let Some(i) = self.linked_printer() else {
            self.status = "No printer is linked to this profile yet; add one in My printers".into();
            return;
        };
        if let Some(loaded) = self
            .printers
            .loaded_filaments(i)
            .map(<[LoadedFilament]>::to_vec)
        {
            self.apply_loaded_filaments(i, &loaded);
            return;
        }
        let c = ctx.clone();
        self.printers.test(i, move || c.request_repaint());
        self.sync_pending = self.printers.id_at(i);
        self.status = format!("Asking {} what is loaded…", self.printers.list[i].name);
    }

    /// Finishes a pending sync once the test connection has answered.
    pub(crate) fn finish_pending_sync(&mut self) {
        let Some(id) = self.sync_pending else { return };
        // By id: the printer may have been removed (or another moved into its place).
        let Some(i) = self.printers.position(id) else {
            self.sync_pending = None;
            return;
        };
        let Some(p) = self.printers.list.get(i) else {
            self.sync_pending = None;
            return;
        };
        match self.printers.tests.get(&p.name) {
            Some(crate::printhost::Test::Running(_)) => {}
            Some(crate::printhost::Test::Done(report)) => {
                let loaded = report.filaments.clone();
                self.sync_pending = None;
                self.apply_loaded_filaments(i, &loaded);
            }
            Some(crate::printhost::Test::Failed(e)) => {
                self.sync_pending = None;
                self.notify_error(e.headline.clone(), format!("{} — {}", e.headline, e.advice));
            }
            None => self.sync_pending = None,
        }
    }

    /// Sets each tool's colour (and material preset when one matches) from the printer. One
    /// undo step. Returns how many tools changed.
    pub(crate) fn apply_loaded_filaments(
        &mut self,
        printer: usize,
        loaded: &[LoadedFilament],
    ) -> usize {
        let name = self
            .printers
            .list
            .get(printer)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        if loaded.is_empty() {
            self.notify_warning(format!("{name} does not report what is loaded"));
            return 0;
        }
        let tools = self.tool_count();
        let mut changed = 0;
        let mut colours = 0;
        let mut materials = 0;
        let mut unmatched = vec![];
        self.record("Sync filaments from printer");
        self.overlays.resize(tools);
        for f in loaded {
            let t = f.tool as usize;
            if t >= tools {
                continue;
            }
            let mut this = false;
            if let Some(rgba) = f
                .colour
                .as_deref()
                .and_then(mica_core::color::parse_hex_rgba)
            {
                let c = [rgba[0], rgba[1], rgba[2]];
                // The printer's alpha too (`#RRGGBB` when it is opaque).
                let hex = super::colour::colour_text("", rgba);
                // A color within the tolerance is the same spool color: the project keeps its
                // own description of it.
                let same = self
                    .tool_colours()
                    .get(t)
                    .is_some_and(|mine| mica_printhost::colour::same_colour(*mine, c));
                if !same && self.overlays.colour(t) != Some(hex.as_str()) {
                    self.overlays.set_colour(t, Some(hex));
                    colours += 1;
                    this = true;
                }
            }
            if let Some(material) = f
                .material
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                && !self
                    .project_material(t)
                    .is_some_and(|m| m.eq_ignore_ascii_case(material))
            {
                match mica_profiles::project::match_filament(
                    &self.profiles.materials,
                    &format!("Generic {material}"),
                    Some(material),
                ) {
                    Some(preset) => {
                        if self.profiles.filaments[t] != preset {
                            self.profiles.filaments[t] = preset;
                            // The colour (project state) survives; other edits belong to
                            // the old preset.
                            self.overlays.filaments[t].clear();
                            materials += 1;
                            this = true;
                        }
                    }
                    None => unmatched.push(format!("T{} {material}", t + 1)),
                }
            }
            if this {
                changed += 1;
            }
        }
        self.profiles.refresh();
        self.settings_changed();
        let mut status = if changed == 0 {
            format!("Project already matches what {name} has loaded")
        } else {
            format!(
                "Synced from {name}: {colours} color{}, {materials} material{}",
                if colours == 1 { "" } else { "s" },
                if materials == 1 { "" } else { "s" }
            )
        };
        if !unmatched.is_empty() {
            status.push_str(&format!(" · no preset for {}", unmatched.join(", ")));
        }
        self.notify_info(status);
        changed
    }
}

/// What a printer reports for one tool, as a key that changes when the spool does.
fn loaded_signature(f: &LoadedFilament) -> String {
    format!(
        "{}|{}",
        f.material.as_deref().unwrap_or("").trim(),
        f.colour.as_deref().unwrap_or("").trim()
    )
}

/// One tool of the Filament card next to what the linked printer has loaded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PrinterFilament {
    pub tool: usize,
    pub printer: String,
    /// `slot 2` (U1) or `T2`.
    pub slot: String,
    pub loaded_colour: Option<[u8; 3]>,
    pub loaded_material: Option<String>,
    pub project_colour: Option<[u8; 3]>,
    pub project_material: Option<String>,
    /// Colors further apart than the tolerance (`colour::SAME_COLOUR_DELTA_E`).
    pub colour_differs: bool,
    pub type_differs: bool,
    /// Ignored until the loaded filament changes.
    pub ignored: bool,
}

impl PrinterFilament {
    /// A difference to show (not ignored).
    pub fn differs(&self) -> bool {
        !self.ignored && (self.colour_differs || self.type_differs)
    }

    /// The popover's sentences: `Printer has yellow loaded in slot 1 (project: gold)`, and
    /// for another type `Printer has PETG loaded in slot 1 (project: PLA)`. No hex codes.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![];
        if self.type_differs
            && let (Some(theirs), Some(mine)) = (&self.loaded_material, &self.project_material)
        {
            out.push(format!(
                "Printer has {theirs} loaded in {} (project: {mine})",
                self.slot
            ));
        }
        if self.colour_differs
            && let (Some(theirs), Some(mine)) = (self.loaded_colour, self.project_colour)
        {
            let (theirs, mine) = mica_printhost::colour::contrast_names(theirs, mine);
            out.push(format!(
                "Printer has {theirs} loaded in {} (project: {mine})",
                self.slot
            ));
        }
        out
    }

    /// The printer swatch's tooltip: what is loaded, in words; hex codes only here.
    pub fn tooltip(&self) -> String {
        use mica_printhost::colour::{colour_name, hex};
        let what = match (self.loaded_colour, &self.loaded_material) {
            (Some(c), Some(m)) => format!("{} {m} ({})", colour_name(c), hex(c)),
            (Some(c), None) => format!("{} ({})", colour_name(c), hex(c)),
            (None, Some(m)) => m.clone(),
            (None, None) => "unknown filament".into(),
        };
        let mut text = format!("{} has {what} loaded in {}", self.printer, self.slot);
        if let Some(c) = self.project_colour {
            text.push_str(&format!("\nProject: {} ({})", colour_name(c), hex(c)));
        }
        if self.ignored && (self.colour_differs || self.type_differs) {
            text.push_str("\nDifference ignored until the loaded filament changes");
        } else if !self.differs() {
            text.push_str("\nMatches the project");
        } else {
            text.push_str("\nClick for options");
        }
        text
    }
}

/// The lines above the Filament list, with whether each is a warning: type differences
/// (`1 filament type differs from the printer`, warning) and color differences (`2 colors
/// differ from the printer`, information). Empty when nothing differs.
pub(crate) fn summary_lines(rows: &[PrinterFilament]) -> Vec<(String, bool)> {
    let count =
        |f: fn(&PrinterFilament) -> bool| rows.iter().filter(|r| r.differs() && f(r)).count();
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            many.to_string()
        }
    };
    let (types, colours) = (count(|r| r.type_differs), count(|r| r.colour_differs));
    let mut out = vec![];
    if types > 0 {
        out.push((
            format!(
                "{types} {} from the printer",
                plural(types, "filament type differs", "filament types differ")
            ),
            true,
        ));
    }
    if colours > 0 {
        out.push((
            format!(
                "{colours} {} from the printer",
                plural(colours, "color differs", "colors differ")
            ),
            false,
        ));
    }
    out
}
