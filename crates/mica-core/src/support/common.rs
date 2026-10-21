// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportCommon.cpp :: generate_support_layers, generate_interface_layers, generate_raft_base, generate_support_toolpaths, tree_supports_generate_paths
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportMaterial.cpp :: PrintObjectSupportMaterial::raft_and_intermediate_support_layers
// Changes: One non-overlapping support slab per Z; polygon plan with lazy fills; no overlapping-contact
// Changes: extrusion modulation or contact loops. Base interface layers (num_top/bottom_base_interface_layers)
// Changes: print the interface pattern with the base role; a zero top gap synchronizes only PrusaSlicer's layers.
// Changes: Prusa flange sheath/fill collections and organic anchors retain no-sort ranges for support ordering.
// Changes: Sparse base/interface fills use the support-specific arch/zig-zag connector (FillSupportBase).
// Source: orca@8500fcdcca src/libslic3r/Support/SupportCommon.cpp :: generate_interface_layers
// Source: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp :: SupportParameters::SupportParameters
// Changes: Orca semantics: interface counts exclude the contact layer (one more dense layer).
// Changes: Smooth-style interface regularization (closing by the island radius; no smooth_outward)
// Changes: and the tree slab projection over compacted intermediate layers.
// Copyright (c) Prusa Research 2016 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas, Roman Beránek @zavorka, Lukáš Matěna @lukasmatena, Vojtěch Král @vojtechkral
// Copyright (c) SuperSlicer 2023 Remi Durand @supermerill
// Copyright (c) 2016 Sakari Kapanen @Flannelhead
//
// ported from lib/Slic3r/Print/SupportMaterial.pm:
// Copyright (c) Prusa Research 2016 - 2017 Vojtěch Bubník @bubnikv
// Copyright (c) 2016 Joseph Lenox @lordofhyphens
// Copyright (c) Slic3r 2013 - 2015 Alessandro Ranellucci @alranel
// Copyright (c) 2013 Mark Hindess
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

use super::material::Contact;
use super::{
    EPS, InterfacePattern, SupportInput, SupportLayer, SupportLayerKind as Kind, SupportParameters,
    SupportPattern, SupportRegion, SupportStyle,
};
use crate::fill::{self, FillParams, Pattern};
use crate::flow::Flow;
use crate::geometry::clip::{Join, diff_ex, intersection_ex, offset_ex, union_ex};
use crate::geometry::{Point, SCALE, scale};
use crate::toolpath::{ExtrusionPath, LayerToolpaths, Role};

/// Schedule maximum-height slabs between exact contact heights. Retains the first print layer,
/// and snaps intermediate heights to object layers within 1/10 of an object layer height.
pub(super) fn generate_support_layers(
    input: &SupportInput<'_>,
    tops: &[Contact],
    bottoms: &[Contact],
    p: &SupportParameters,
) -> Vec<SupportLayer> {
    let c = &p.config;
    let Some(top) = tops.last() else {
        return Vec::new();
    };
    let start = if c.raft_layers > 0 {
        p.raft_top_z
    } else {
        c.first_layer_height
    };
    let mut extremes: Vec<f64> = tops
        .iter()
        .chain(bottoms)
        .map(|c| c.print_z)
        .filter(|z| *z > start + EPS && *z <= top.print_z + EPS)
        .collect();
    // Reserve the actual bottom-contact thickness above the air gap. Splitting only at its
    // top would let a maximum-height slab cross the gap and be discarded by object trimming.
    extremes.extend(
        bottoms
            .iter()
            .map(|b| b.print_z - c.layer_height)
            .filter(|z| *z > start + EPS && *z < top.print_z - EPS),
    );
    extremes.sort_by(f64::total_cmp);
    extremes.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let mut zs = vec![start];
    for end in extremes {
        let begin = *zs.last().unwrap();
        if end - begin < EPS {
            continue;
        }
        // A zero top gap (soluble interface) synchronizes PrusaSlicer's support layers; Orca
        // keeps independent support layer heights unless they are turned off.
        if (c.top_gap == 0.0 && !c.orca_semantics) || c.synchronize_layers {
            for layer in input.layers {
                if layer.print_z > begin + EPS && layer.print_z < end - EPS {
                    zs.push(layer.print_z);
                }
            }
            zs.push(end);
        } else {
            let n = ((end - begin) / p.max_height - EPS).ceil().max(1.0) as usize;
            let step = (end - begin) / n as f64;
            for i in 1..=n {
                let z = begin + step * i as f64;
                let snap = input
                    .layers
                    .iter()
                    .find(|l| (l.print_z - z).abs() < c.layer_height / 10.0);
                let z = if i < n {
                    snap.map_or(z, |l| l.print_z)
                } else {
                    end
                };
                // Snapping must not create a tiny or over-thick layer on either side.
                let last = *zs.last().unwrap();
                let z = if z - last < p.min_height - EPS
                    || z - last > p.max_height + EPS
                    || (i < n
                        && (end - z < p.min_height - EPS
                            || begin + step * (i + 1) as f64 - z > p.max_height + EPS
                            || begin + step * (i + 1) as f64 - z < p.min_height - EPS))
                {
                    begin + step * i as f64
                } else {
                    z
                };
                zs.push(z);
            }
        }
    }
    let mut previous = if c.raft_layers > 0 { p.raft_top_z } else { 0.0 };
    zs.into_iter()
        .filter_map(|z| {
            let height = z - previous;
            previous = z;
            (height > EPS).then_some(SupportLayer {
                print_z: z,
                height,
                interface_id: 0,
                regions: Vec::new(),
            })
        })
        .collect()
}

