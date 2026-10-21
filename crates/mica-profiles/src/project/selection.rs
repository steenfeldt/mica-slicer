// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica GUI and CLI project printer selection.
// Changes: Match model/variant first, then inherited presets and copy-stripped settings IDs.

use super::ProjectSettings;

/// `Prusa CORE One HF0.4 nozzle` → (`Prusa CORE One`, `HF 0.4`); `Snapmaker U1 (0.4 nozzle)`
/// → (`Snapmaker U1`, `0.4`).
pub fn split_printer_name(name: &str) -> (String, String) {
    let name = name.trim();
    if let Some((model, rest)) = name.split_once('(')
        && let Some(inner) = rest.strip_suffix(')')
    {
        let variant = inner.trim().trim_end_matches("nozzle").trim();
        return (model.trim().to_string(), variant.to_string());
    }
    let stripped = name.strip_suffix(" nozzle").unwrap_or(name).trim();
    match stripped.rsplit_once(' ') {
        Some((model, variant)) if variant.chars().any(|c| c.is_ascii_digit()) => {
            let variant = match variant.strip_prefix("HF") {
                Some(v) => format!("HF {v}"),
                None => variant.to_string(),
            };
            (model.trim_end_matches(" -").trim().to_string(), variant)
        }
        _ => (stripped.to_string(), String::new()),
    }
}

/// What to do with a project's settings when it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The project was made for the selected printer: use its settings right away (the card
    /// says so and offers a revert).
    Use,
    /// Another printer, or one Mica does not have: ask.
    Ask,
}

/// A project made for the printer that is already selected is used without asking; one for
/// another printer (or an unknown one) is offered. "The same printer" is the same preset, or
/// a copy of it, or the same model and nozzle (`Snapmaker U1 (0.4 nozzle) - Copy` is the
/// selected `Snapmaker U1 (0.4 nozzle)`).
pub fn decide(matched_printer: Option<&(usize, String)>, vendor: usize, printer: &str) -> Decision {
    match matched_printer {
        Some((vi, name)) if *vi == vendor && same_machine(name, printer) => Decision::Use,
        _ => Decision::Ask,
    }
}

/// Whether two printer presets name the same model and nozzle, copies included.
pub fn same_machine(a: &str, b: &str) -> bool {
    let key = |name: &str| {
        let (model, variant) = split_printer_name(&strip_copy(name));
        (model.to_ascii_lowercase(), variant.to_ascii_lowercase())
    };
    key(a) == key(b)
}

/// `Snapmaker U1 (0.4 nozzle) - Copy` → `Snapmaker U1 (0.4 nozzle)`: Orca's and Prusa's
/// suffixes for duplicated presets.
pub fn strip_copy(name: &str) -> String {
    let mut s = name.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let next = [" - copy", " (copy)", " copy", " - modified", " (modified)"]
            .iter()
            .find_map(|suffix| lower.strip_suffix(suffix).map(str::len));
        match next {
            Some(len) if len > 0 => s = s[..len].trim_end(),
            _ => return s.to_string(),
        }
    }
}

/// The part of a preset name before its ` @printer` qualifier.
fn stem(name: &str) -> String {
    name.split(" @").next().unwrap_or(name).trim().to_string()
}

/// The best candidate for a preset name: exact, then without a copy suffix, then the same
/// stem (`0.20 Standard @Snapmaker U1` ~ `0.20 Standard @Snapmaker U1 (0.4 nozzle)`).
pub fn match_name(candidates: &[String], wanted: &str) -> Option<String> {
    if let Some(c) = candidates.iter().find(|c| *c == wanted) {
        return Some(c.clone());
    }
    let stripped = strip_copy(wanted);
    if let Some(c) = candidates.iter().find(|c| **c == stripped) {
        return Some(c.clone());
    }
    let want = stem(&stripped).to_ascii_lowercase();
    candidates
        .iter()
        .find(|c| stem(c).to_ascii_lowercase() == want)
        .cloned()
}

/// A filament preset for a project filament: by name, else a generic preset of the same
/// material type (`Generic PLA`), else none (the current selection stays).
pub fn match_filament(
    materials: &[String],
    wanted: &str,
    material_type: Option<&str>,
) -> Option<String> {
    if let Some(m) = match_name(materials, wanted) {
        return Some(m);
    }
    let ty = material_type?.trim().to_ascii_lowercase();
    if ty.is_empty() {
        return None;
    }
    let generic = format!("generic {ty}");
    materials
        .iter()
        .find(|m| m.to_ascii_lowercase().starts_with(&generic))
        .or_else(|| {
            materials
                .iter()
                .find(|m| m.to_ascii_lowercase().contains(&ty))
        })
        .cloned()
}

/// The printer preset a project names, among every vendor's printers `(vendor index, vendor,
/// name)`: by model and nozzle variant, then inherited presets, then settings ID.
pub fn match_printer(
    printers: &[(usize, String, String)],
    settings: &ProjectSettings,
) -> Option<(usize, String)> {
    let by_name = |name: &str| {
        printers
            .iter()
            .find(|(_, _, n)| n == name)
            .map(|(vi, _, n)| (*vi, n.clone()))
    };
    if let Some(model) = settings.printer_model.as_deref().map(str::trim)
        && !model.is_empty()
        && let Some(variant) = settings.printer_variant.as_deref().map(str::trim)
        && !variant.is_empty()
        && let Some((vi, _, name)) = printers.iter().find(|(_, _, name)| {
            let (candidate_model, candidate_variant) = split_printer_name(name);
            candidate_model.eq_ignore_ascii_case(model)
                && candidate_variant
                    .replace(' ', "")
                    .eq_ignore_ascii_case(&variant.replace(' ', ""))
        })
    {
        return Some((*vi, name.clone()));
    }
    settings
        .printer_inherits
        .iter()
        .map(String::as_str)
        .chain(settings.printer.as_deref())
        .find_map(|name| by_name(name).or_else(|| by_name(&strip_copy(name))))
}
