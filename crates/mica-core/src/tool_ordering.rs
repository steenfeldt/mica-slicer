// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::reorder_extruders, ToolOrdering::collect_extruder_statistics
// Source: orca@8500fcdcca src/libslic3r/GCode/ToolOrdering.cpp :: ToolOrdering::fill_wipe_tower_partitions, calc_max_layer_height
// Changes: Zero-based Rust tools, None for don't-care support; compact layer lists.
// Changes: The extruder a colour change needs prints first on its layer (reorder_extruders, extruder_needed_for_color_changer).
// Changes: With a wipe tower the first layer starts with its first soluble tool (reorder_extruders).
// Changes: `has_object` and `tower_layers` (which layers carry the wipe tower) from fill_wipe_tower_partitions; the raft gap layer is the caller's filler layer.
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ToolOrdering.cpp :: is_overriddable, ToolOrdering::collect_extruders, WipingExtrusions::ensure_perimeters_infills_order, LayerTools::is_extruder_order
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/ExtrusionOrder.cpp :: get_extruder_id
// Source: orca@8500fcdcca src/libslic3r/GCode/ToolOrdering.cpp :: WipingExtrusions::is_overriddable, WipingExtrusions::ensure_perimeters_infills_order
// Changes: Wiping into infill/objects without mark_wiping_extrusions (it marks nothing for independent tools); Orca's ensure_perimeters_infills_order parenthesis quirk kept.

// Copyright (c) Prusa Research 2017–2023 Lukáš Matěna, Vojtěch Bubník, Oleksandra Iushchenko, Enrico Turri, Tomáš Mészáros.
// Copyright (c) SuperSlicer 2023 Remi Durand.
// Upstream is released under the AGPLv3 or higher; this port is AGPL-3.0-only.

//! Tool ordering without a wipe tower. Tool IDs are zero based throughout the engine.

use crate::toolpath::Role;

#[derive(Clone, Debug, Default)]
pub struct LayerTools {
    pub print_z: f64,
    pub extruders: Vec<usize>,
    /// The layer prints object (not only support) extrusions. [`ToolOrdering::new`] assumes it;
    /// the print job clears it for support-only layers of independent support layer heights.
    pub has_object: bool,
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

/// Which purge-absorbing options apply to a region: PrusaSlicer's `wipe_into_infill` (region)
/// and `wipe_into_objects` (object), OrcaSlicer's `flush_into_infill` / `flush_into_objects`
/// (both object settings, read into the same keys).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WipeInto {
    pub infill: bool,
    pub objects: bool,
}

impl WipeInto {
    pub fn any(self) -> bool {
        self.infill || self.objects
    }
}

/// The upstream entity collection a path belongs to: a layer region's perimeters (walls and
/// gap fill), its fills (every infill role and ironing), or something the wiping rules never
/// touch (support, skirt, brim).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Perimeter,
    Fill,
    Other,
}

pub fn entity_kind(role: Role) -> EntityKind {
    match role {
        Role::ExternalPerimeter | Role::Perimeter | Role::OverhangPerimeter | Role::GapFill => {
            EntityKind::Perimeter
        }
        Role::InternalInfill
        | Role::SolidInfill
        | Role::BridgeInfill
        | Role::InternalBridgeInfill
        | Role::TopSolidInfill
        | Role::Ironing => EntityKind::Fill,
        _ => EntityKind::Other,
    }
}

/// A region's tools on a layer (upstream `LayerTools::perimeter_extruder`,
/// `solid_infill_extruder`, `infill_extruder`, a per-layer tool switch already applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionTools {
    pub perimeter: usize,
    pub solid: usize,
    pub infill: usize,
}

