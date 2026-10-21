// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/OrganicSupport.cpp :: organic_draw_branches
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: finalize_interface_and_support_areas, generate_support_areas
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: organic_draw_branches, recover_pending_branch_roofs
// Changes: Owned layer-indexed area storage replaces SupportGeneratorLayer pointers; branches drawn in parallel and combined per tree in order; existing Mica mesh slicer and Clipper2 geometry; branch meshes in object coordinates, as upstream, then moved back to the bed.
// Changes: Safety-offset clip areas are grown once per layer on first use; the per-tree unions run in parallel and are appended in tree order.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník
// Tree supports by Thomas Rahm, loosely based on CuraEngine tree supports.
// Copyright (c) 2021 Ultimaker B.V.

use super::{
    branch_mesh::{NodeId, extrude_branch},
    geometry::area,
    nudge::organic_smooth_branches_avoid_collisions,
    smoothing::smooth_outward,
    *,
};
use crate::{
    geometry::clip,
    slice::{SliceRounding, slice_branch_mesh},
};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// Layer-indexed equivalent of upstream intermediate_layers, bottom_contacts and
/// InterfacePlacer storage. Print heights come from the same TreeSlicingParameters grid.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeDrawing {
    pub base: Vec<Areas>,
    pub bottom_contacts: Vec<Areas>,
    pub roofs: TreeRoofs,
}
struct Branch {
    path: Vec<NodeId>,
    root: bool,
    tip: bool,
}
/// One drawn branch: its slices from layer `begin`, floor contacts and recovered roofs.
struct DrawnBranch {
    begin: usize,
    slices: Vec<Areas>,
    bottoms: Vec<Areas>,
    recovered: Vec<(Areas, usize, usize)>,
}
fn visit(
    bounds: &[SupportElements],
    start: NodeId,
    marked: &mut [Vec<bool>],
    out: &mut Vec<Branch>,
) {
    marked[start.0][start.1] = true;
    let root = out.is_empty();
    for &parent in &bounds[start.0][start.1].parents {
        let mut path = vec![start];
        let mut at = (start.0 + 1, parent as usize);
        loop {
            path.push(at);
            let e = &bounds[at.0][at.1];
            if e.parents.len() != 1 {
                if e.parents.is_empty() {
                    marked[at.0][at.1] = true;
                }
                break;
            }
            marked[at.0][at.1] = true;
            at = (at.0 + 1, e.parents[0] as usize);
        }
        let tip = bounds[at.0][at.1].parents.is_empty();
        out.push(Branch { path, root, tip });
        if !tip {
            visit(bounds, at, marked, out);
        }
    }
}
/// One tree's drawn base and bottom-contact areas per layer, and its recovered roofs.
type TreeAreas = (
    BTreeMap<usize, Areas>,
    BTreeMap<usize, Areas>,
    Vec<(Areas, usize, usize)>,
);

/// Per-layer clip areas with their safety-offset growth (Orca clips with
/// `ApplySafetyOffset::Yes`), grown once per layer on first use instead of once per
/// branch slice. The results are those of `diff_ex_safety` / `intersection_ex_safety`.
struct Clips<'a> {
    raw: &'a [Areas],
    grown: Vec<std::sync::OnceLock<Areas>>,
}
impl<'a> Clips<'a> {
    fn new(raw: &'a [Areas]) -> Self {
        Self {
            raw,
            grown: raw.iter().map(|_| std::sync::OnceLock::new()).collect(),
        }
    }
    fn grown(&self, l: usize) -> &Areas {
        self.grown[l].get_or_init(|| clip::safety_offset_ex(&self.raw[l]))
    }
}
fn trim(a: &Areas, b: &Clips, l: usize, orca: bool) -> Areas {
    if orca {
        clip::diff_ex_safety_grown(a, &b.raw[l], b.grown(l))
    } else {
        clip::diff_ex(a, &b.raw[l])
    }
}
fn intersect(a: &Areas, b: &Clips, l: usize, orca: bool) -> Areas {
    if orca {
        clip::intersection_ex_safety_grown(a, &b.raw[l], b.grown(l))
    } else {
        clip::intersection_ex(a, &b.raw[l])
    }
}
fn remove_small(a: &mut Areas) {
    let mut polygons = clip::to_polygons(a);
    polygons.retain(|p| crate::geometry::signed_area(p).abs() >= tiny_area_threshold());
    *a = clip::union_ex_positive(&polygons);
}

