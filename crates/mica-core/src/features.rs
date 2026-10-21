// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Slicing without a feature that Settings › Features turned off (`[features]` in
//! mica.toml): the CLI applies these to what it loaded, the GUI to its scene the same way, so
//! both write the same G-code. The project's own data is never changed by them; they work on
//! the copies a slice takes.
//!
//! - **Textures off**: no `mica_texture*` setting and no texture paint reaches the slicer
//!   (every texture option at its default).
//! - **ColorMix off**: every mixed color prints in its main color alone
//!   ([`FullSpectrum::main_colors_only`]), so parts and paint on a mix keep a defined tool.
//! - **Brick layers off** (the default): no `mica_brick_layers*` setting reaches the slicer
//!   (every brick-layers option at its default, which is off).

use crate::config::Config;
use crate::full_spectrum::FullSpectrum;
use crate::input::InputObject;
use crate::regions::RegionModel;

/// The prefix of every texture option (`mica_texture_skin*`, the paint slots).
const TEXTURE_PREFIX: &str = "mica_texture";

fn texture_keys(c: &Config) -> Vec<String> {
    c.values()
        .map(|(k, _)| k)
        .chain(c.extras().map(|(k, _)| k))
        .filter(|k| k.starts_with(TEXTURE_PREFIX))
        .map(str::to_string)
        .collect()
}

fn texture_on(c: &Config) -> bool {
    c.raw("mica_texture_skin").is_some_and(|v| v != "none")
}

/// Whether a regions model has texture settings or texture paint.
pub fn region_uses_textures(model: &RegionModel) -> bool {
    model
        .volumes
        .iter()
        .any(|v| texture_on(&v.config) || !v.painting.texture.is_empty())
        || model.height_ranges.iter().any(|h| texture_on(&h.config))
}

/// Whether the configuration or these objects use surface textures.
pub fn uses_textures(config: &Config, objects: &[InputObject]) -> bool {
    texture_on(config)
        || objects
            .iter()
            .any(|o| texture_on(&o.config) || o.region.as_ref().is_some_and(region_uses_textures))
}

/// A configuration without textures: every texture option at its default.
pub fn config_without_textures(c: &mut Config) {
    for k in crate::texture::KEYS {
        if let Some(d) = crate::config::option_defs().get(k) {
            let _ = c.set_raw(k, d.default);
        }
    }
}

/// A regions model without texture settings or texture paint (its configurations are sparse
/// overrides, so the texture options are dropped).
pub fn region_without_textures(model: &mut RegionModel) {
    for v in &mut model.volumes {
        for k in texture_keys(&v.config) {
            v.config.remove(&k);
        }
        v.painting.texture = Default::default();
    }
    for h in &mut model.height_ranges {
        for k in texture_keys(&h.config) {
            h.config.remove(&k);
        }
    }
}

/// The configuration and objects of a slice without textures. `config` is the print's
/// configuration; each object's own configuration (that one with the object's settings
/// applied) takes the texture options of the stripped `config`, as if the object never had
/// texture settings.
pub fn without_textures(config: &mut Config, objects: &mut [InputObject]) {
    config_without_textures(config);
    for o in objects {
        for k in texture_keys(&o.config) {
            match config.get(&k) {
                Some(v) => o.config.set(&k, v.clone()),
                None => {
                    o.config.remove(&k);
                }
            }
        }
        for k in crate::texture::KEYS {
            if let Some(v) = config.get(k) {
                o.config.set(k, v.clone());
            }
        }
        o.overrides.retain(|k, _| !k.starts_with(TEXTURE_PREFIX));
        if let Some(model) = &mut o.region {
            region_without_textures(model);
        }
    }
}

/// The prefix of every brick-layers option.
const BRICK_PREFIX: &str = "mica_brick_layers";

fn brick_keys(c: &Config) -> Vec<String> {
    c.values()
        .map(|(k, _)| k)
        .chain(c.extras().map(|(k, _)| k))
        .filter(|k| k.starts_with(BRICK_PREFIX))
        .map(str::to_string)
        .collect()
}

/// Whether the configuration or these objects (their settings, modifiers and height ranges)
/// turn brick layers on.
pub fn uses_brick_layers(config: &Config, objects: &[InputObject]) -> bool {
    let on = crate::brick_layers::enabled_in;
    on(config)
        || objects
            .iter()
            .any(|o| on(&o.config) || o.region.as_ref().is_some_and(region_uses_brick_layers))
}

/// A configuration without brick layers: every brick-layers option at its default (off).
pub fn config_without_brick_layers(c: &mut Config) {
    for k in crate::brick_layers::KEYS {
        if let Some(d) = crate::config::option_defs().get(k) {
            let _ = c.set_raw(k, d.default);
        }
    }
}

/// The configuration and objects of a slice without brick layers (as [`without_textures`]).
pub fn without_brick_layers(config: &mut Config, objects: &mut [InputObject]) {
    config_without_brick_layers(config);
    for o in objects {
        for k in brick_keys(&o.config) {
            match config.get(&k) {
                Some(v) => o.config.set(&k, v.clone()),
                None => {
                    o.config.remove(&k);
                }
            }
        }
        o.overrides.retain(|k, _| !k.starts_with(BRICK_PREFIX));
        if let Some(model) = &mut o.region {
            region_without_brick_layers(model);
        }
    }
}

