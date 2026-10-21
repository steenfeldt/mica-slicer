// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica's own features that Settings › Features can turn off: the `[features]` table of
//! `mica.toml` in Mica's settings folder. The GUI and the CLI read the same table, so the same
//! settings folder gives the same G-code from both.
//!
//! Off hides the feature's controls. A project that uses a feature that is off keeps its data
//! (saving writes it back unchanged), says so, and slices without it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The settings file that holds the `[features]` table (the GUI's preferences file).
pub const FILE_NAME: &str = "mica.toml";

/// One of Mica's own features with a switch in Settings › Features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Textures,
    ColorMix,
    Mcp,
    BrickLayers,
}

impl Feature {
    pub const ALL: [Self; 4] = [Self::Textures, Self::ColorMix, Self::Mcp, Self::BrickLayers];

    /// Its key in the `[features]` table.
    pub fn key(self) -> &'static str {
        match self {
            Self::Textures => "textures",
            Self::ColorMix => "color_mix",
            Self::Mcp => "mcp",
            Self::BrickLayers => "brick_layers",
        }
    }

    /// The name Preferences and the notices use.
    pub fn title(self) -> &'static str {
        match self {
            Self::Textures => "Surface textures",
            Self::ColorMix => "ColorMix",
            Self::Mcp => "Mica MCP",
            Self::BrickLayers => "Brick layers",
        }
    }

    /// The short help text under the switch.
    pub fn description(self) -> &'static str {
        match self {
            Self::Textures => {
                "Height-map textures on the outer walls: a Texture section in the print \
                 settings, a texture gallery and Texture painting. Inspired by CNC Kitchen's \
                 BumpMesh."
            }
            Self::ColorMix => {
                "Mixed colors made from two or three filaments, layer by layer: the Mixed \
                 colors section under Filaments, mixes for objects, parts and painting. \
                 Inspired by Prusa ColorMix and prusa-fdm-mixer (MIT)."
            }
            Self::Mcp => {
                "Lets AI assistants open, slice and inspect models through `mica mcp`. Off: \
                 `mica mcp` refuses to start."
            }
            Self::BrickLayers => {
                "Every other inner wall prints half a layer higher, so the walls' layer lines \
                 interlock like bricks for stronger parts: a Brick layers group under Strength \
                 in the print settings. Inspired by GeekDetour's Brick Layers; strength tests \
                 by CNC Kitchen."
            }
        }
    }

    /// Whether the feature exists yet (its switch can be used).
    pub fn available(self) -> bool {
        true
    }

    /// Whether Settings › Features shows the switch only once the feature was turned on
    /// by hand in mica.toml (Brick layers).
    pub fn hidden(self) -> bool {
        matches!(self, Self::BrickLayers)
    }
}

/// The switches of [`Feature`]. Textures, ColorMix and Mica MCP are on unless turned off;
/// Brick layers is off unless turned on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    pub textures: bool,
    pub color_mix: bool,
    pub mcp: bool,
    pub brick_layers: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            textures: true,
            color_mix: true,
            mcp: true,
            brick_layers: false,
        }
    }
}

impl Features {
    pub fn on(&self, f: Feature) -> bool {
        match f {
            Feature::Textures => self.textures,
            Feature::ColorMix => self.color_mix,
            Feature::Mcp => self.mcp,
            Feature::BrickLayers => self.brick_layers,
        }
    }

    pub fn set(&mut self, f: Feature, on: bool) {
        match f {
            Feature::Textures => self.textures = on,
            Feature::ColorMix => self.color_mix = on,
            Feature::Mcp => self.mcp = on,
            Feature::BrickLayers => self.brick_layers = on,
        }
    }

    /// `mica mcp` may start.
    pub fn mcp_enabled(&self) -> bool {
        self.mcp
    }

    /// Brick layers may change the walls (a hidden opt-in: `brick_layers = true` by hand).
    pub fn brick_layers_enabled(&self) -> bool {
        self.brick_layers
    }

    /// `mica.toml` in Mica's settings folder (`MICA_CONFIG_DIR`, else the platform's).
    pub fn default_path() -> PathBuf {
        mica_core::user_data::config_dir().join(FILE_NAME)
    }

    /// The `[features]` table of the TOML text `text` (other tables are ignored); defaults
    /// without one.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        match table.get("features") {
            None => Ok(Self::default()),
            Some(v) => v
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| format!("[features]: {e}")),
        }
    }

    /// The switches in the file at `path`: defaults when it is missing; a broken file also
    /// gives defaults, with the reason alongside so the caller can say so.
    pub fn load_from(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(f) => (f, None),
                Err(e) => (
                    Self::default(),
                    Some(format!(
                        "{}: {e}; using the default features",
                        path.display()
                    )),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!(
                    "{}: {e}; using the default features",
                    path.display()
                )),
            ),
        }
    }

    /// The switches of Mica's settings folder ([`Self::default_path`]).
    pub fn load() -> (Self, Option<String>) {
        Self::load_from(&Self::default_path())
    }
}

/// Whether `mica mcp` may start with these switches.
pub fn mcp_enabled(features: &Features) -> bool {
    features.mcp_enabled()
}

/// Whether Brick layers may change a print with these switches.
pub fn brick_layers_enabled(features: &Features) -> bool {
    features.brick_layers_enabled()
}

/// What `mica mcp` says when Mica MCP is off.
pub fn mcp_off_message(path: &Path) -> String {
    format!(
        "Mica MCP is turned off in Settings › Features. Turn it on there, or set \
         `mcp = true` under [features] in {}",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_without_a_table() {
        let f = Features::from_toml("theme = \"dark\"\n").unwrap();
        assert_eq!(f, Features::default());
        assert!(f.textures && f.color_mix && f.mcp && !f.brick_layers);
        assert!(f.mcp_enabled() && !f.brick_layers_enabled());
    }

    #[test]
    fn reads_the_table_and_keeps_missing_switches_at_their_defaults() {
        let f = Features::from_toml(
            "auto_slice = true\n[features]\ncolor_mix = false\nmcp = false\n\
             [window]\nwidth = 3.0\n",
        )
        .unwrap();
        assert!(f.textures && !f.color_mix && !f.mcp && !f.brick_layers);
        assert!(!mcp_enabled(&f));
        let f = Features::from_toml("[features]\nbrick_layers = true\n").unwrap();
        assert!(brick_layers_enabled(&f) && f.textures && f.color_mix && f.mcp);
        assert!(Features::from_toml("[features]\nmcp = 3\n").is_err());
    }

    #[test]
    fn on_and_set_agree_with_the_keys() {
        let mut f = Features::default();
        for feature in Feature::ALL {
            let on = f.on(feature);
            f.set(feature, !on);
            assert_eq!(f.on(feature), !on);
            let text = format!("[features]\n{} = {}\n", feature.key(), !on);
            assert_eq!(Features::from_toml(&text).unwrap().on(feature), !on);
        }
        assert!(Feature::BrickLayers.available() && Feature::BrickLayers.hidden());
        assert!(!Feature::ColorMix.hidden());
    }

    #[test]
    fn a_missing_or_broken_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("mica-features-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (f, e) = Features::load_from(&dir.join("absent.toml"));
        assert_eq!((f, e), (Features::default(), None));
        let broken = dir.join("broken.toml");
        std::fs::write(&broken, "[features\n").unwrap();
        let (f, e) = Features::load_from(&broken);
        assert_eq!(f, Features::default());
        assert!(e.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