/// Draw organic branches using the active pinned upstream mesh path. Cancellation
/// is checked between nodes, trees, branches and output layers.
pub fn organic_draw_branches<E>(
    v: &mut TreeModelVolumes,
    c: &TreeSupportSettings,
    p: &TreeSlicingParameters,
    infill: &TreeSupportInfill,
    mut pathing: TreePathing,
    mut cancel: impl FnMut() -> Result<(), E>,
) -> Result<TreeDrawing, E> {
    let bounds = &mut pathing.move_bounds;
    let n = bounds.len();
    let orca = c.settings.rules.is_orca();
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportSmooth);
    organic_smooth_branches_avoid_collisions(v, c, p, bounds, &mut cancel)?;
    drop(timer);
    v.clear_all_but_object_collision();
    let mut marked: Vec<_> = bounds.iter().map(|b| vec![false; b.len()]).collect();
    let mut trees = Vec::new();
    for l in 0..n.saturating_sub(1) {
        for i in 0..bounds[l].len() {
            if !marked[l][i] && !bounds[l][i].parents.is_empty() {
                let mut branches = Vec::new();
                visit(bounds, (l, i), &mut marked, &mut branches);
                trees.push(branches);
            }
        }
    }
    let mut result = TreeDrawing {
        base: vec![vec![]; n],
        bottom_contacts: vec![vec![]; n],
        roofs: pathing.roofs,
    };
    // Radius-0 volumes used while drawing, gathered once so the trees can be drawn
    // in parallel like upstream's tbb::parallel_for over trees.
    let v = &*v;
    let collision_min: Vec<Areas> = (0..n)
        .into_par_iter()
        .map(|l| (*v.get_collision(0, l, true)).clone())
        .collect();
    let collision: Vec<Areas> = (0..n)
        .into_par_iter()
        .map(|l| (*v.get_collision(0, l, false)).clone())
        .collect();
    let placeable: Vec<Areas> = (0..n)
        .into_par_iter()
        .map(|l| (*v.get_placeable_areas(0, l)).clone())
        .collect();
    let bed_area = [v.bed_area.clone()];
    let (collision_min, collision, placeable, bed_area) = (
        Clips::new(&collision_min),
        Clips::new(&collision),
        Clips::new(&placeable),
        Clips::new(&bed_area),
    );
    let bounds = &*bounds;
    cancel()?;
    // Branches are independent (upstream draws trees in parallel; one big tree would keep a
    // single worker busy). Each branch is drawn on its own, then the results are combined
    // tree by tree in the original branch order.
    let draw = |branch: &Branch| -> Option<DrawnBranch> {
        let mut recovered = Vec::new();
        let (mesh, (zmin, zmax)) = extrude_branch(bounds, &branch.path, branch.root, c, p);
        let first = branch.path[0];
        let last = *branch.path.last().unwrap();
        let mut begin = if branch.root {
            first.0
        } else {
            first.0.min(c.layer_idx_ceil(p, f64::from(zmin)))
        };
        let end = (if branch.tip {
            last.0
        } else {
            last.0.max(c.layer_idx_floor(p, f64::from(zmax)))
        } + 1)
            .min(n);
        if begin >= end {
            return None;
        }
        let zs: Vec<_> = (begin..end)
            .map(|l| {
                let z = c.layer_z(p, l);
                let bottom = if l > 0 { c.layer_z(p, l - 1) } else { 0. };
                f64::from((0.5 * (bottom + z)) as f32)
            })
            .collect();
        let mut slices: Vec<Areas> = slice_branch_mesh(
            &mesh,
            &zs,
            if orca {
                SliceRounding::Nearest
            } else {
                SliceRounding::Prusa
            },
        )
        .into_iter()
        .map(|mut s| {
            // The branch meshes are in object coordinates; back to the bed.
            for ex in &mut s.islands {
                for poly in std::iter::once(&mut ex.contour).chain(ex.holes.iter_mut()) {
                    for q in poly.iter_mut() {
                        q.x += p.origin.x;
                        q.y += p.origin.y;
                    }
                }
            }
            s.islands
        })
        .collect();
        for (i, slice) in slices.iter_mut().enumerate() {
            *slice = trim(slice, &collision_min, begin + i, orca);
            if orca {
                *slice = intersect(slice, &bed_area, 0, true);
                remove_small(slice);
            }
        }
        let empty = slices.iter().take_while(|s| s.is_empty()).count();
        if empty == slices.len() {
            return None;
        }
        // Prusa skips landing generation when the original front is empty.
        // Orca first removes empty slices and uses the new front for contacts.
        let landing = empty == 0 || orca;
        if orca && empty > 0 {
            slices.drain(..empty);
            begin += empty;
        }
        let front = slices[0].clone();
        let mut bottoms = Vec::new();
        if landing && branch.root {
            let e = &bounds[first.0][first.1];
            let floor = c.settings.support_floor_layers > 0;
            if e.state.to_model_gracious {
                if floor {
                    let mut contacts = if orca
                        && c.support_rests_on_model
                        && c.z_distance_bottom_layers > 0
                        && begin > 0
                    {
                        front.clone()
                    } else {
                        intersect(&front, &placeable, begin, orca)
                    };
                    if orca {
                        remove_small(&mut contacts);
                        if contacts.is_empty() && c.support_rests_on_model && begin > 0 {
                            contacts = front.clone();
                        }
                    }
                    bottoms.push(contacts);
                }
            } else if begin > 0 {
                let radius = e.radius(c);
                let max_layers = (5 * radius / c.layer_height) as usize;
                let bottom = if e.state.verylost {
                    0
                } else {
                    begin.saturating_sub(max_layers)
                };
                let stop = (0.2 * std::f64::consts::PI * (radius as f64).powi(2))
                    .max(0.5 * std::f64::consts::PI * (c.branch_radius as f64).powi(2));
                let mut extra = Vec::<Areas>::new();
                let mut rest = Vec::new();
                for l in (bottom..begin).rev() {
                    let collision_layer = if orca && l == begin - 1 { begin } else { l };
                    rest = trim(
                        if rest.is_empty() { &front } else { &rest },
                        &collision,
                        collision_layer,
                        orca,
                    );
                    if orca {
                        remove_small(&mut rest);
                    }
                    if area(&rest) < stop {
                        break;
                    }
                    extra.push(rest.clone());
                }
                if floor {
                    if orca {
                        let reference = extra.last().unwrap_or(&front);
                        let mut contacts = if c.support_rests_on_model
                            && c.z_distance_bottom_layers > 0
                            && begin > 0
                        {
                            if extra.is_empty() {
                                front.clone()
                            } else {
                                intersect(reference, &bed_area, 0, true)
                            }
                        } else {
                            intersect(reference, &placeable, begin, true)
                        };
                        remove_small(&mut contacts);
                        if contacts.is_empty() && c.support_rests_on_model && begin > 0 {
                            contacts = front.clone();
                        }
                        if !contacts.is_empty() {
                            bottoms.push(contacts);
                        }
                    } else {
                        for i in (0..extra.len().saturating_sub(1)).rev() {
                            bottoms.push(intersect(&extra[i], &placeable, begin - i - 1, false));
                        }
                    }
                }
                begin -= extra.len();
                extra.reverse();
                extra.append(&mut slices);
                slices = extra;
            }
        }
        // Orca recover_pending_branch_roofs: tip slices of branches still owing
        // roof layers become contacts/interfaces instead of base.
        if orca && infill.num_top_interface_layers > 0 {
            for &(l, i) in branch.path.iter().rev() {
                let state = &bounds[l][i].state;
                if !state.has_pending_roof_recovery() {
                    break;
                }
                let Some(k) = l.checked_sub(begin).filter(|&k| k < slices.len()) else {
                    continue;
                };
                if slices[k].is_empty()
                    || state.orca.roof_recovery_dtt as usize > infill.num_top_interface_layers
                {
                    continue;
                }
                let area = std::mem::take(&mut slices[k]);
                recovered.push((area, l, state.orca.roof_recovery_dtt as usize));
            }
        }
        Some(DrawnBranch {
            begin,
            slices,
            bottoms,
            recovered,
        })
    };
    let flat: Vec<(usize, &Branch)> = trees
        .iter()
        .enumerate()
        .flat_map(|(t, branches)| branches.iter().map(move |b| (t, b)))
        .collect();
    let branches: Vec<Option<DrawnBranch>> = flat.par_iter().map(|&(_, b)| draw(b)).collect();
    cancel()?;
    let mut drawn: Vec<TreeAreas> =
        vec![(BTreeMap::new(), BTreeMap::new(), Vec::new()); trees.len()];
    for (&(t, _), branch) in flat.iter().zip(branches) {
        let Some(mut branch) = branch else {
            continue;
        };
        let (tree_base, tree_bottom, recovered): &mut (
            BTreeMap<usize, Areas>,
            BTreeMap<usize, Areas>,
            Vec<_>,
        ) = &mut drawn[t];
        recovered.append(&mut branch.recovered);
        let begin = branch.begin;
        for (j, slice) in branch.slices.into_iter().enumerate() {
            if begin + j >= n {
                break;
            }
            if !slice.is_empty() {
                tree_base.entry(begin + j).or_default().extend(slice);
                if let Some(contact) = branch.bottoms.get_mut(j) {
                    tree_bottom.entry(begin + j).or_default().append(contact);
                }
            }
        }
    }
    // Union within each tree before combining separate trees (upstream order). The unions
    // are independent, so they run in parallel and are appended in tree order.
    let unions = |pick: fn(&TreeAreas) -> &BTreeMap<usize, Areas>| -> Vec<(usize, Areas)> {
        let jobs: Vec<(usize, &Areas)> = drawn
            .iter()
            .flat_map(|tree| pick(tree).iter().map(|(&l, a)| (l, a)))
            .collect();
        jobs.par_iter()
            .map(|&(l, a)| (l, clip::union_ex(a)))
            .collect()
    };
    let base_unions = unions(|t| &t.0);
    let bottom_unions = unions(|t| &t.1);
    for (_, _, recovered) in drawn {
        for (area, l, dtt) in recovered {
            result.roofs.add(c, infill, area, l, dtt);
        }
    }
    for (l, a) in base_unions {
        result.base[l].extend(a);
    }
    for (l, a) in bottom_unions {
        result.bottom_contacts[l].extend(a);
    }
    // Organic's active finalizer: unlike draw_areas it does not infer floors from
    // overhangs or classify missing-roof branch circles as new roofs (#if 0).
    // Layers are independent here.
    cancel()?;
    let tops = &result.roofs.top_contacts;
    result
        .base
        .par_iter_mut()
        .zip(result.bottom_contacts.par_iter_mut())
        .enumerate()
        .for_each(|(l, (base_out, bottom_out))| {
            let mut base = polygons_simplify(
                &smooth_outward(clip::union_ex(base_out), c.support_line_width),
                scaled(0.03).min(c.resolution) as f64,
            );
            let mut bottom = clip::union_ex(bottom_out);
            let top = &tops[l];
            if !top.is_empty() && !base.is_empty() {
                base = clip::diff_ex(&base, top);
                bottom = clip::diff_ex(&bottom, top);
            }
            if !bottom.is_empty() {
                base = clip::diff_ex(&base, &bottom);
            }
            *base_out = clip::union_ex(&base);
            *bottom_out = bottom;
        });
    cancel()?;
    Ok(result)
}
