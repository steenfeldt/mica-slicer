// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupport.cpp :: generate_overhangs, generate_support_areas
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp :: layer_initialize
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportCommon.cpp :: generate_interface_layers
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportParameters.cpp :: SupportParameters
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupport3D.cpp :: generate_support_areas
// Source: orca@8500fcdcca src/libslic3r/Support/SupportParameters.hpp :: SupportParameters
// Changes: can_merge_support_regions from the object's extruders (upstream's zero-based/one-based comparison kept);
// Changes: PrusaSlicer's bottom base interface layers follow the soluble (zero top gap) interface; base interfaces are
// Changes: their own region kind, filled with the interface pattern in the base material.
// Changes: Shared settings switch and SupportLayer adapter; compact intermediate slabs retain precomputed organic roofs; reuse Mica raft/toolpath backend; per-layer overhangs and bed clipping in parallel.
// Copyright (c) Prusa Research 2016 - 2023 Pavel Mikuš, Vojtěch Bubník, Lukáš Matěna, Tomáš Mészáros
// Tree supports by Thomas Rahm, loosely based on CuraEngine tree supports.
// Copyright (c) 2021 Ultimaker B.V.

use super::super::{
    SupportError, SupportInput, SupportLayer, SupportLayerKind as Kind, SupportParameters,
    SupportRegion, TreeStatistics, common,
};
use super::{smoothing::smooth_outward, *};
use crate::{
    config::Config,
    geometry::{ExPolygon, Point, SCALE, clip},
    progress::{self, Observer, Stage},
    settings::PrintSettings,
};
use rayon::prelude::*;