/// Upstream `WipingExtrusions`: which extrusions may print with another tool than their own so
/// that they absorb the purge of a tool change, and which tool prints them.
///
/// The purge volume bookkeeping (`mark_wiping_extrusions`) is not needed: upstream only marks
/// extrusions for the purge a single-extruder multi-material printer computes from its purging
/// volumes. For independent tools PrusaSlicer's volume left after the minimal tower purge is
/// zero (`extract_wipe_volumes` keeps only `filament_minimal_purge_on_wipe_tower`), and Orca's
/// WipeTower2 calls it only for `purge_in_prime_tower` with SEMM. Mica refuses SEMM towers, so
/// on every printer it slices upstream marks nothing; what remains is `collect_extruders`
/// (an overridable entity does not ask for its own tool), `ensure_perimeters_infills_order`
/// on the tower layers and the "tool not on this layer: the layer's last tool" fallback.
#[derive(Clone, Debug)]
pub struct WipingExtrusions {
    pub orca: bool,
    soluble: Vec<bool>,
    /// Orca's `filament_is_support` (first/last non-soluble tool skips these too).
    support: Vec<bool>,
}

impl WipingExtrusions {
    pub fn new(orca: bool, soluble: Vec<bool>, support: Vec<bool>) -> Self {
        Self {
            orca,
            soluble,
            support,
        }
    }

    fn soluble(&self, tool: usize) -> bool {
        self.soluble.get(tool).copied().unwrap_or(false)
    }

    fn body(&self, tool: usize) -> bool {
        !self.soluble(tool) && !(self.orca && self.support.get(tool).copied().unwrap_or(false))
    }

    /// Upstream `is_overriddable`: `tool` is the entity's own (zero-based) tool.
    pub fn is_overridable(&self, role: Role, tool: usize, into: WipeInto) -> bool {
        let kind = entity_kind(role);
        if kind == EntityKind::Other || self.soluble(tool) {
            return false;
        }
        if into.objects {
            return true;
        }
        into.infill && role == Role::InternalInfill
    }

    /// Upstream `collect_extruders` for one layer region: the tools it needs when `roles` are
    /// its perimeter and fill paths. A non-overridable perimeter asks for the perimeter tool; a
    /// non-overridable fill asks for the solid tool (if the region has solid fill) and the
    /// infill tool (if it has sparse infill), so an overridable sparse infill next to solid
    /// infill keeps its tool, as upstream.
    pub fn collect_region(
        &self,
        roles: impl IntoIterator<Item = Role>,
        tools: RegionTools,
        into: WipeInto,
        out: &mut Vec<Option<usize>>,
    ) {
        let (mut perimeters, mut solid, mut sparse) = (false, false, false);
        let (mut fixed_perimeter, mut fixed_fill) = (false, false);
        for role in roles {
            let own = match role {
                Role::InternalInfill => tools.infill,
                r if entity_kind(r) == EntityKind::Fill => tools.solid,
                _ => tools.perimeter,
            };
            let fixed = !self.is_overridable(role, own, into);
            match entity_kind(role) {
                EntityKind::Perimeter => {
                    perimeters = true;
                    fixed_perimeter |= fixed;
                }
                EntityKind::Fill => {
                    if role == Role::InternalInfill {
                        sparse = true;
                    } else {
                        solid = true;
                    }
                    fixed_fill |= fixed;
                }
                EntityKind::Other => {}
            }
        }
        let mut push = |t: usize| {
            if !out.contains(&Some(t)) {
                out.push(Some(t));
            }
        };
        if perimeters && fixed_perimeter {
            push(tools.perimeter);
        }
        if fixed_fill {
            if solid {
                push(tools.solid);
            }
            if sparse {
                push(tools.infill);
            }
        }
    }

    fn first_body(&self, layer: &[usize]) -> Option<usize> {
        layer.iter().copied().find(|&t| self.body(t))
    }

    fn last_body(&self, layer: &[usize]) -> Option<usize> {
        layer.iter().rev().copied().find(|&t| self.body(t))
    }