/// Project contact areas through the configured count of support layers, then subtract each dense
/// region from the base. Prusa counts include the contact layer itself (SupportParameters subtracts
/// one); Orca's do not, so normal supports get one more dense layer under Orca semantics.
///
/// `intermediate` marks tree layers that carry branch (intermediate) areas, excluding roofs.
/// Upstream collects, for each intermediate layer, the top contacts whose bottom Z lies below the
/// print Z of the intermediate layer `num_top - 1` positions higher in the compacted list. With
/// Orca's count on the tree's shared layer grid this reaches one layer further below a contact
/// while branches continue above (upstream: "maybe this adds one interface layer in excess?");
/// below a flat roof with no branch areas above, it does not.
pub(super) fn generate_interface_layers(
    layers: &mut [SupportLayer],
    tops: &[Contact],
    bottoms: &[Contact],
    p: &SupportParameters,
    intermediate: Option<&[bool]>,
) {
    let mut masks: Vec<Vec<SupportRegion>> = vec![Vec::new(); layers.len()];
    // Upstream's interface count excludes the contact layer: Prusa subtracts one from the
    // configured count, Orca does not, so Orca prints one more dense layer.
    let orca = p.config.orca_semantics;
    let num_top = if orca {
        p.config.top_interface_layers
    } else {
        p.config.top_interface_layers.saturating_sub(1)
    };
    // Print Z of the intermediate layer num_top-1 places above each intermediate layer. For
    // Prusa's count this never reaches below the plain range, so only Orca uses the slab test.
    let slab_top: Option<Vec<Option<f64>>> =
        intermediate.filter(|_| orca && num_top > 0).map(|flags| {
            let compact: Vec<usize> = (0..layers.len()).filter(|&i| flags[i]).collect();
            let mut top = vec![None; layers.len()];
            let step = num_top - 1;
            for (k, &i) in compact.iter().enumerate() {
                let j = compact[(k + step).min(compact.len() - 1)];
                top[i] = Some(layers[j].print_z);
            }
            top
        });
    for (contacts, count, top) in [
        (tops, p.config.top_interface_layers, true),
        (bottoms, p.bottom_interface_layers, false),
    ] {
        if count == 0 {
            continue;
        }
        for contact in contacts {
            let at = layers.partition_point(|l| l.print_z < contact.print_z - EPS);
            if at == layers.len() {
                continue;
            }
            // Normal supports keep separate contact and intermediate layers; under Orca's count
            // the slab reaches one more layer (contact + count dense layers).
            let span = if orca && intermediate.is_none() {
                count + 1
            } else {
                count
            };
            let mut range = if top {
                at.saturating_sub(span - 1)..at + 1
            } else {
                at..(at + span).min(layers.len())
            };
            if top && let Some(slab_top) = &slab_top {
                let bottom_z = if at == 0 { 0.0 } else { layers[at - 1].print_z };
                let mut first = range.start;
                while first > 0 && slab_top[first - 1].is_some_and(|z| bottom_z <= z + EPS) {
                    first -= 1;
                }
                range = first..range.end;
            }
            // The interface layers furthest from the contact print with the base tool
            // (`num_top/bottom_base_interface_layers`, contacts beyond
            // `num_*_interface_layers_only` of an intermediate layer).
            let base_layers = if top {
                p.config.top_base_interface_layers
            } else {
                p.config.bottom_base_interface_layers
            };
            let only = (span - 1).saturating_sub(base_layers);
            for i in range {
                let distance = i.abs_diff(at);
                let kind = match (top, i == at) {
                    (_, false) if distance > only && distance < span => Kind::BaseInterface,
                    (true, true) => Kind::TopContact,
                    (true, false) => Kind::TopInterface,
                    (false, true) => Kind::BottomContact,
                    (false, false) => Kind::BottomInterface,
                };
                masks[i].push(SupportRegion {
                    kind,
                    polygons: contact.polygons.clone(),
                });
            }
        }
    }
    // Upstream regularizes the projected contacts of smooth (non-grid) supports before trimming
    // them with the base: a closing whose expansion exceeds its shrink by the minimum island
    // radius (one interface line pitch), so base slivers next to a contact become interface.
    let smooth = p.config.style != SupportStyle::Grid;
    let smoothing = p.flow.spacing() * 1.5 * SCALE;
    let island = p.flow.spacing() / p.interface_density.max(EPS) * SCALE;
    for (layer, masks) in layers.iter_mut().zip(masks) {
        let mut base = union_ex(
            &layer
                .regions
                .drain(..)
                .flat_map(|r| r.polygons)
                .collect::<Vec<_>>(),
        );
        let masks = if smooth {
            regularize_masks(masks, smoothing, island)
        } else {
            masks
        };
        for mask in masks {
            let polygons = intersection_ex(&base, &mask.polygons);
            if !polygons.is_empty() {
                base = diff_ex(&base, &polygons);
                layer.regions.push(SupportRegion {
                    kind: mask.kind,
                    polygons,
                });
            }
        }
        if !base.is_empty() {
            layer.regions.push(SupportRegion {
                kind: Kind::Base,
                polygons: base,
            });
        }
    }
}

