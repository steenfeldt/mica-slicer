// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The textures that ship with Mica and who made them. Images are 8-bit grey PNGs, at most
//! 512 px, resized from BumpMesh's maps (see THIRD_PARTY.md, "Surface textures").

/// Where a bundled texture comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credit {
    /// Public domain (CC0 1.0); the asset page the map was made from.
    Cc0(&'static str),
    /// Hero Patterns by Steve Schoger, CC BY 4.0: attribution required.
    HeroPatterns,
}

/// Gallery groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Procedural,
    Cc0,
    Patterns,
    Custom,
}
impl Category {
    pub const ALL: [Self; 4] = [Self::Procedural, Self::Cc0, Self::Patterns, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Procedural => "Procedural",
            Self::Cc0 => "CC0",
            Self::Patterns => "Patterns",
            Self::Custom => "Custom",
        }
    }
}

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub credit: Credit,
    /// Suggested tile size, mm.
    pub tile: f64,
    /// The map's file name in BumpMesh's `textures/` folder.
    pub origin: &'static str,
    pub png: &'static [u8],
}
impl Builtin {
    pub fn category(&self) -> Category {
        match self.credit {
            Credit::Cc0(_) => Category::Cc0,
            Credit::HeroPatterns => Category::Patterns,
        }
    }
    /// One line for the gallery and the credits page.
    pub fn credit_line(&self) -> String {
        match self.credit {
            Credit::Cc0(source) => format!("CC0 (public domain), from {source}"),
            Credit::HeroPatterns => HERO_PATTERNS_LINE.into(),
        }
    }
}

pub const HERO_PATTERNS_LINE: &str = "Based on Hero Patterns by Steve Schoger (heropatterns.com), CC BY 4.0; made into a height map by CNC Kitchen for BumpMesh";

macro_rules! builtin {
    ($id:literal, $name:literal, $credit:expr, $tile:literal, $origin:literal) => {
        Builtin {
            id: $id,
            name: $name,
            credit: $credit,
            tile: $tile,
            origin: $origin,
            png: include_bytes!(concat!("../../assets/textures/", $id, ".png")),
        }
    };
}

pub const BUILTINS: &[Builtin] = &[
    builtin!(
        "autumn",
        "Autumn",
        Credit::HeroPatterns,
        15.0,
        "hero-autumn.png"
    ),
    builtin!(
        "aztec",
        "Aztec",
        Credit::HeroPatterns,
        10.0,
        "hero-aztec.png"
    ),
    builtin!(
        "bamboo",
        "Bamboo",
        Credit::HeroPatterns,
        5.0,
        "hero-bamboo.png"
    ),
    builtin!(
        "bank_note",
        "Bank Note",
        Credit::HeroPatterns,
        16.0,
        "hero-bank-note.png"
    ),
    builtin!(
        "bark",
        "Bark",
        Credit::Cc0("https://ambientcg.com/view?id=Bark001"),
        10.0,
        "bark.png"
    ),
    builtin!(
        "bubbles",
        "Bubbles",
        Credit::HeroPatterns,
        16.0,
        "hero-bubbles.png"
    ),
    builtin!("cage", "Cage", Credit::HeroPatterns, 5.0, "hero-cage.png"),
    builtin!(
        "connections",
        "Connections",
        Credit::HeroPatterns,
        6.0,
        "hero-connections.png"
    ),
    builtin!(
        "current",
        "Current",
        Credit::HeroPatterns,
        12.0,
        "hero-current.png"
    ),
    builtin!(
        "curtain",
        "Curtain",
        Credit::HeroPatterns,
        8.0,
        "hero-curtain.png"
    ),
    builtin!(
        "death_star",
        "Death Star",
        Credit::HeroPatterns,
        20.0,
        "hero-death-star.png"
    ),
    builtin!(
        "endless_clouds",
        "Endless Clouds",
        Credit::HeroPatterns,
        9.0,
        "hero-endless-clouds.png"
    ),
    builtin!("eyes", "Eyes", Credit::HeroPatterns, 6.0, "hero-eyes.png"),
    builtin!(
        "falling_triangles",
        "Falling Triangles",
        Credit::HeroPatterns,
        11.0,
        "hero-falling-triangles.png"
    ),
    builtin!(
        "flipped_diamonds",
        "Flipped Diamonds",
        Credit::HeroPatterns,
        10.0,
        "hero-flipped-diamonds.png"
    ),
    builtin!(
        "floating_cogs",
        "Floating Cogs",
        Credit::HeroPatterns,
        20.0,
        "hero-floating-cogs.png"
    ),
    builtin!(
        "formal_invitation",
        "Formal Invitation",
        Credit::HeroPatterns,
        17.0,
        "hero-formal-invitation.png"
    ),
    builtin!(
        "four_point_stars",
        "Four Point Stars",
        Credit::HeroPatterns,
        4.0,
        "hero-four-point-stars.png"
    ),
    builtin!(
        "groovy",
        "Groovy",
        Credit::HeroPatterns,
        12.0,
        "hero-groovy.png"
    ),
    builtin!(
        "happy_intersection",
        "Happy Intersection",
        Credit::HeroPatterns,
        14.0,
        "hero-happy-intersection.png"
    ),
    builtin!(
        "hexagon_outline",
        "Hexagon Outline",
        Credit::HeroPatterns,
        9.0,
        "hero-hexagons.png"
    ),
    builtin!(
        "intersecting_circles",
        "Intersecting Circles",
        Credit::HeroPatterns,
        5.0,
        "hero-intersecting-circles.png"
    ),
    builtin!(
        "leaves",
        "Leaves",
        Credit::Cc0("https://ambientcg.com/view?id=ScatteredLeaves007"),
        10.0,
        "leaves.png"
    ),
    builtin!("lips", "Lips", Credit::HeroPatterns, 18.0, "hero-lips.png"),
    builtin!(
        "lisbon",
        "Lisbon",
        Credit::HeroPatterns,
        12.0,
        "hero-lisbon.png"
    ),
    builtin!("melt", "Melt", Credit::HeroPatterns, 4.0, "hero-melt.png"),
    builtin!(
        "pine_bark",
        "Pine Bark",
        Credit::Cc0("https://polyhaven.com/a/pine_bark"),
        15.0,
        "pineBark.png"
    ),
    builtin!("rain", "Rain", Credit::HeroPatterns, 8.0, "hero-rain.png"),
    builtin!(
        "roof_tiles",
        "Roof Tiles",
        Credit::Cc0("https://ambientcg.com/view?id=RoofingTiles002"),
        10.0,
        "roofTiles.png"
    ),
    builtin!(
        "roof_tiles_2",
        "Roof Tiles 2",
        Credit::Cc0("https://ambientcg.com/view?id=RoofingTiles004"),
        15.0,
        "roofTiles_02.png"
    ),
    builtin!(
        "shingles",
        "Shingles",
        Credit::Cc0("https://ambientcg.com/view?id=RoofingTiles001"),
        10.0,
        "shingles.png"
    ),
    builtin!(
        "signal",
        "Signal",
        Credit::HeroPatterns,
        15.0,
        "hero-signal.png"
    ),
    builtin!(
        "speckle",
        "Speckle",
        Credit::HeroPatterns,
        2.0,
        "hero-texture.png"
    ),
    builtin!(
        "straw",
        "Straw",
        Credit::Cc0("https://polyhaven.com/a/thatch_roof_angled"),
        10.0,
        "straw.png"
    ),
    builtin!(
        "tiny_checkers",
        "Tiny Checkers",
        Credit::HeroPatterns,
        2.0,
        "hero-tiny-checkers.png"
    ),
    builtin!(
        "twill",
        "Twill",
        Credit::Cc0("https://ambientcg.com/view?id=Fabric004"),
        15.0,
        "twill.png"
    ),
    builtin!(
        "twill_2",
        "Twill 2",
        Credit::Cc0("https://ambientcg.com/view?id=Fabric015"),
        20.0,
        "twill_02.png"
    ),
    builtin!(
        "wicker",
        "Wicker",
        Credit::Cc0("https://ambientcg.com/view?id=Wicker001"),
        20.0,
        "wicker.png"
    ),
    builtin!(
        "wicker_2",
        "Wicker 2",
        Credit::Cc0("https://ambientcg.com/view?id=Wicker009A"),
        10.0,
        "wicker_02.png"
    ),
    builtin!("yyy", "YYY", Credit::HeroPatterns, 20.0, "hero-yyy.png"),
];

