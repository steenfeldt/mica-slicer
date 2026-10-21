// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! How preset names read on screen. Orca and PrusaSlicer tag their presets with the printers
//! they are made for (`Generic PLA @System`, `Snapmaker PLA @U1`, `0.20mm SPEED @COREONE
//! HF0.4`); Mica shows the name without that tag (`Generic PLA`) and keeps the full name in
//! the tooltip. The full name stays the preset's identity everywhere else: keys, settings,
//! projects, G-code and the command line never see these labels.
//!
//! In a list, two presets can lose their difference with the tag (`Generic PLA @System` and
//! `Generic PLA @U1`, or a copy `Generic PLA @U1 Tweaked`); those get the tag back as a muted
//! suffix (`Generic PLA · System`, `Generic PLA · U1`).

use eframe::egui;
use std::collections::HashMap;

/// A preset name without its ` @…` compatibility tag: `Prusament PLA @COREONE HF0.4` →
/// `Prusament PLA`. Names without a tag (or with nothing before it) are shown as they are.
pub fn display_name(name: &str) -> &str {
    match name.split_once(" @") {
        Some((head, _)) if !head.trim().is_empty() => head.trim(),
        _ => name.trim(),
    }
}

/// The compatibility tag a preset name carries (`COREONE HF0.4`, `System`), or `""`.
pub fn tag(name: &str) -> &str {
    match name.split_once(" @") {
        Some((head, tail)) if !head.trim().is_empty() => tail.trim(),
        _ => "",
    }
}

/// One preset's label in a list: the shown name, and a muted suffix that tells it apart
/// from another entry with the same shown name (empty when it needs none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub suffix: String,
}

impl Label {
    /// The label as one line of text: `Generic PLA` or `Generic PLA · U1`.
    pub fn text(&self) -> String {
        if self.suffix.is_empty() {
            self.name.clone()
        } else {
            format!("{} · {}", self.name, self.suffix)
        }
    }

    /// The label of a preset shown on its own (nothing to tell it apart from).
    pub fn single(name: &str) -> Self {
        Self {
            name: display_name(name).to_string(),
            suffix: String::new(),
        }
    }
}

/// Labels for the presets of one list, in order. Entries whose shown names collide get their
/// tag as the suffix (an entry without a tag keeps none); the rare entry that still can't be
/// told apart that way shows its full name as the suffix.
pub fn labels<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<Label> {
    labels_ranked(names.into_iter().map(|n| (n, false)))
}

/// Like `labels`, for a list with secondary entries (`true`: the presets made for other
/// printers, listed apart under their own header). A secondary entry gets its tag when its
/// shown name collides with any other entry; the primary ones only among themselves, so
/// turning the other printers' presets on doesn't change how the printer's own read.
pub fn labels_ranked<'a>(items: impl IntoIterator<Item = (&'a str, bool)>) -> Vec<Label> {
    let items: Vec<(&str, bool)> = items.into_iter().collect();
    let names: Vec<&str> = items.iter().map(|(n, _)| *n).collect();
    let mut primary: HashMap<&str, usize> = HashMap::new();
    let mut all: HashMap<&str, usize> = HashMap::new();
    for (n, secondary) in &items {
        *all.entry(display_name(n)).or_default() += 1;
        if !secondary {
            *primary.entry(display_name(n)).or_default() += 1;
        }
    }
    let mut out: Vec<Label> = items
        .iter()
        .map(|(n, secondary)| {
            let name = display_name(n);
            let count = if *secondary { all[name] } else { primary[name] };
            let suffix = if count > 1 { tag(n) } else { "" };
            Label {
                name: name.to_string(),
                suffix: suffix.to_string(),
            }
        })
        .collect();
    // Still the same text (a primary entry against an untagged secondary one, or a name that
    // reads like another's label): the tag where it has none yet, then the full name.
    for last in [false, true] {
        let mut seen: HashMap<String, usize> = HashMap::new();
        for l in &out {
            *seen.entry(l.text()).or_default() += 1;
        }
        for (l, n) in out.iter_mut().zip(&names) {
            if seen[&l.text()] < 2 || l.name == n.trim() {
                continue;
            }
            if !last && l.suffix.is_empty() {
                l.suffix = tag(n).to_string();
            } else if last {
                l.suffix = n.trim().to_string();
            }
        }
    }
    out
}

