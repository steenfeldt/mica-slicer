// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Filament colours for `mica mcp`: `#RRGGBB` text to RGB, the nearest plain colour name (an
//! AI reasons better with "red" than with "#D32F2F"), and names back to hex.

/// Plain colour names with a typical filament shade each. The first match of a name wins
/// when parsing; the nearest shade wins when naming.
const NAMES: &[(&str, [u8; 3])] = &[
    ("black", [0x1A, 0x1A, 0x1A]),
    ("dark gray", [0x4A, 0x4A, 0x4A]),
    ("gray", [0x80, 0x80, 0x80]),
    ("light gray", [0xC0, 0xC0, 0xC0]),
    ("white", [0xF5, 0xF5, 0xF5]),
    ("red", [0xD3, 0x2F, 0x2F]),
    ("dark red", [0x8B, 0x1A, 0x1A]),
    ("orange", [0xFF, 0x8C, 0x00]),
    ("yellow", [0xF5, 0xD0, 0x20]),
    ("gold", [0xC9, 0xA2, 0x27]),
    ("beige", [0xE8, 0xD8, 0xB0]),
    ("brown", [0x7B, 0x4A, 0x2A]),
    ("lime", [0x9A, 0xE0, 0x3A]),
    ("green", [0x2E, 0x9D, 0x40]),
    ("dark green", [0x1B, 0x5E, 0x20]),
    ("olive", [0x80, 0x80, 0x20]),
    ("teal", [0x00, 0x80, 0x80]),
    ("cyan", [0x20, 0xC8, 0xE0]),
    ("light blue", [0x7C, 0xB8, 0xF0]),
    ("blue", [0x1E, 0x5C, 0xD8]),
    ("navy", [0x14, 0x24, 0x6E]),
    ("purple", [0x7B, 0x3F, 0xB0]),
    ("magenta", [0xD0, 0x30, 0xB0]),
    ("pink", [0xF4, 0x8F, 0xB1]),
    ("silver", [0xB8, 0xBC, 0xC4]),
    ("grey", [0x80, 0x80, 0x80]),
    ("dark grey", [0x4A, 0x4A, 0x4A]),
    ("light grey", [0xC0, 0xC0, 0xC0]),
    ("violet", [0x8E, 0x5C, 0xD8]),
    ("natural", [0xEE, 0xE8, 0xD8]),
];

/// `#RRGGBB`, `RRGGBB` or `#RRGGBBAA` (alpha ignored).
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let h = text.trim().trim_start_matches('#');
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// A colour from a name ("red", "light blue") or hex text.
pub fn parse(text: &str) -> Option<[u8; 3]> {
    parse_hex(text).or_else(|| {
        let wanted = text.trim().to_ascii_lowercase().replace(['-', '_'], " ");
        NAMES.iter().find(|(n, _)| *n == wanted).map(|(_, c)| *c)
    })
}

/// The nearest plain name (weighted RGB distance, which follows perceived difference well
/// enough for naming).
pub fn name(c: [u8; 3]) -> &'static str {
    let d = |a: [u8; 3]| {
        let [dr, dg, db] = std::array::from_fn::<f32, 3, _>(|i| f32::from(c[i]) - f32::from(a[i]));
        2. * dr * dr + 4. * dg * dg + 3. * db * db
    };
    NAMES
        .iter()
        .min_by(|a, b| d(a.1).total_cmp(&d(b.1)))
        .map_or("gray", |(n, _)| n)
}

/// The accepted names, for error messages.
pub fn known() -> String {
    let mut names: Vec<&str> = NAMES.iter().map(|(n, _)| *n).collect();
    names.dedup();
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_hex_round_trip() {
        assert_eq!(parse("#ff0000"), Some([255, 0, 0]));
        assert_eq!(parse("00FF0080"), Some([0, 255, 0]));
        assert_eq!(parse("Light-Blue"), Some([0x7C, 0xB8, 0xF0]));
        assert_eq!(parse("chartreuse-ish"), None);
        assert_eq!(parse("#12"), None);
        assert_eq!(name([250, 10, 10]), "red");
        assert_eq!(name([255, 255, 255]), "white");
        assert_eq!(name([10, 10, 10]), "black");
        assert_eq!(name([30, 90, 220]), "blue");
        assert_eq!(hex([1, 171, 255]), "#01ABFF");
        for (n, c) in NAMES {
            assert_eq!(parse(n), Some(*c), "{n}");
        }
    }
}
