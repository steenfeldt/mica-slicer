// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Minimal INI reader for PrusaSlicer files: `[section]` headers and `key = value` lines.
//! Values are kept verbatim (PrusaSlicer's own escaping is undone by the config layer).

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    /// Full header text, e.g. `printer:Prusa CORE One HF0.4 nozzle`. Empty for keys before any header.
    pub name: String,
    /// Entries in file order.
    pub entries: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Parses INI text. Lines starting with `#` (or `;` outside a value) are comments.
pub fn parse(text: &str) -> Vec<Section> {
    let mut sections = vec![Section::default()];
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            sections.push(Section {
                name: name.to_string(),
                entries: Vec::new(),
            });
            continue;
        }
        let (key, value) = match line.split_once(" = ") {
            Some((k, v)) => (k.trim(), v),
            None => match line.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim_start()),
                None => continue,
            },
        };
        sections
            .last_mut()
            .unwrap()
            .entries
            .push((key.to_string(), value.to_string()));
    }
    if sections[0].entries.is_empty() {
        sections.remove(0);
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_and_values() {
        let s = parse(
            "# comment\n[vendor]\nname = Prusa\n\n[printer:A]\nstart_gcode = M17\\nG28 ; home\nbed_shape = 0x0,1x1\nempty =\n",
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "vendor");
        assert_eq!(s[1].get("start_gcode"), Some("M17\\nG28 ; home"));
        assert_eq!(s[1].get("empty"), Some(""));
    }

    #[test]
    fn value_may_contain_equals() {
        let s = parse("[a]\ncond = printer_model==\"XL\" and x = 1\n");
        assert_eq!(s[0].get("cond"), Some("printer_model==\"XL\" and x = 1"));
    }
}