    /// Upstream `ensure_perimeters_infills_order` for one overridable entity that no purge
    /// claimed, on a layer the wipe tower plans (`layer` are its tools in print order): the
    /// tool it is forced to, or `None` when it prints normally.
    pub fn ensure_order(
        &self,
        layer: &[usize],
        role: Role,
        tools: RegionTools,
        into: WipeInto,
        infill_first: bool,
    ) -> Option<usize> {
        let (first, last) = (self.first_body(layer), self.last_body(layer));
        match entity_kind(role) {
            EntityKind::Fill => {
                let force = if self.orca {
                    // OrcaSlicer's parentheses pass `last || !has_extruder(sparse)` (0 or 1) as
                    // the second tool of `is_extruder_order`; its `flush_into_objects` clause
                    // is commented out. Kept as Orca behaves.
                    let b = usize::from(last != Some(0) || !layer.contains(&tools.infill));
                    infill_first || is_extruder_order(layer, tools.perimeter, b)
                } else {
                    infill_first
                        || into.objects
                        || last.is_some_and(|l| is_extruder_order(layer, tools.perimeter, l))
                        || !layer.contains(&tools.infill)
                };
                if force {
                    if infill_first { first } else { last }
                } else {
                    None
                }
            }
            EntityKind::Perimeter => {
                if infill_first {
                    last
                } else {
                    first
                }
            }
            EntityKind::Other => None,
        }
    }
}

/// Upstream `LayerTools::is_extruder_order`: whether `a` prints before `b` on the layer.
pub fn is_extruder_order(layer: &[usize], a: usize, b: usize) -> bool {
    if a == b {
        return false;
    }
    for &t in layer {
        if t == a {
            return true;
        }
        if t == b {
            return false;
        }
    }
    false
}