pub fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// One entry of the app's credits page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditEntry {
    pub title: String,
    pub detail: String,
    pub license: &'static str,
    pub link: &'static str,
}

/// Credits for surface textures: the feature's inspiration and every bundled map that asks for
/// (or deserves) attribution.
pub fn credits() -> Vec<CreditEntry> {
    let mut out = vec![
        CreditEntry {
            title: "Surface textures".into(),
            detail: "Inspired by CNC Kitchen's BumpMesh (Stefan Hermann and contributors); Mica's texture projections follow BumpMesh's.".into(),
            license: "AGPL-3.0",
            link: "https://github.com/CNCKitchen/stlTexturizer",
        },
        CreditEntry {
            title: "Hero Patterns".into(),
            detail: format!(
                "Steve Schoger. Used for: {}. Made into height maps by CNC Kitchen for BumpMesh and resized for Mica.",
                BUILTINS
                    .iter()
                    .filter(|b| b.credit == Credit::HeroPatterns)
                    .map(|b| b.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            license: "CC BY 4.0",
            link: "https://heropatterns.com",
        },
    ];
    for b in BUILTINS {
        if let Credit::Cc0(source) = b.credit {
            out.push(CreditEntry {
                title: format!("Texture “{}”", b.name),
                detail: format!("From {source}, via BumpMesh; resized for Mica."),
                license: "CC0 1.0",
                link: source,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_maps_decode_and_are_small() {
        assert_eq!(BUILTINS.len(), 40);
        assert_eq!(
            BUILTINS
                .iter()
                .filter(|b| b.category() == Category::Cc0)
                .count(),
            11
        );
        for b in BUILTINS {
            let m = super::super::heightmap::HeightMap::from_png(b.png).unwrap();
            assert!(m.width().max(m.height()) <= 512, "{}", b.id);
            assert!(m.width().max(m.height()) >= 400, "{}", b.id);
        }
    }

    #[test]
    fn every_hero_pattern_is_credited() {
        let credits = credits();
        for b in BUILTINS.iter().filter(|b| b.credit == Credit::HeroPatterns) {
            assert!(credits[1].detail.contains(b.name));
        }
        assert!(credits.iter().any(|c| c.license == "CC BY 4.0"));
    }
}