/// The label of `name` as it reads in a list of `(name, secondary)` items (the same as the
/// list's row for it; see `labels_ranked`).
pub fn label_in<'a>(name: &str, items: impl IntoIterator<Item = (&'a str, bool)>) -> Label {
    let items: Vec<(&str, bool)> = items.into_iter().collect();
    match items.iter().position(|(n, _)| *n == name) {
        Some(i) => labels_ranked(items).swap_remove(i),
        None => Label::single(name),
    }
}

/// A label's own name, for widgets that show it, and the full preset name for the tooltip
/// when it differs from what is shown.
pub fn tooltip(name: &str, label: &Label) -> Option<String> {
    (label.text() != name).then(|| name.to_string())
}

/// A label as widget text in the button font: the name in `colour`, the suffix muted.
pub fn text(ui: &egui::Ui, label: &Label, colour: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::TextStyle::Button.resolve(ui.style());
    job.append(
        &label.name,
        0.,
        egui::TextFormat::simple(font.clone(), colour),
    );
    if !label.suffix.is_empty() {
        let suffix = format!(" · {}", label.suffix);
        let muted = crate::theme::of(ui).muted;
        job.append(&suffix, 0., egui::TextFormat::simple(font, muted));
    }
    job
}

/// Whether a search's lower-case words all occur in the preset's full or shown name.
pub fn matches(name: &str, words: &[String]) -> bool {
    let full = name.to_lowercase();
    let shown = display_name(name).to_lowercase();
    words
        .iter()
        .all(|w| full.contains(w.as_str()) || shown.contains(w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_of_every_bundled_form_are_dropped() {
        for (full, shown, t) in [
            ("Generic PLA @System", "Generic PLA", "System"),
            ("Snapmaker PLA @U1", "Snapmaker PLA", "U1"),
            (
                "Prusament PLA @COREONE HF0.4",
                "Prusament PLA",
                "COREONE HF0.4",
            ),
            ("0.20mm SPEED @PrusaXL", "0.20mm SPEED", "PrusaXL"),
            ("0.20mm SPEED @XLIS 0.4", "0.20mm SPEED", "XLIS 0.4"),
            ("Prusament PLA @XLIS", "Prusament PLA", "XLIS"),
            ("Prusament PETG @XL+ HF0.6", "Prusament PETG", "XL+ HF0.6"),
            ("Generic PLA @PG 0.6", "Generic PLA", "PG 0.6"),
            ("Generic PLA @MMU1", "Generic PLA", "MMU1"),
            (
                "0.15mm QUALITY @0.6 nozzle MINI",
                "0.15mm QUALITY",
                "0.6 nozzle MINI",
            ),
            (
                "0.10mm DETAIL @0.25 nozzle MK3",
                "0.10mm DETAIL",
                "0.25 nozzle MK3",
            ),
            (
                "Prusament PLA @COREONEL HT0.4",
                "Prusament PLA",
                "COREONEL HT0.4",
            ),
            (
                "0.20 Standard @Snapmaker U1 (0.4 nozzle)",
                "0.20 Standard",
                "Snapmaker U1 (0.4 nozzle)",
            ),
            (
                "Snapmaker PLA @U1 0.6 nozzle",
                "Snapmaker PLA",
                "U1 0.6 nozzle",
            ),
            (
                "0.16 Optimal @Snapmaker U1 (0.4+0.6 nozzle)",
                "0.16 Optimal",
                "Snapmaker U1 (0.4+0.6 nozzle)",
            ),
            (
                "Snapmaker PLA @Snapmaker J1 (0.4 nozzle)",
                "Snapmaker PLA",
                "Snapmaker J1 (0.4 nozzle)",
            ),
            ("Bambu PLA Basic @base", "Bambu PLA Basic", "base"),
            (
                "Extrudr GreenTEC Pro @COREONE HF0.4 Tweaked",
                "Extrudr GreenTEC Pro",
                "COREONE HF0.4 Tweaked",
            ),
            // No tag, or nothing before it: shown as is.
            ("My PLA", "My PLA", ""),
            ("  Padded PLA  ", "Padded PLA", ""),
            ("@System", "@System", ""),
            ("user@example PLA", "user@example PLA", ""),
        ] {
            assert_eq!(display_name(full), shown, "{full}");
            assert_eq!(tag(full), t, "{full}");
            assert!(!Label::single(full).text().contains(" @"), "{full}");
        }
    }

    #[test]
    fn colliding_names_get_their_tag_as_a_suffix() {
        let names = [
            "Generic PLA @System",
            "Generic PLA @U1",
            "Generic PLA",
            "Snapmaker PLA @U1",
            "Generic PETG @U1",
        ];
        let texts: Vec<String> = labels(names).iter().map(Label::text).collect();
        assert_eq!(
            texts,
            [
                "Generic PLA · System",
                "Generic PLA · U1",
                "Generic PLA",
                "Snapmaker PLA",
                "Generic PETG"
            ]
        );
        // A copy with words after the tag.
        let copy = labels([
            "Prusament PLA @COREONE HF0.4",
            "Prusament PLA @COREONE HF0.4 Tweaked",
        ]);
        assert_eq!(copy[0].text(), "Prusament PLA · COREONE HF0.4");
        assert_eq!(copy[1].text(), "Prusament PLA · COREONE HF0.4 Tweaked");
        // A user preset literally named like another's label falls back to the full name.
        let odd = labels(["Generic PLA · U1", "Generic PLA @U1", "Generic PLA @System"]);
        let texts: Vec<String> = odd.iter().map(Label::text).collect();
        assert_eq!(texts[0], "Generic PLA · U1");
        assert_eq!(texts[1], "Generic PLA · Generic PLA @U1");
        // An other printer's untagged preset reading like one of the printer's own.
        let untagged = labels_ranked([
            ("3D-Fuel PLA+ @COREONE HF0.4", false),
            ("3D-Fuel PLA+", true),
        ]);
        let texts: Vec<String> = untagged.iter().map(Label::text).collect();
        assert_eq!(texts, ["3D-Fuel PLA+ · COREONE HF0.4", "3D-Fuel PLA+"]);
        // The label of one entry matches its row in the list.
        let items = || names.iter().map(|n| (*n, false));
        assert_eq!(
            label_in("Generic PLA @U1", items()).text(),
            "Generic PLA · U1"
        );
        assert_eq!(
            label_in("Snapmaker PLA @U1", items()).text(),
            "Snapmaker PLA"
        );
        assert_eq!(label_in("Not listed @U1", items()).text(), "Not listed");
        // The other printers' presets get their tag; the printer's own stay clean.
        let ranked = [
            ("0.20mm SPEED @COREONE HF0.4", false),
            ("0.20mm SPEED @XL HF0.4", true),
            ("0.20mm SPEED @MK4S HF0.4", true),
            ("0.05mm DETAIL @COREONE 0.25", true),
        ];
        let texts: Vec<String> = labels_ranked(ranked).iter().map(Label::text).collect();
        assert_eq!(
            texts,
            [
                "0.20mm SPEED",
                "0.20mm SPEED · XL HF0.4",
                "0.20mm SPEED · MK4S HF0.4",
                "0.05mm DETAIL"
            ]
        );
        assert_eq!(
            label_in("0.20mm SPEED @XL HF0.4", ranked).text(),
            "0.20mm SPEED · XL HF0.4"
        );
    }

    #[test]
    fn tooltips_and_search_keep_the_full_name() {
        let l = Label::single("Generic PLA @System");
        assert_eq!(
            tooltip("Generic PLA @System", &l).as_deref(),
            Some("Generic PLA @System")
        );
        assert_eq!(tooltip("My PLA", &Label::single("My PLA")), None);
        let w = |s: &str| {
            s.split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        };
        assert!(matches("Generic PLA @System", &w("generic pla")));
        assert!(matches("Generic PLA @System", &w("@system")));
        assert!(matches("Generic PLA @System", &w("PLA system")));
        assert!(!matches("Generic PLA @System", &w("petg")));
    }
}