/// Union one layer's contact masks per kind (first-appearance order) and close each union with
/// `closing(polys, smoothing + island, smoothing)` as upstream `generate_interface_layers`
/// `insert_layer` does for smooth supports. `smooth_outward` is not applied.
fn regularize_masks(masks: Vec<SupportRegion>, smoothing: f64, island: f64) -> Vec<SupportRegion> {
    let mut grouped: Vec<SupportRegion> = Vec::new();
    for mask in masks {
        match grouped.iter_mut().find(|g| g.kind == mask.kind) {
            Some(g) => g.polygons.extend(mask.polygons),
            None => grouped.push(mask),
        }
    }
    for g in &mut grouped {
        let united = union_ex(&g.polygons);
        g.polygons = crate::geometry::clip::offset2_ex(
            &united,
            smoothing + island,
            -smoothing,
            Join::Square,
        );
    }
    grouped
}

/// Raft layers and the first-layer support flange use the same density/expansion settings.
pub(super) fn generate_raft_base(
    input: &SupportInput<'_>,
    layers: &mut Vec<SupportLayer>,
    p: &SupportParameters,
) {
    let c = &p.config;
    if c.raft_layers == 0 {
        if let Some(layer) = layers
            .first_mut()
            .filter(|l| l.print_z <= c.first_layer_height + EPS)
        {
            let trimming = offset_ex(input.layers[0].outlines, c.xy_gap * SCALE, Join::Round);
            for region in &mut layer.regions {
                if region.kind != Kind::Base {
                    continue;
                }
                // Incremental expansion cannot leak through thin walls to the object's other side.
                let n = (c.first_layer_expansion / c.first_layer_width)
                    .ceil()
                    .max(5.0) as usize;
                for _ in 0..n {
                    region.polygons = diff_ex(
                        &offset_ex(
                            &region.polygons,
                            c.first_layer_expansion * SCALE / n as f64,
                            Join::Round,
                        ),
                        &trimming,
                    );
                }
            }
        }
        return;
    }
    let footprint = offset_ex(
        input.layers[0].outlines,
        c.raft_expansion * SCALE,
        Join::Round,
    );
    let columns: Vec<_> = layers
        .first()
        .map(|l| l.regions.iter().flat_map(|r| r.polygons.clone()).collect())
        .unwrap_or_default();
    let base = union_ex(&[offset_ex(&footprint, 0.5 * SCALE, Join::Round), columns].concat());
    let mut raft = Vec::new();
    for i in 0..c.raft_layers {
        let height = if i == 0 {
            c.first_layer_height
        } else {
            p.raft_height
        };
        let z = c.first_layer_height + i as f64 * p.raft_height;
        let polygons = if c.raft_layers == 1 {
            footprint.clone()
        } else if i == 0 {
            offset_ex(
                &base,
                (c.first_layer_expansion - 0.5).max(0.0) * SCALE,
                Join::Round,
            )
        } else if i + 1 == c.raft_layers {
            footprint.clone()
        } else {
            base.clone()
        };
        raft.push(SupportLayer {
            print_z: z,
            height,
            interface_id: 0,
            regions: vec![SupportRegion {
                kind: if i < p.raft_base_layers {
                    Kind::RaftBase
                } else {
                    Kind::RaftInterface
                },
                polygons,
            }],
        });
    }
    raft.append(layers);
    *layers = raft;
}

