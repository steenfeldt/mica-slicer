// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::reorder_extruders, ToolOrdering::collect_extruder_statistics
// Source: orca@8500fcdcca src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::fill_wipe_tower_partitions, calc_max_layer_height
// Changes: Zero-based Rust tools, None for don't-care support; compact layer lists. Wiping hooks reserved.
// Changes: The extruder a colour change needs prints first on its layer (reorder_extruders, extruder_needed_for_color_changer).
// Changes: With a wipe tower the first layer starts with its first soluble tool (reorder_extruders).
// Changes: `has_object` and `tower_layers` (which layers carry the wipe tower) from fill_wipe_tower_partitions; the raft gap layer is the caller's filler layer.

// Copyright (c) Prusa Research 2017–2023 Lukáš Matěna, Vojtěch Bubník, Oleksandra Iushchenko, Enrico Turri, Tomáš Mészáros.
// Copyright (c) SuperSlicer 2023 Remi Durand.
// Upstream is released under the AGPLv3 or higher; this port is AGPL-3.0-only.

//! Tool ordering without a wipe tower. Tool IDs are zero based throughout the engine.

#[derive(Clone, Debug, Default)]
pub struct WipingExtrusions {
    /// Reserved for the wipe-tower integration; currently no extrusion can absorb a purge.
    pub has_overrides: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LayerTools {
    pub print_z: f64,
    pub extruders: Vec<usize>,
    /// The layer prints object (not only support) extrusions. [`ToolOrdering::new`] assumes it;
    /// the print job clears it for support-only layers of independent support layer heights.
    pub has_object: bool,
    pub wiping: WipingExtrusions,
    /// Upstream `extruder_needed_for_color_changer`: the zero-based tool picked first on this
    /// layer so that its colour change (`M600`) runs with it.
    pub color_change_tool: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ToolOrdering {
    pub layers: Vec<LayerTools>,
    pub used: Vec<usize>,
    pub first: usize,
    pub last: usize,
    pub changes: usize,
}

impl ToolOrdering {
    pub fn new(layers: Vec<(f64, Vec<Option<usize>>)>, optimized: bool) -> Self {
        Self::with_color_changes(layers, optimized, &[], false)
    }

    /// [`Self::new`] with the tool each layer's colour change needs (`needed[i]`, upstream
    /// `extruder_needed_for_color_changer`), which then prints first on that layer. `tower`
    /// keeps upstream's exception for the first layer of a wipe-tower print.
    pub fn with_color_changes(
        layers: Vec<(f64, Vec<Option<usize>>)>,
        optimized: bool,
        needed: &[Option<usize>],
        tower: bool,
    ) -> Self {
        Self::with_options(layers, optimized, needed, tower, &[])
    }

    /// [`Self::with_color_changes`]; with a wipe tower, the first layer starts with its first
    /// soluble tool (`soluble[tool]`), so that the soluble filament is not wiped there.
    pub fn with_options(
        layers: Vec<(f64, Vec<Option<usize>>)>,
        optimized: bool,
        needed: &[Option<usize>],
        tower: bool,
        soluble: &[bool],
    ) -> Self {
        let first = layers
            .iter()
            .find_map(|(_, ts)| ts.iter().flatten().copied().min())
            .unwrap_or(0);
        let mut last = first;
        let mut result = Self {
            first,
            ..Self::default()
        };
        for (li, (print_z, mut tools)) in layers.into_iter().enumerate() {
            let color_change_tool = needed.get(li).copied().flatten();
            if let Some(t) = color_change_tool {
                tools.push(Some(t));
            }
            let has_paths = !tools.is_empty();
            let mut extruders: Vec<_> = tools.into_iter().flatten().collect();
            extruders.sort_unstable();
            extruders.dedup();
            if extruders.is_empty() && has_paths {
                extruders.push(last);
            }
            if optimized && let Some(i) = extruders.iter().position(|t| *t == last) {
                extruders[..=i].rotate_right(1);
            }
            if li == 0 && tower {
                // On the first layer with a wipe tower, prefer a soluble extruder at the
                // beginning, so it is not wiped on the first layer.
                if let Some(i) = extruders
                    .iter()
                    .position(|&t| soluble.get(t).copied().unwrap_or(false))
                {
                    extruders.swap(0, i);
                }
            } else if let Some(t) = color_change_tool
                && let Some(i) = extruders.iter().position(|e| *e == t)
            {
                // Put the extruder needed for the colour change at the beginning.
                extruders[..=i].rotate_right(1);
            }
            for &tool in &extruders {
                result.changes += usize::from(tool != last);
                last = tool;
                result.used.push(tool);
            }
            result.layers.push(LayerTools {
                print_z,
                extruders,
                has_object: true,
                wiping: WipingExtrusions::default(),
                color_change_tool,
            });
        }
        result.used.sort_unstable();
        result.used.dedup();
        result.first = result
            .layers
            .iter()
            .find_map(|l| l.extruders.first())
            .copied()
            .unwrap_or(0);
        result.last = last;
        result
    }
}

/// Upstream `EPSILON` of the layer comparisons in `fill_wipe_tower_partitions`.
const EPSILON: f64 = 1e-4;

impl ToolOrdering {
    /// Which layers print the wipe tower (upstream `LayerTools::has_wipe_tower`). Layers carrying
    /// a tool change, every object layer below the last change and every layer below the first
    /// object layer (raft) do; support-only layers that keep the current tool do not, unless the
    /// tower would otherwise skip more than `max_layer_height`. The tower layer height is the
    /// distance to the previous tower layer, so interleaved support layers never thin it.
    pub fn tower_layers(&self, max_layer_height: f64) -> Vec<bool> {
        let layers = &self.layers;
        let n = layers.len();
        // Count the minimum number of tool changes per layer.
        let mut partitions = vec![0usize; n];
        let mut last: Option<usize> = None;
        for (i, l) in layers.iter().enumerate() {
            partitions[i] = l.extruders.len();
            if let (Some(&front), Some(&back)) = (l.extruders.first(), l.extruders.last()) {
                if last.is_none_or(|t| t == front) {
                    // The first tool equals the current one: no initial tool change.
                    partitions[i] -= 1;
                }
                last = Some(back);
            }
        }
        // Propagate the partitions down so lower partitions support the upper ones.
        for i in (0..n.saturating_sub(1)).rev() {
            partitions[i] = partitions[i].max(partitions[i + 1]);
        }
        let first_object = layers.iter().position(|l| l.has_object).unwrap_or(n);
        let mut has: Vec<bool> = (0..n)
            .map(|i| (layers[i].has_object && partitions[i] > 0) || i < first_object)
            .collect();
        // A non-object layer starting with a different tool than the last one ended with (or with
        // more than one tool) needs the tower, and the tower must not skip more than
        // max_layer_height; upstream's loop bounds are kept.
        for i in 0..n.saturating_sub(1) {
            let (lt, next) = (&layers[i], &layers[i + 1]);
            if lt.extruders.is_empty() || next.extruders.is_empty() {
                break;
            }
            if !has[i + 1]
                && (next.extruders[0] != *lt.extruders.last().unwrap() || next.extruders.len() > 1)
            {
                has[i + 1] = true;
            }
            let mut j = i + 1;
            let mut last_z = next.print_z;
            loop {
                j += 1;
                if !(j < n - 1 && !has[j]) {
                    break;
                }
                if layers[j + 1].print_z - last_z > max_layer_height + EPSILON {
                    has[j] = true;
                    last_z = layers[j].print_z;
                }
            }
        }
        has
    }
}

/// Upstream `calc_max_layer_height`: the smallest tool `max_layer_height` (zero means 75% of the
/// nozzle), but at least the object layer height.
pub fn max_layer_height(c: &crate::config::Config) -> f64 {
    (0..c.vector_len("nozzle_diameter").max(1))
        .map(|i| {
            c.float_at("max_layer_height", i)
                .filter(|h| *h > 0.)
                .unwrap_or(0.75 * c.float_at("nozzle_diameter", i).unwrap_or(0.4))
        })
        .fold(f64::INFINITY, f64::min)
        .max(c.float("layer_height").unwrap_or(0.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn carry_last_tool_and_current_support_across_empty_layers() {
        let o = ToolOrdering::new(
            vec![
                (0.2, vec![Some(0), Some(1), Some(4), None]),
                (0.4, vec![Some(0), Some(1), Some(4)]),
                (0.6, vec![]),
                (0.8, vec![None]),
                (1.0, vec![Some(0), Some(4)]),
            ],
            true,
        );
        assert_eq!(
            o.layers
                .iter()
                .map(|l| l.extruders.clone())
                .collect::<Vec<_>>(),
            vec![vec![0, 1, 4], vec![4, 0, 1], vec![], vec![1], vec![0, 4]]
        );
        assert_eq!(o.used, [0, 1, 4]);
        assert_eq!(o.changes, 6);
    }

    /// `reorder_extruders`: with a wipe tower the first layer starts with a soluble tool, so
    /// that it is not wiped there; later layers keep the last tool first.
    #[test]
    fn first_tower_layer_starts_with_the_soluble_tool() {
        let layers = || vec![(0.2, vec![Some(0), Some(1)]), (0.4, vec![Some(0), Some(1)])];
        let soluble = [false, true];
        let tools = |o: ToolOrdering| -> Vec<Vec<usize>> {
            o.layers.into_iter().map(|l| l.extruders).collect()
        };
        assert_eq!(
            tools(ToolOrdering::with_options(
                layers(),
                true,
                &[],
                true,
                &soluble
            )),
            [vec![1, 0], vec![0, 1]]
        );
        // Without a tower, or without a soluble tool, the sorted order stays.
        assert_eq!(
            tools(ToolOrdering::with_options(
                layers(),
                true,
                &[],
                false,
                &soluble
            )),
            [vec![0, 1], vec![1, 0]]
        );
        assert_eq!(
            tools(ToolOrdering::with_options(layers(), true, &[], true, &[])),
            [vec![0, 1], vec![1, 0]]
        );
    }
}