/// Whether a regions model's parts or height ranges turn brick layers on.
pub fn region_uses_brick_layers(model: &RegionModel) -> bool {
    let on = crate::brick_layers::enabled_in;
    model.volumes.iter().any(|v| on(&v.config)) || model.height_ranges.iter().any(|h| on(&h.config))
}

/// A regions model without brick-layers options (sparse overrides: they are dropped).
pub fn region_without_brick_layers(model: &mut RegionModel) {
    for v in &mut model.volumes {
        for k in brick_keys(&v.config) {
            v.config.remove(&k);
        }
    }
    for h in &mut model.height_ranges {
        for k in brick_keys(&h.config) {
            h.config.remove(&k);
        }
    }
}

/// Whether these objects use mixed colors.
pub fn uses_color_mix(objects: &[InputObject]) -> bool {
    objects.iter().any(|o| {
        o.region
            .as_ref()
            .is_some_and(|r| !r.full_spectrum.extruders().is_empty())
    })
}

/// The objects of a slice without mixed colors: each mix prints in its main color, and an
/// object's layer-0 tools (assigned to a mix in its configuration) follow.
pub fn without_color_mix(objects: &mut [InputObject]) {
    const ROLES: [&str; 5] = [
        "perimeter_extruder",
        "infill_extruder",
        "solid_infill_extruder",
        "support_material_extruder",
        "support_material_interface_extruder",
    ];
    for o in objects {
        let Some(model) = &mut o.region else {
            continue;
        };
        if model.full_spectrum.extruders().is_empty() {
            continue;
        }
        let solid: FullSpectrum = model.full_spectrum.main_colors_only();
        for (role, key) in ROLES.into_iter().enumerate() {
            if let Some(id) = o.config.int(key).filter(|t| *t > 0).map(|t| t as usize)
                && solid.contains(id)
            {
                o.tools.0[role] = Some(solid.resolve(id, 0) - 1);
            }
        }
        model.full_spectrum = solid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::full_spectrum::{Component, Pattern, VirtualExtruder};

    fn mix(id: usize, parts: &[(usize, f64)]) -> VirtualExtruder {
        VirtualExtruder {
            id,
            color: None,
            components: parts
                .iter()
                .map(|&(extruder, ratio)| Component { extruder, ratio })
                .collect(),
            name: None,
            pattern: Pattern::Layers,
            end_ratios: vec![],
        }
    }

    #[test]
    fn main_colors_only_prints_each_mix_in_its_largest_share() {
        let s = FullSpectrum::new(
            vec![
                mix(3, &[(1, 0.25), (2, 0.75)]),
                mix(4, &[(1, 0.5), (2, 0.5)]),
            ],
            2,
        )
        .unwrap();
        let solid = s.main_colors_only();
        for layer in 0..8 {
            assert_eq!(solid.resolve(3, layer), 2);
            // A tie goes to the first component.
            assert_eq!(solid.resolve(4, layer), 1);
        }
        assert_eq!(solid.expand(3), vec![2]);
        assert!(solid.contains(3) && solid.contains(4));
        // The original keeps mixing.
        assert!((0..4).any(|l| s.resolve(3, l) == 1));
    }

    #[test]
    fn brick_layers_are_stripped_to_their_defaults() {
        let mut c = Config::defaults();
        assert!(!uses_brick_layers(&c, &[]));
        c.set_raw("mica_brick_layers", "1").unwrap();
        c.set_raw("mica_brick_layers_flow_ratio", "110%").unwrap();
        c.set_raw("perimeters", "4").unwrap();
        assert!(uses_brick_layers(&c, &[]));
        config_without_brick_layers(&mut c);
        assert!(!uses_brick_layers(&c, &[]));
        assert_eq!(
            c.raw("mica_brick_layers_flow_ratio").as_deref(),
            Some("105%")
        );
        assert_eq!(c.int("perimeters"), Some(4));
    }

    #[test]
    fn texture_options_leave_sparse_configs() {
        let mut c = Config::new();
        c.set_raw("mica_texture_skin", "external").unwrap();
        c.set_raw("perimeters", "3").unwrap();
        let mut model = RegionModel {
            volumes: vec![crate::regions::Volume {
                mesh: Default::default(),
                painting: Default::default(),
                kind: crate::threemf::PartKind::Model,
                config: c.clone(),
            }],
            ..Default::default()
        };
        region_without_textures(&mut model);
        assert!(model.volumes[0].config.get("mica_texture_skin").is_none());
        assert_eq!(model.volumes[0].config.int("perimeters"), Some(3));
        let mut base = Config::defaults();
        base.set_raw("mica_texture_skin", "external").unwrap();
        assert!(uses_textures(&base, &[]));
        config_without_textures(&mut base);
        assert!(!uses_textures(&base, &[]));
    }
}