pub(super) fn generate_support_toolpaths(
    layer: &SupportLayer,
    index: usize,
    p: &SupportParameters,
    origin: Point,
) -> LayerToolpaths {
    let c = &p.config;
    let first = layer.print_z - layer.height < EPS;
    let mut paths = Vec::new();
    let mut support_groups = Vec::new();
    // fill_expolygons_with_sheath_generate_paths keeps each first-layer island's
    // sheath and fill in one no-sort collection. Orca retains its existing flat order.
    let split_flange;
    let regions = if first && !c.orca_semantics {
        split_flange = layer
            .regions
            .iter()
            .flat_map(|r| {
                if matches!(r.kind, Kind::Base | Kind::RaftBase) {
                    r.polygons
                        .iter()
                        .map(|p| SupportRegion {
                            kind: r.kind,
                            polygons: vec![p.clone()],
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![r.clone()]
                }
            })
            .collect::<Vec<_>>();
        &split_flange
    } else {
        &layer.regions
    };
    for region in regions {
        let region_start = paths.len();
        let interface = !matches!(region.kind, Kind::Base | Kind::RaftBase);
        // Base interface layers: the interface pattern and density in the base material.
        let base_interface = region.kind == Kind::BaseInterface;
        let raft = matches!(region.kind, Kind::RaftBase | Kind::RaftInterface);
        let flange = first && !interface;
        let flow = if flange {
            p.first_layer_flow
        } else {
            Flow::new(c.width, layer.height)
        };
        let role = if interface && !base_interface {
            Role::SupportMaterialInterface
        } else {
            Role::SupportMaterial
        };
        let density = if flange {
            c.first_layer_density
        } else if interface {
            p.interface_density
        } else {
            p.support_density
        };
        let spacing = if flange {
            flow.spacing()
        } else {
            p.flow.spacing()
        };
        let raft_interface_angle = if p.raft_base_layers > 1 {
            c.angle
                + 90.0
                + if (c.raft_layers - p.raft_base_layers).is_multiple_of(2) {
                    90.0
                } else {
                    0.0
                }
        } else if p.raft_base_layers == 1 {
            c.angle + 180.0
        } else {
            90.0
        };
        let interface_angle = if c.style == SupportStyle::Grid && !raft {
            c.angle + 90.0
        } else {
            (if raft { raft_interface_angle } else { 0.0 })
                + if layer.interface_id & 1 == 0 {
                    45.0
                } else {
                    -45.0
                }
        };
        let angle = if flange {
            if raft && p.raft_base_layers == 1 {
                c.angle
            } else {
                c.angle + 90.0
            }
        } else if interface {
            interface_angle
        } else {
            c.angle
                + if c.pattern == SupportPattern::RectilinearGrid && index & 1 == 1 {
                    90.0
                } else {
                    0.0
                }
        };
        // Organic trunks are hollow shells; their first-layer flange still uses dense fill.
        if c.style == SupportStyle::Organic && !interface && !flange && !raft {
            // Orca doubles every organic wall above 100 mm (generate_support_toolpaths).
            let double_wall = if c.orca_semantics && layer.print_z > 100.0 {
                // Area threshold of 0.1 scaled units squared as a diameter.
                2.0 * (0.1 / std::f64::consts::PI).sqrt() / SCALE
            } else {
                c.tree.branch_diameter_double_wall
            };
            paths.extend(super::tree_paths::generate(
                &region.polygons,
                flow,
                double_wall,
                c.orca_semantics,
                &mut support_groups,
                paths.len(),
            ));
            continue;
        }
        let sheath = flange || (!interface && (c.with_sheath || c.style == SupportStyle::Tree));
        let mut polygons = region.polygons.clone();
        if sheath {
            let centres = offset_ex(&polygons, -0.5 * spacing * SCALE, Join::Round);
            for ex in &centres {
                for poly in ex.polygons() {
                    if poly.len() < 3 {
                        continue;
                    }
                    let mut points = poly.clone();
                    points.push(points[0]);
                    paths.push(ExtrusionPath {
                        role,
                        points,
                        flow,
                        overhang: None,
                        perimeter_index: None,
                        seam: None,
                        brick: None,
                    });
                    if !c.orca_semantics && !flange {
                        support_groups.push(paths.len() - 1..paths.len());
                    }
                }
            }
            polygons = offset_ex(&polygons, -spacing * SCALE, Join::Round);
        }
        if interface
            && !base_interface
            && !flange
            && (c.interface_pattern == InterfacePattern::Concentric
                || (c.interface_pattern == InterfacePattern::Auto && c.top_gap == 0.0))
        {
            // Concentric contact fill; sparse concentric follows the interface line spacing.
            let mut loops = offset_ex(&polygons, -0.5 * spacing * SCALE, Join::Round);
            while !loops.is_empty() {
                for ex in &loops {
                    for poly in ex.polygons() {
                        let mut points = poly.clone();
                        points.push(points[0]);
                        paths.push(ExtrusionPath {
                            role,
                            points,
                            flow,
                            overhang: None,
                            perimeter_index: None,
                            seam: None,
                            brick: None,
                        });
                    }
                }
                loops = offset_ex(&loops, -scale(spacing / density) as f64, Join::Round);
            }
            continue;
        }
        let params = FillParams {
            spacing,
            density,
            angle,
            layer_id: 0,
            z: layer.print_z,
            anchor_length: 1000.0,
            anchor_length_max: 1000.0,
            dont_adjust: true,
            link_max_length: 0.0,
            origin,
        };
        let pattern = if flange
            || (sheath && (c.style != SupportStyle::Tree || c.with_sheath))
            || density > 0.95
            || (base_interface && c.with_sheath)
        {
            Pattern::AlignedRectilinear
        } else {
            Pattern::SupportBase
        };
        for filled in fill::fill(pattern, &polygons, &params) {
            paths.extend(filled.polylines.into_iter().map(|points| ExtrusionPath {
                overhang: None,
                perimeter_index: None,
                seam: None,
                role,
                points,
                flow,
                brick: None,
            }));
        }
        if flange && !c.orca_semantics && paths.len() > region_start {
            support_groups.push(region_start..paths.len());
        }
    }
    LayerToolpaths {
        support_groups,
        index,
        print_z: layer.print_z,
        height: layer.height,
        paths,
    }
}