/// `PrintObject::object_extruders` from the config's role tools (zero based, sorted, unique).
fn object_extruders(cfg: &Config) -> Vec<usize> {
    let mut out: Vec<usize> = [
        "perimeter_extruder",
        "infill_extruder",
        "solid_infill_extruder",
    ]
    .iter()
    .map(|key| (cfg.int(key).unwrap_or(1).max(1) - 1) as usize)
    .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Resolved for every print. Kept with PrintSettings so GUI and CLI
/// share exactly the same config reader and retain raw Orca organic options.
#[derive(Clone, Debug, PartialEq)]
pub struct Tree3dConfig {
    pub mesh: TreeSupportMeshGroupSettings,
    pub bed_area: Areas,
    pub top_base_layers: usize,
    pub bottom_base_layers: usize,
    pub bottom_layers: usize,
    pub angle: f32,
    pub support_ironing: bool,
    pub with_sheath: bool,
    /// Orca support_bottom_interface_spacing (mm); bottom interface island radius.
    pub bottom_interface_spacing: f64,
    /// Configured bottom interface layers are nonzero (after -1 resolves to top).
    pub bottom_interfaces: bool,
    /// SupportParameters::can_merge_support_regions: one tool prints base and interface.
    pub can_merge_support_regions: bool,
    /// Configured one-based support base and interface tools (0: the layer's tool).
    pub support_tools: (usize, usize),
}
impl Tree3dConfig {
    /// `SupportParameters::can_merge_support_regions`: base and interface use the same tool, or
    /// one is "don't care" and the object prints with exactly one extruder equal to the other.
    /// Upstream compares the zero-based `PrintObject::object_extruders` with the one-based
    /// configured tool; that comparison is kept as is.
    pub(crate) fn can_merge(base: usize, interface: usize, object_extruders: &[usize]) -> bool {
        base == interface
            || ((base == 0 || interface == 0)
                && object_extruders.len() == 1
                && object_extruders[0] == base.max(interface))
    }

    /// Recompute [`Self::can_merge_support_regions`] for an object's zero-based extruders.
    pub fn set_object_extruders(&mut self, object_extruders: &[usize]) {
        let (base, interface) = self.support_tools;
        self.can_merge_support_regions = Self::can_merge(base, interface, object_extruders);
    }

    pub(crate) fn double_wall_diameter(&self) -> f64 {
        2. * (self.mesh.tree_branch_diameter_double_wall_area_scaled / std::f64::consts::PI).sqrt()
            / SCALE
    }

    pub(crate) fn from_resolved(
        cfg: &Config,
        s: &PrintSettings,
    ) -> Result<Self, crate::resolve::ResolveError> {
        let mesh = TreeSupportMeshGroupSettings::from_resolved(cfg, s)?;
        let top = mesh.support_roof_layers.max(0) as usize;
        let orca = mesh.rules.is_orca();
        let bottom_raw = if orca {
            cfg.raw("support_interface_bottom_layers")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(s.support_material_bottom_interface_layers)
        } else {
            s.support_material_bottom_interface_layers
        };
        let bottom = if bottom_raw < 0 {
            top
        } else {
            bottom_raw as usize
        };
        let tool = |prusa: &str, orca_key: &str| {
            if orca {
                cfg.raw(orca_key)
                    .and_then(|v| v.parse::<usize>().ok())
                    .or_else(|| cfg.int(prusa).map(|v| v.max(0) as usize))
                    .unwrap_or(0)
            } else {
                cfg.int(prusa).unwrap_or(0).max(0) as usize
            }
        };
        let interface = tool(
            "support_material_interface_extruder",
            "support_interface_filament",
        );
        let base = tool("support_material_extruder", "support_filament");
        let soluble =
            |tool: usize| tool > 0 && cfg.bool_at("filament_soluble", tool - 1).unwrap_or(false);
        let special = soluble(interface) && !soluble(base);
        let zero_top = mesh.support_top_distance < scaled(0.0001);
        let zero_bottom = mesh.support_bottom_distance < scaled(0.0001);
        let base_count = |count: usize, zero: bool| {
            if zero && special {
                (count / 2).min(2)
            } else if orca && zero && interface != 0 && interface != base {
                1
            } else {
                0
            }
        };
        Ok(Self {
            bed_area: cfg
                .points("bed_shape")
                .map(|pts| {
                    vec![ExPolygon {
                        contour: pts.iter().map(|&(x, y)| Point::from_mm(x, y)).collect(),
                        holes: vec![],
                    }]
                })
                .unwrap_or_default(),
            top_base_layers: base_count(top, zero_top),
            // PrusaSlicer's soluble interface is a zero top gap for both (SupportParameters);
            // Orca checks each gap.
            bottom_base_layers: base_count(bottom, if orca { zero_bottom } else { zero_top }),
            bottom_layers: if orca {
                bottom
            } else {
                bottom.saturating_sub(1)
            },
            angle: (s.support_material_angle as f32).to_radians(),
            support_ironing: orca && cfg.bool("support_ironing").unwrap_or(false),
            with_sheath: if orca {
                cfg.raw("tree_support_wall_count")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
                    > 0
            } else {
                s.support_material_with_sheath
            },
            bottom_interfaces: bottom > 0,
            can_merge_support_regions: Self::can_merge(base, interface, &object_extruders(cfg)),
            support_tools: (base, interface),
            bottom_interface_spacing: cfg
                .raw("support_bottom_interface_spacing")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(
                    (mesh.support_roof_line_distance - mesh.support_roof_line_width) as f64 / SCALE,
                ),
            mesh,
        })
    }
}
fn overhangs(input: &SupportInput<'_>, c: &TreeSupportSettings) -> Vec<Areas> {
    let pad = c.raft_layers.len();
    let mut out = vec![vec![]; input.layers.len() + pad];
    // Every layer reads only its own and the layer below's outlines.
    let areas: Vec<Areas> = (1..input.layers.len())
        .into_par_iter()
        .map(|i| overhang_layer(input, c, i))
        .collect();
    for (i, area) in areas.into_iter().enumerate() {
        out[pad + i + 1] = area;
    }
    out
}
fn overhang_layer(input: &SupportInput<'_>, c: &TreeSupportSettings, i: usize) -> Areas {
    let cfg = input.config;
    {
        let layer = &input.layers[i];
        let lower = &input.layers[i - 1];
        let enforced = i < cfg.enforce_layers;
        let (enforcers, blockers) = input.masks.map_or((&[][..], &[][..]), |(a, b)| {
            (a[i].as_slice(), b[i].as_slice())
        });
        let mut area = if (cfg.enabled && cfg.automatic) || enforced {
            let offset = if enforced {
                0.
            } else if cfg.threshold == 0. {
                0.5 * cfg.external_width
            } else {
                lower.height / (cfg.threshold + 1.).to_radians().tan()
            };
            let mut a = clip::diff_ex(
                layer.outlines,
                &clip::offset_ex(lower.outlines, offset * SCALE, clip::Join::Miter),
            );
            if !enforced {
                a = clip::diff_ex_safety(&a, blockers);
            }
            // An empty overhang stays empty: only a non-empty one needs the coverage.
            if cfg.dont_support_bridges
                && !a.is_empty()
                && let Some(bridges) = layer.bridges
            {
                a = clip::diff_ex(&a, bridges.get());
            }
            a
        } else {
            vec![]
        };
        if !enforcers.is_empty() {
            let forced =
                clip::intersection_ex(&clip::diff_ex(layer.outlines, lower.outlines), enforcers);
            let forced = clip::diff_ex(
                &clip::offset_ex(
                    &forced,
                    c.settings.support_tree_tip_diameter as f64,
                    clip::Join::Miter,
                ),
                lower.outlines,
            );
            area.extend(forced);
        }
        clip::union_ex(&area)
    }
}
/// Active organic equivalent of finalize_interface_and_support_areas followed
/// by SupportCommon::generate_interface_layers. Precomputed roofs are never
/// clipped to sparse branch base (they may bridge several tips upstream).
pub(super) fn finalize_interface_and_support_areas(
    mut draw: TreeDrawing,
    c: &TreeSupportSettings,
    slicing: &TreeSlicingParameters,
    infill: &TreeSupportInfill,
    resolved: &Tree3dConfig,
) -> Vec<SupportLayer> {
    let n = draw.base.len();
    let compact: Vec<_> = (0..n).filter(|&i| !draw.base[i].is_empty()).collect();
    let smoothing = scaled(infill.interface_spacing * 1.5);
    let island = scaled(infill.interface_spacing / infill.interface_density.max(1e-6));
    let orca = c.settings.rules.is_orca();
    let island_top = scaled(infill.interface_spacing / infill.top_interface_density.max(1e-6));
    let island_bottom = scaled(
        infill.interface_spacing
            / (infill.interface_spacing
                / (resolved.bottom_interface_spacing + infill.interface_spacing))
                .clamp(1e-6, 1.),
    );
    let mut layers = Vec::with_capacity(n);
    for i in 0..n {
        let mut base = std::mem::take(&mut draw.base[i]);
        let mut interface = draw.roofs.top_interfaces[i].clone();
        let mut interface_kind = if interface.is_empty() {
            Kind::BottomInterface
        } else {
            Kind::TopInterface
        };
        let mut base_interface = draw.roofs.top_base_interfaces[i].clone();
        let mut bottom_projection = Vec::new();
        let mut top_projection = Vec::new();
        let mut lower_base = Vec::new();
        let mut upper_base = Vec::new();
        if let Ok(k) = compact.binary_search(&i) {
            let top_count = infill.num_top_interface_layers;
            let top_only = top_count.saturating_sub(infill.num_top_base_interface_layers);
            if top_count > 0 {
                let hi = compact[(k + top_count - 1).min(compact.len() - 1)];
                let only_hi = if top_only > 0 {
                    Some(compact[(k + top_only - 1).min(compact.len() - 1)])
                } else {
                    None
                };
                for j in i..n {
                    let bottom_z = if j == 0 {
                        0.
                    } else {
                        c.layer_z(slicing, j - 1)
                    };
                    if bottom_z - 1e-4 > c.layer_z(slicing, hi) {
                        break;
                    }
                    let is_base = infill.num_top_base_interface_layers > 0
                        && only_hi.is_none_or(|h| bottom_z - 1e-4 > c.layer_z(slicing, h));
                    if is_base {
                        upper_base.extend_from_slice(&draw.roofs.top_contacts[j]);
                    } else {
                        top_projection.extend_from_slice(&draw.roofs.top_contacts[j]);
                    }
                }
            }
            let bottom_count = resolved.bottom_layers;
            let bottom_only = bottom_count.saturating_sub(resolved.bottom_base_layers);
            if bottom_count > 0 {
                let lo = compact[(k + 1).saturating_sub(bottom_count)];
                let lo_z = if lo == 0 {
                    0.
                } else {
                    c.layer_z(slicing, lo - 1)
                };
                let only_z = if bottom_only == 0 {
                    f64::MAX
                } else {
                    let at = compact[k.saturating_sub(bottom_only)];
                    if at == 0 {
                        0.
                    } else {
                        c.layer_z(slicing, at - 1)
                    }
                };
                let current_bottom = if i == 0 {
                    0.
                } else {
                    c.layer_z(slicing, i - 1)
                };
                for j in 0..i {
                    let z = c.layer_z(slicing, j);
                    if z < lo_z - 1e-4 || z - 1e-4 > current_bottom {
                        continue;
                    }
                    if resolved.bottom_base_layers > 0 && z - 1e-4 <= only_z {
                        lower_base.extend_from_slice(&draw.bottom_contacts[j]);
                    } else {
                        bottom_projection.extend_from_slice(&draw.bottom_contacts[j]);
                    }
                }
            }
            let close = |a: Areas, island: i64| {
                if a.is_empty() {
                    return a;
                }
                smooth_outward(
                    clip::offset2_ex(
                        &a,
                        (smoothing + island) as f64,
                        -(smoothing as f64),
                        clip::Join::Square,
                    ),
                    smoothing,
                )
            };
            // Prusa regularizes merged top+bottom projections with one island radius;
            // Orca regularizes them separately with top/bottom interface densities.
            let regularize = |mut bottom: Areas, top: Areas| {
                let closed = if orca {
                    let mut closed = close(bottom, island_bottom);
                    closed.extend(close(top, island_top));
                    clip::union_ex(&closed)
                } else {
                    bottom.extend(top);
                    close(bottom, island)
                };
                clip::intersection_ex(&closed, &base)
            };
            if !top_projection.is_empty() {
                interface_kind = Kind::TopInterface;
            }
            interface.extend(regularize(bottom_projection, top_projection));
            interface = clip::union_ex(&interface);
            base_interface.extend(regularize(lower_base, upper_base));
            base_interface = clip::diff_ex(&clip::union_ex(&base_interface), &interface);
            base = clip::diff_ex(&base, &[interface.clone(), base_interface.clone()].concat());
        }
        let z = c.layer_z(slicing, i);
        let bottom = if i == 0 {
            0.
        } else {
            c.layer_z(slicing, i - 1)
        };
        // SupportCommon::generate_support_toolpaths merges same-height contact and
        // interface layers before filling them, so overlapping regions print once.
        let mut top_contact = draw.roofs.top_contacts[i].clone();
        let mut bottom_contact = draw.bottom_contacts[i].clone();
        let merge = |a: &mut Areas, b: Areas| {
            a.extend(b);
            *a = clip::union_ex(a);
        };
        if resolved.mesh.support_roof_layers == 0 {
            if resolved.can_merge_support_regions && !top_contact.is_empty() {
                merge(&mut base, std::mem::take(&mut top_contact));
            }
        } else if !top_contact.is_empty() && !interface.is_empty() {
            merge(&mut top_contact, std::mem::take(&mut interface));
        }
        if (resolved.mesh.support_roof_layers == 0 || !resolved.bottom_interfaces)
            && resolved.can_merge_support_regions
        {
            if !bottom_contact.is_empty() {
                merge(&mut base, std::mem::take(&mut bottom_contact));
            }
        } else if !bottom_contact.is_empty() && !top_contact.is_empty() {
            merge(&mut top_contact, std::mem::take(&mut bottom_contact));
        } else if !bottom_contact.is_empty() && !interface.is_empty() && !orca {
            // Orca skips this merge for organic trees.
            merge(&mut bottom_contact, std::mem::take(&mut interface));
        }
        let mut regions = Vec::new();
        for (kind, polygons) in [
            (Kind::BottomContact, bottom_contact),
            (Kind::TopContact, top_contact),
            (interface_kind, interface),
            (Kind::BaseInterface, base_interface),
            (Kind::Base, base),
        ] {
            if !polygons.is_empty() {
                regions.push(SupportRegion { kind, polygons });
            }
        }
        layers.push(SupportLayer {
            print_z: z,
            height: z - bottom,
            interface_id: 0,
            regions,
        });
    }
    layers
}
pub(crate) fn generate(
    input: &SupportInput<'_>,
    p: &SupportParameters,
    observer: &dyn Observer,
) -> Result<(Vec<SupportLayer>, TreeStatistics), SupportError> {
    let resolved = p.config.tree3d.as_ref().expect("tree3d selected");
    let mut slicing =
        TreeSlicingParameters::new(p.config.layer_height, p.config.first_layer_height);
    slicing.object_print_z_min = p.object_z_offset;
    slicing.origin = input.origin;
    slicing.max_support_layer_height = p.max_height;
    slicing.base_raft_layers = p.raft_base_layers;
    slicing.interface_raft_layers = p.config.raft_layers - p.raft_base_layers;
    slicing.base_raft_layer_height = p.raft_height;
    slicing.interface_raft_layer_height = p.raft_height;
    slicing.raft_contact_top_z = p.raft_top_z;
    let c = TreeSupportSettings::new(
        &resolved.mesh,
        &slicing,
        resolved.mesh.support_top_distance < scaled(0.0001),
    );
    let overhangs = overhangs(input, &c);
    let outlines = input.layers.iter().map(|l| l.outlines.to_vec()).collect();
    let blockers = input.masks.map_or_else(Vec::new, |(_, b)| {
        let mut a = vec![vec![]; c.raft_layers.len()];
        a.extend_from_slice(b);
        a
    });
    let mut v = TreeModelVolumes::new(c.clone(), outlines, blockers, resolved.bed_area.clone());
    // TreeSupport.cpp precalculate(): eager volumes up to the highest tip layer. This
    // also fixes the ignorable radii, and thereby the radius buckets, as upstream.
    let max_layer = (c.raft_layers.len().max(1)..overhangs.len())
        .rev()
        .find(|&l| !overhangs[l].is_empty())
        .map_or(0, |l| l.saturating_sub(c.z_distance_top_layers));
    if max_layer > 0 {
        let _timer = crate::timing::Span::new(crate::timing::Stage::SupportVolumes);
        v.precalculate_with_cancel(max_layer, || progress::check(observer))
            .map_err(SupportError)?;
    }
    let mut infill = TreeSupportInfill::new(&c);
    infill.base_angle = resolved.angle;
    infill.interface_angle = resolved.angle + std::f32::consts::FRAC_PI_2;
    infill.num_top_base_interface_layers = resolved.top_base_layers;
    infill.origin = input.origin;
    infill.top_interface_density = if resolved.support_ironing {
        1.
    } else {
        infill.interface_density
    };
    // SupportParameters computes the sampling pattern before generate_support_areas
    // overrides with_sheath. Preserve the constructor's original selection.
    if resolved.with_sheath && c.support_pattern != TreeSupportPattern::Honeycomb {
        infill.base_pattern = crate::fill::Pattern::Rectilinear;
    }
    let raft = (!c.raft_layers.is_empty()).then(|| TreeRaftContact {
        contact_layer: p.config.raft_layers - 1,
        first_tree_layer: c.raft_layers.len(),
        outline: input.layers[0].outlines.to_vec(),
        expansion: scaled(p.config.raft_expansion),
    });
    let check = || progress::check(observer).map_err(SupportError);
    check()?;
    observer.progress(Stage::Supports, 0.2);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportTips);
    let mut pathing =
        generate_initial_areas_with_cancel(&mut v, &c, &overhangs, &infill, raft.as_ref(), check)?;
    drop(timer);
    observer.progress(Stage::Supports, 0.4);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportPathing);
    create_layer_pathing_with_cancel(&v, &c, &mut pathing.move_bounds, check)?;
    observer.progress(Stage::Supports, 0.7);
    create_nodes_from_area_with_cancel(&v, &c, &mut pathing.move_bounds, check)?;
    drop(timer);
    let mut stats = TreeStatistics::default();
    let mut has_child: Vec<_> = pathing
        .move_bounds
        .iter()
        .map(|l| vec![false; l.len()])
        .collect();
    for (l, elems) in pathing.move_bounds.iter().enumerate() {
        for e in elems {
            if e.parents.is_empty() {
                stats.tips += 1;
                stats.supported_tips += 1;
            }
            for &j in &e.parents {
                has_child[l + 1][j as usize] = true;
            }
        }
    }
    for (l, children) in has_child.iter().enumerate() {
        for &child in children {
            if !child {
                if l == 0 {
                    stats.bed_trunks += 1;
                } else {
                    stats.model_trunks += 1;
                }
            }
        }
    }
    observer.progress(Stage::Supports, 0.8);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportDraw);
    let draw = organic_draw_branches(&mut v, &c, &slicing, &infill, pathing, check)?;
    drop(timer);
    let timer = crate::timing::Span::new(crate::timing::Stage::SupportFinalize);
    let mut layers = finalize_interface_and_support_areas(draw, &c, &slicing, &infill, resolved);
    drop(timer);
    // Raft geometry and flange expansion remain in Mica's existing common backend.
    if p.config.raft_layers > 0 {
        layers.retain(|l| l.print_z > p.raft_top_z + 1e-6);
    }
    common::generate_raft_base(input, &mut layers, p);
    // Orca clips every generated layer after raft/interface assembly (#4769).
    if c.settings.rules.is_orca() {
        layers.par_iter_mut().for_each(|layer| {
            for region in &mut layer.regions {
                region.polygons = clip::intersection_ex(&region.polygons, &resolved.bed_area);
            }
        });
    }
    check()?;
    observer.progress(Stage::Supports, 0.9);
    Ok((layers, stats))
}
