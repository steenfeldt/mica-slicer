// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Native option text and its mapped baseline, carried through settings composition. The
//! exporter can preserve a source value until an editor changes its canonical equivalent.
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeConfig {
    pub options: BTreeMap<String, String>,
    pub baseline: BTreeMap<String, String>,
    /// Distinguish a later typed edit from a stale raw twin (and vice versa).
    pub typed_baseline: BTreeMap<String, String>,
}

/// The text settings setters and native template options use. Known canonical keys
/// are typed; unknown native keys can have a later raw edit (tower placement, IDs).
pub fn current(config: &super::Config) -> BTreeMap<String, String> {
    let mut values: BTreeMap<_, _> = config
        .values()
        .map(|(k, v)| (k.into(), super::serialize_value(v)))
        .collect();
    for (k, v) in config.extras() {
        if super::option_defs().contains_key(k) && values.contains_key(k) {
            continue;
        }
        let typed_changed = config
            .native
            .typed_baseline
            .get(k)
            .is_some_and(|before| values.get(k) != Some(before));
        let raw_changed = config
            .native
            .baseline
            .get(k)
            .is_some_and(|before| before != v);
        if !typed_changed || raw_changed || !values.contains_key(k) {
            values.insert(k.into(), v.into());
        }
    }
    values
}
