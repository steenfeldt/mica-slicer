// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Brick layers in the app: a hidden opt-in. Settings › Features shows its switch, and the
//! print settings its group, only once `brick_layers = true` is set by hand under `[features]`
//! in mica.toml. Otherwise every plate slices without it, as `mica slice` does.

use std::collections::BTreeMap;

/// The group of the brick-layers settings on the Strength page.
pub const GROUP: &str = "Brick layers";

/// Whether these objects or the configuration turn brick layers on.
pub fn objects_use_brick_layers(
    objects: &[crate::scene::Object],
    config: &mica_core::config::Config,
) -> bool {
    let on = |m: &BTreeMap<String, String>| {
        m.get("mica_brick_layers")
            .is_some_and(|v| v == "1" || v == "true")
    };
    mica_core::brick_layers::enabled_in(config)
        || objects.iter().any(|o| {
            on(&o.overrides)
                || o.volumes.iter().any(|v| on(&v.overrides))
                || o.height_ranges.iter().any(|r| on(&r.overrides))
                || o.region
                    .as_deref()
                    .is_some_and(mica_core::features::region_uses_brick_layers)
        })
}

/// The objects without brick-layers settings (the feature is off), as `mica slice` strips them.
pub fn without_brick_layers(mut objects: Vec<crate::scene::Object>) -> Vec<crate::scene::Object> {
    let strip =
        |m: &mut BTreeMap<String, String>| m.retain(|k, _| !k.starts_with("mica_brick_layers"));
    for o in &mut objects {
        strip(&mut o.overrides);
        for v in &mut o.volumes {
            strip(&mut v.overrides);
        }
        for r in &mut o.height_ranges {
            strip(&mut r.overrides);
        }
        if let Some(r) = &mut o.region
            && mica_core::features::region_uses_brick_layers(r)
        {
            *r = crate::scene::derived_region(
                crate::scene::Derived::WithoutBrickLayers,
                r,
                mica_core::features::region_without_brick_layers,
            );
        }
    }
    objects
}

/// Whether `key` is a brick-layers option (hidden with the feature off).
pub fn is_option(key: &str) -> bool {
    key.starts_with("mica_brick_layers")
}
