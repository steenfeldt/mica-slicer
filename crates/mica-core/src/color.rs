// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Hex color text as `filament_colour` and `extruder_colour` hold it: `#RRGGBB`, or
//! `#RRGGBBAA` with an alpha byte (Bambu Studio and OrcaSlicer projects store translucent
//! filaments that way). The `#` is optional and the digits are case-insensitive; surrounding
//! whitespace is ignored. Anything else (names, other lengths, non-hex digits) is `None`, so
//! callers can fall back to their own reading of the text.

/// The hex digits of `text` (6 or 8 ASCII hex digits after an optional `#`), if it is one.
fn digits(text: &str) -> Option<&str> {
    let t = text.trim();
    let h = t.strip_prefix('#').unwrap_or(t);
    ((h.len() == 6 || h.len() == 8) && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

/// `[r, g, b, a]` of `#RRGGBB` / `RRGGBB` (alpha 255) or `#RRGGBBAA` / `RRGGBBAA`.
pub fn parse_hex_rgba(text: &str) -> Option<[u8; 4]> {
    let h = digits(text)?;
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if h.len() == 8 { byte(6)? } else { 255 },
    ])
}

/// `[r, g, b]` of the same forms as [`parse_hex_rgba`] (any alpha dropped).
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    parse_hex_rgba(text).map(|[r, g, b, _]| [r, g, b])
}

/// [`parse_hex`] scaled to 0..1.
pub fn parse_hex_f32(text: &str) -> Option<[f32; 3]> {
    parse_hex(text).map(|c| c.map(|v| f32::from(v) / 255.))
}

/// Whether `text` is a color with an alpha byte (the 8-digit form).
pub fn has_alpha(text: &str) -> bool {
    digits(text).is_some_and(|h| h.len() == 8)
}

/// The text a project keeps for a color: the 6-digit form as `#RRGGBB` (uppercase, as Mica
/// always wrote it), the 8-digit form exactly as written (trimmed), so a translucent filament's
/// color survives import and save byte for byte, as in OrcaSlicer (which keeps the string).
pub fn project_text(text: &str) -> Option<String> {
    if has_alpha(text) {
        Some(text.trim().to_string())
    } else {
        parse_hex(text).map(to_hex)
    }
}

/// `#RRGGBB`, uppercase.
pub fn to_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// `#RRGGBBAA`, uppercase.
pub fn to_hex_rgba(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_forms() {
        assert_eq!(parse_hex_rgba("#B50011"), Some([0xB5, 0x00, 0x11, 255]));
        assert_eq!(parse_hex_rgba("B50011"), Some([0xB5, 0x00, 0x11, 255]));
        assert_eq!(parse_hex_rgba("#B5001180"), Some([0xB5, 0x00, 0x11, 0x80]));
        assert_eq!(parse_hex_rgba("B5001180"), Some([0xB5, 0x00, 0x11, 0x80]));
        assert_eq!(parse_hex("#B5001180"), Some([0xB5, 0x00, 0x11]));
        assert_eq!(parse_hex(" #29b2b2 \n"), Some([0x29, 0xB2, 0xB2]));
        assert_eq!(parse_hex_f32("#FF8000"), Some([1., 128. / 255., 0.]));
    }

    #[test]
    fn case_does_not_matter() {
        assert_eq!(parse_hex_rgba("#aBcDeF0a"), parse_hex_rgba("#ABCDEF0A"));
        assert_eq!(parse_hex("29b2b2"), Some([0x29, 0xB2, 0xB2]));
    }

    #[test]
    fn junk_is_none() {
        for junk in [
            "",
            "#",
            "red",
            "#F8F",
            "#FFFFF",
            "#FFFFFFF",
            "#FFFFFFFFF",
            "#GGGGGG",
            "##FF0000",
            "#FF 000",
            "#+12345",
            "0x1234",
            "#ÄÄÄ",
            "#12345é",
        ] {
            assert_eq!(parse_hex_rgba(junk), None, "{junk:?}");
            assert!(!has_alpha(junk), "{junk:?}");
        }
    }

    #[test]
    fn alpha_form_and_encoding() {
        assert!(has_alpha("#B5001180"));
        assert!(has_alpha("b5001180"));
        assert!(!has_alpha("#B50011"));
        assert_eq!(to_hex([0xB5, 0, 0x11]), "#B50011");
        assert_eq!(project_text(" #b5001180 ").as_deref(), Some("#b5001180"));
        assert_eq!(project_text("b50011").as_deref(), Some("#B50011"));
        assert_eq!(project_text("red"), None);
        assert_eq!(to_hex_rgba([0xB5, 0, 0x11, 0x80]), "#B5001180");
        assert_eq!(
            parse_hex_rgba(&to_hex_rgba([1, 2, 3, 4])),
            Some([1, 2, 3, 4])
        );
    }
}