/// GCode's `get_extruder_id` fallback: an entity whose tool is not on the layer (its tool was
/// dropped because the entity could absorb a purge) prints with the layer's last tool.
pub fn layer_tool_or_last(layer: &[usize], tool: usize) -> usize {
    if layer.contains(&tool) {
        tool
    } else {
        layer.last().copied().unwrap_or(tool)
    }
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

    const INFILL: WipeInto = WipeInto {
        infill: true,
        objects: false,
    };
    const OBJECTS: WipeInto = WipeInto {
        infill: false,
        objects: true,
    };
    /// Walls and solid infill on tool 1, sparse infill on tool 2 (zero based 0 / 1).
    const SPLIT: RegionTools = RegionTools {
        perimeter: 0,
        solid: 0,
        infill: 1,
    };

    fn prusa(soluble: &[bool]) -> WipingExtrusions {
        WipingExtrusions::new(false, soluble.to_vec(), vec![])
    }

    /// `is_overriddable`: purging into infill takes sparse infill only, into objects every wall
    /// and fill; soluble filament, supports, skirt and brim never.
    #[test]
    fn overridable_entities() {
        let w = prusa(&[false, false, true]);
        assert!(w.is_overridable(Role::InternalInfill, 1, INFILL));
        for role in [Role::SolidInfill, Role::TopSolidInfill, Role::Perimeter] {
            assert!(!w.is_overridable(role, 0, INFILL), "{role:?}");
        }
        for role in [
            Role::ExternalPerimeter,
            Role::GapFill,
            Role::BridgeInfill,
            Role::Ironing,
            Role::InternalInfill,
        ] {
            assert!(w.is_overridable(role, 0, OBJECTS), "{role:?}");
        }
        for role in [Role::SupportMaterial, Role::Brim, Role::Skirt] {
            assert!(!w.is_overridable(role, 0, OBJECTS), "{role:?}");
        }
        assert!(!w.is_overridable(Role::InternalInfill, 2, INFILL));
        assert!(!w.is_overridable(Role::InternalInfill, 1, WipeInto::default()));
    }

    fn collect(roles: &[Role], into: WipeInto) -> Vec<Option<usize>> {
        let mut out = Vec::new();
        prusa(&[false, false]).collect_region(roles.iter().copied(), SPLIT, into, &mut out);
        out
    }

    /// `collect_extruders`: an overridable sparse infill does not ask for its tool, unless a
    /// non-overridable fill (solid infill) of the same region does; then upstream adds both.
    #[test]
    fn overridable_infill_drops_its_tool_unless_solid_infill_shares_the_region() {
        use Role::*;
        let sparse = [ExternalPerimeter, Perimeter, InternalInfill];
        assert_eq!(collect(&sparse, WipeInto::default()), [Some(0), Some(1)]);
        assert_eq!(collect(&sparse, INFILL), [Some(0)]);
        let mixed = [ExternalPerimeter, SolidInfill, InternalInfill];
        assert_eq!(collect(&mixed, INFILL), [Some(0), Some(1)]);
        // Into the object: nothing asks for a tool (the caller adds "don't care").
        assert_eq!(collect(&mixed, OBJECTS), []);
    }

    /// `ensure_perimeters_infills_order` with PrusaSlicer's rules.
    #[test]
    fn unclaimed_entities_take_the_first_or_last_body_tool() {
        use Role::*;
        let w = prusa(&[false, false, true]);
        // The infill tool left the layer: the infill prints last, with the wall tool.
        assert_eq!(
            w.ensure_order(&[0], InternalInfill, SPLIT, INFILL, false),
            Some(0)
        );
        // Walls (tool 1) before the infill tool: the infill moves to the last tool, its own.
        assert_eq!(
            w.ensure_order(&[0, 1], InternalInfill, SPLIT, INFILL, false),
            Some(1)
        );
        // Walls last: left alone, so it prints normally with tool 2 (before its walls).
        assert_eq!(
            w.ensure_order(&[1, 0], InternalInfill, SPLIT, INFILL, false),
            None
        );
        // Infill first: the first body tool; soluble tool 3 is skipped.
        assert_eq!(
            w.ensure_order(&[2, 1, 0], InternalInfill, SPLIT, INFILL, true),
            Some(1)
        );
        // Into objects: fills always forced, walls to the first body tool.
        assert_eq!(
            w.ensure_order(&[2, 1, 0], SolidInfill, SPLIT, OBJECTS, false),
            Some(0)
        );
        assert_eq!(
            w.ensure_order(&[2, 1, 0], Perimeter, SPLIT, OBJECTS, false),
            Some(1)
        );
        // Only soluble tools: nothing to force.
        assert_eq!(w.ensure_order(&[2], Perimeter, SPLIT, OBJECTS, false), None);
    }

    /// OrcaSlicer passes `last || !has_extruder(sparse)` (0 or 1) as the second tool of
    /// `is_extruder_order` and has no into-objects clause; Mica keeps that.
    #[test]
    fn orca_keeps_its_is_extruder_order_argument() {
        let tools = RegionTools {
            perimeter: 3,
            solid: 3,
            infill: 1,
        };
        let layer = [1, 3, 2];
        let prusa = prusa(&[false; 4]);
        let orca = WipingExtrusions::new(true, vec![false; 4], vec![false; 4]);
        // PrusaSlicer: the walls (4) print before the last tool (3): forced to 3.
        assert_eq!(
            prusa.ensure_order(&layer, Role::InternalInfill, tools, INFILL, false),
            Some(2)
        );
        // Orca compares the walls with tool 1 (index 1), which comes first: left alone.
        assert_eq!(
            orca.ensure_order(&layer, Role::InternalInfill, tools, INFILL, false),
            None
        );
        // Orca skips support filaments for the first/last tool.
        let orca = WipingExtrusions::new(true, vec![false; 4], vec![false, false, true, false]);
        assert_eq!(
            orca.ensure_order(&layer, Role::Perimeter, tools, OBJECTS, true),
            Some(3)
        );
    }

    #[test]
    fn dropped_tool_falls_back_to_the_layer_last() {
        assert_eq!(layer_tool_or_last(&[0, 2], 2), 2);
        assert_eq!(layer_tool_or_last(&[2, 0], 1), 0);
        assert!(is_extruder_order(&[2, 0, 1], 0, 1));
        assert!(!is_extruder_order(&[2, 0, 1], 1, 0));
        assert!(!is_extruder_order(&[2, 0, 1], 0, 0));
    }
}
