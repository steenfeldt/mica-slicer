// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Plain-language filament colors: whether two colors look the same (CIEDE2000 below a
//! threshold, on the DeltaE00 port in `u1_mapping`) and the nearest name from a small
//! curated palette, so user-visible text never shows hex codes.

use crate::u1_mapping::{delta_e00, rgb_to_lab};

/// Two filament colors closer than this (CIEDE2000) count as the same spool color.
///
/// ΔE00 ≈ 2 is the classic "just noticeable" step, but filament colors are reported loosely:
/// the printer's RFID or picker says `#FFFFFF` for a spool the project calls `#F5F5F5`
/// (ΔE 2.0), `#6F4C2F` brown for `#7B4A2A` (4.2) and `#000000` for `#1A1A1A` (5.5, the
/// dark end of L* where CIEDE2000 is most sensitive). A different spool is above 6: white
/// against light gray `#E0E0E0` is 6.5, black against charcoal `#262626` 9.3, yellow
/// `#F8F81C` against golden `#F5D020` 11.5, red against dark red 23.8.
pub const SAME_COLOUR_DELTA_E: f64 = 6.0;

/// CIEDE2000 between two sRGB colors.
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    delta_e00(rgb_to_lab(a), rgb_to_lab(b))
}

/// Whether two filament colors look the same to a person holding the spools.
pub fn same_colour(a: [u8; 3], b: [u8; 3]) -> bool {
    delta_e(a, b) < SAME_COLOUR_DELTA_E
}

/// The curated palette: a name and a typical filament color for it.
const PALETTE: &[(&str, [u8; 3])] = &[
    ("white", [0xFF, 0xFF, 0xFF]),
    ("off-white", [0xEE, 0xEA, 0xDC]),
    ("light gray", [0xC8, 0xC8, 0xC8]),
    ("silver", [0xA8, 0xAD, 0xB4]),
    ("gray", [0x80, 0x80, 0x80]),
    ("dark gray", [0x45, 0x45, 0x45]),
    ("black", [0x00, 0x00, 0x00]),
    ("red", [0xD8, 0x1E, 0x1E]),
    ("dark red", [0x8B, 0x10, 0x10]),
    ("orange", [0xFF, 0x80, 0x00]),
    ("yellow", [0xFF, 0xF0, 0x10]),
    ("gold", [0xE8, 0xBC, 0x10]),
    ("lime", [0xA0, 0xE0, 0x20]),
    ("green", [0x20, 0xA0, 0x40]),
    ("dark green", [0x0B, 0x50, 0x20]),
    ("olive", [0x70, 0x70, 0x20]),
    ("teal", [0x00, 0x80, 0x80]),
    ("cyan", [0x00, 0xC8, 0xE0]),
    ("light blue", [0x8E, 0xC8, 0xF0]),
    ("blue", [0x20, 0x40, 0xF0]),
    ("navy", [0x10, 0x20, 0x50]),
    ("purple", [0x6A, 0x2C, 0x91]),
    ("violet", [0x9A, 0x6F, 0xD8]),
    ("magenta", [0xE0, 0x10, 0x9A]),
    ("pink", [0xF7, 0xA1, 0xC4]),
    ("brown", [0x6B, 0x44, 0x23]),
    ("tan", [0xC8, 0xA4, 0x72]),
    ("beige", [0xE8, 0xD9, 0xB5]),
];

/// The nearest palette name (by CIEDE2000).
pub fn colour_name(c: [u8; 3]) -> &'static str {
    let lab = rgb_to_lab(c);
    PALETTE
        .iter()
        .map(|(name, rgb)| (delta_e00(lab, rgb_to_lab(*rgb)), *name))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or("unknown", |(_, n)| n)
}

/// Names for two colors that differ, telling them apart even when both land on the same
/// palette entry (`"lighter yellow"` / `"darker yellow"`).
pub fn contrast_names(a: [u8; 3], b: [u8; 3]) -> (String, String) {
    let (na, nb) = (colour_name(a), colour_name(b));
    if na != nb {
        return (na.into(), nb.into());
    }
    let (la, lb) = (rgb_to_lab(a)[0], rgb_to_lab(b)[0]);
    if (la - lb).abs() >= 3. {
        let (wa, wb) = if la > lb {
            ("lighter", "darker")
        } else {
            ("darker", "lighter")
        };
        (format!("{wa} {na}"), format!("{wb} {nb}"))
    } else {
        (format!("{na} (one shade)"), format!("{nb} (another shade)"))
    }
}

/// `#RRGGBB`, for tooltips only.
pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::u1_mapping::parse_hex;

    fn h(s: &str) -> [u8; 3] {
        parse_hex(s).unwrap()
    }

    /// A U1 with four loaded spools against its project: white, brown and black are the same
    /// spool colors described a little differently; only yellow against gold is a real difference.
    #[test]
    fn four_loaded_spools() {
        assert!(!same_colour(h("F8F81C"), h("F5D020")));
        assert!(same_colour(h("FFFFFF"), h("F5F5F5")));
        assert!(same_colour(h("6F4C2F"), h("7B4A2A")));
        assert!(same_colour(h("000000"), h("1A1A1A")));
        assert_eq!(colour_name(h("F8F81C")), "yellow");
        assert_eq!(colour_name(h("F5D020")), "gold");
        assert_eq!(colour_name(h("FFFFFF")), "white");
        assert_eq!(colour_name(h("6F4C2F")), "brown");
        assert_eq!(colour_name(h("7B4A2A")), "brown");
        assert_eq!(colour_name(h("000000")), "black");
        assert_eq!(colour_name(h("1A1A1A")), "black");
    }

    #[test]
    fn real_differences_are_flagged() {
        for (a, b) in [
            ("FF0000", "8B0000"),
            ("FF8000", "F5D020"),
            ("1E5AD0", "102050"),
            ("FFFFFF", "E0E0E0"),
            ("000000", "262626"),
            ("000000", "404040"),
            ("20A040", "00C8E0"),
        ] {
            assert!(
                !same_colour(h(a), h(b)),
                "{a} vs {b}: {}",
                delta_e(h(a), h(b))
            );
        }
    }

    #[test]
    fn common_filament_colors_get_sensible_names() {
        for (hex, name) in [
            ("FF0000", "red"),
            ("FF8C00", "orange"),
            ("FFA500", "orange"),
            ("00FF00", "lime"),
            ("008000", "green"),
            ("0000FF", "blue"),
            ("000080", "navy"),
            ("808080", "gray"),
            ("F5F5F5", "white"),
            ("26A69A", "teal"),
            ("FF69B4", "pink"),
            ("800080", "purple"),
            ("FF00FF", "magenta"),
            ("8B4513", "brown"),
            ("EAD7B0", "beige"),
        ] {
            assert_eq!(colour_name(h(hex)), name, "{hex}");
        }
    }

    #[test]
    fn same_name_pairs_still_read_differently() {
        let (a, b) = contrast_names(h("FFFF00"), h("C8C000"));
        assert_ne!(a, b);
        let (a, b) = contrast_names(h("F8F81C"), h("F5D020"));
        assert_eq!((a.as_str(), b.as_str()), ("yellow", "gold"));
    }
}
