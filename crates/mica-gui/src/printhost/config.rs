// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The physical printers Mica can talk to (`printers.toml`, see `mica_printers::config`) and
//! how well each fits a Mica printer profile.

pub use mica_printers::config::*;

use std::collections::BTreeSet;

/// How well a physical printer fits a Mica profile, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    /// Explicitly linked to this profile.
    Linked,
    /// Same backend family and the reported model names the profile's model.
    Model,
    /// Same backend family (PrusaLink for Prusa profiles, Moonraker / U1 for Snapmaker).
    Family,
    /// Another kind of printer altogether.
    Other,
}

/// Lower-case alphanumeric tokens of a model name, without the vendor words.
fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| !t.is_empty() && !matches!(t.as_str(), "original" | "prusa" | "snapmaker"))
        .collect()
}

/// Ranks `printer` for the profile `vendor/preset` whose model name is `model` (the part of
/// the preset name before the nozzle variant).
pub fn fit(printer: &Printer, vendor: &str, preset: &str, model: &str) -> Fit {
    if printer.profile.as_deref() == Some(&format!("{vendor}/{preset}")) {
        return Fit::Linked;
    }
    if !printer.kind.vendor().eq_ignore_ascii_case(vendor) {
        return Fit::Other;
    }
    let wanted = tokens(model);
    if printer.kind == Kind::SnapmakerU1 && wanted.contains("u1") {
        return Fit::Model;
    }
    if let Some(reported) = &printer.model {
        // "Original Prusa XL 5T" fits "Original Prusa XL 5T Input Shaper" (and the other
        // way round); "MK4" does not fit "MK4S".
        let got = tokens(reported);
        if !wanted.is_empty()
            && !got.is_empty()
            && (wanted.is_subset(&got) || got.is_subset(&wanted))
        {
            return Fit::Model;
        }
    }
    Fit::Family
}

/// Printers ordered for a profile: linked, then by model, then same family, then the rest;
/// stable within a rank. Returns `(index, fit)`.
pub fn ranked(printers: &[Printer], vendor: &str, preset: &str, model: &str) -> Vec<(usize, Fit)> {
    let mut out: Vec<(usize, Fit)> = printers
        .iter()
        .enumerate()
        .map(|(i, p)| (i, fit(p, vendor, preset, model)))
        .collect();
    out.sort_by_key(|(i, f)| (*f, *i));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(kind: Kind, name: &str, model: Option<&str>, profile: Option<&str>) -> Printer {
        let mut p = Printer::new(kind);
        p.name = name.into();
        p.model = model.map(str::to_string);
        p.profile = profile.map(str::to_string);
        p
    }

    #[test]
    fn profiles_pick_the_first_matching_physical_printer() {
        let printers = vec![
            printer(Kind::SnapmakerU1, "U1", None, None),
            printer(Kind::PrusaLink, "CORE One", Some("Prusa CORE One"), None),
            printer(Kind::PrusaLink, "XL", Some("Original Prusa XL 5T"), None),
            printer(
                Kind::PrusaLink,
                "XL linked",
                None,
                Some("Prusa/Original Prusa XL 5T Input Shaper 0.4 nozzle"),
            ),
        ];
        let r = ranked(
            &printers,
            "Prusa",
            "Original Prusa XL 5T Input Shaper 0.4 nozzle",
            "Original Prusa XL 5T Input Shaper",
        );
        assert_eq!(
            r,
            vec![
                (3, Fit::Linked),
                (2, Fit::Model),
                (1, Fit::Family),
                (0, Fit::Other)
            ]
        );
        let r = ranked(
            &printers,
            "Prusa",
            "Prusa CORE One HF0.4 nozzle",
            "Prusa CORE One",
        );
        assert_eq!(r[0], (1, Fit::Model));
        let r = ranked(
            &printers,
            "Snapmaker",
            "Snapmaker U1 (0.4 nozzle)",
            "Snapmaker U1",
        );
        assert_eq!(r[0], (0, Fit::Model));
        assert!(r[1..].iter().all(|(_, f)| *f == Fit::Other));
    }
}
