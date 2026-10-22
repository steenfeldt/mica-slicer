// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PrintApply.cpp :: generate_print_object_regions
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObject.cpp :: region_config_from_model_volume
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: slices_to_regions, slice_support_volumes
// Changes: Ordered polygon booleans and interned settings (layers in parallel, table filled in layer order); painted facet regions; no cached volume bounding boxes.
// Source: prusaslicer@b028299c77 src/libslic3r/Feature/FullSpectrum/VirtualExtruder.cpp :: remap_virtual_region_slices_to_physical, remap_virtual_extruders_to_physical
// Changes: Retain logical virtual regions through volume/modifier clipping, then intern physical targets per object-local layer before path generation; Mica's mix patterns resolve walls and inside separately there, and gradients over each mix's layer span in the object.
// Upstream attribution (PrintApply.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Enrico Turri @enricoturri1966, Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Oleksandra Iushchenko @YuSanka, Lukáš Matěna @lukasmatena, Lukáš Hejl @hejllukas, Roman Beránek @zavorka
// Source: prusaslicer@b028299c77 src/libslic3r/PrintObjectSlice.cpp :: slice_volumes
// Changes: Spiral vase: model parts sliced to their largest outline (PositiveLargestContour) above the bottom layers of their own settings.
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Upstream attribution (PrintObject.cpp):
// Copyright (c) Prusa Research 2016 - 2023 Lukáš Hejl @hejllukas, Pavel Mikuš @Godrak, Lukáš Matěna @lukasmatena, Vojtěch Bubník @bubnikv, Enrico Turri @enricoturri1966, Oleksandra Iushchenko @YuSanka, David Kocík @kocikdav, Roman Beránek @zavorka
// Copyright (c) 2021 Justin Schuh @jschuh
// Copyright (c) 2021 Ilya @xorza
// Copyright (c) 2016 Joseph Lenox @lordofhyphens
// Copyright (c) Slic3r 2014 - 2016 Alessandro Ranellucci @alranel
// Copyright (c) 2015 Maksim Derbasov @ntfshard
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//
// Upstream attribution (PrintObjectSlice.cpp):
// Copyright (c) Prusa Research 2021 - 2023 Oleksandra Iushchenko @YuSanka, Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak, Lukáš Hejl @hejllukas
//
// PrusaSlicer is released under the terms of the AGPLv3 or higher
//

//! Object/part/range configuration and indexed layer geometry. All meshes are in bed coordinates.
mod pipeline;
mod schedule;
mod surfaces;
pub(crate) use schedule::layer_tops;
pub mod combine;
pub(crate) use pipeline::PreparedRegions;

use crate::config::Config;
use crate::geometry::ExPolygon;
use crate::geometry::clip::{Join, diff_ex, intersection_ex, offset_ex, union_ex};
use crate::mesh::TriangleMesh;
use crate::print_job::{ToolAssignments, tool_config};
use crate::settings::PrintSettings;
use crate::threemf::PartKind;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct Volume {
    pub mesh: TriangleMesh,
    pub painting: crate::painting::FacetPainting,
    pub kind: PartKind,
    /// Sparse canonical overrides. Object settings belong in the preparation's base config.
    pub config: Config,
}
/// Spiral vase: a model part is sliced to its largest outline, holes closed, above the bottom
/// layers its own (region) settings ask for (`slice_volumes`' PositiveLargestContour; layer
/// height ranges are not combined with spiral vase upstream).
fn spiral_largest_contours(
    volume: &Volume,
    base: &Config,
    zs: &[f64],
    slices: &mut [crate::slice::LayerSlice],
) {
    if volume.kind != PartKind::Model || !crate::spiral_vase::enabled(base) {
        return;
    }
    let config = apply_config(base, &volume.config);
    let from = crate::spiral_vase::slice_from_layer_config(&config, zs);
    for slice in slices.iter_mut().skip(from) {
        slice.islands = crate::spiral_vase::largest_contour(&slice.islands);
    }
}

/// The mesh a volume is sliced from: its displaced mesh when it has one.
fn sliced_mesh<'a>(
    volume: &'a Volume,
    index: usize,
    displaced: Option<&'a [Option<TriangleMesh>]>,
) -> &'a TriangleMesh {
    displaced
        .and_then(|d| d.get(index))
        .and_then(Option::as_ref)
        .unwrap_or(&volume.mesh)
}

#[derive(Clone, Debug)]
pub struct HeightRange {
    /// Object height in mm, after placement on the bed; half-open [min_z, max_z).
    pub min_z: f64,
    pub max_z: f64,
    pub config: Config,
}
#[derive(Clone, Debug, Default)]
pub struct RegionModel {
    pub full_spectrum: crate::full_spectrum::FullSpectrum,
    /// Archive volume order is significant: later solids/negatives clip earlier volumes.
    pub volumes: Vec<Volume>,
    pub height_ranges: Vec<HeightRange>,
    /// Variable layer height profile, `(z, height)` pairs from the object's bottom to its top
    /// ([`crate::layer_profile`]). Empty: constant layers (and `height_ranges`). A profile
    /// that does not end at the object's top is ignored, as upstream does.
    pub layer_height_profile: Vec<f64>,
    /// Hand-placed brim ears in bed coordinates, used with `brim_type = painted`.
    pub brim_ears: Vec<crate::brim::BrimEar>,
}

pub struct PrintRegion {
    pub settings: PrintSettings,
    pub tools: ToolAssignments,
    pub config: Config,
    pub(crate) by_tool: Vec<(Config, PrintSettings)>,
}
#[derive(Clone, Debug)]
pub struct LayerRegion {
    /// One-based facet material; remains more specific than parameter modifiers.
    pub painted_tool: Option<u8>,
    pub region: usize,
    pub slices: Vec<ExPolygon>,
    pub surfaces: crate::shells::FillSurfaces,
}

pub(crate) struct RegionSlices {
    pub outlines: Vec<Vec<ExPolygon>>,
    pub table: Vec<PrintRegion>,
    pub layers: Vec<Vec<LayerRegion>>,
    pub enforcers: Vec<Vec<ExPolygon>>,
    pub blockers: Vec<Vec<ExPolygon>>,
    pub dropped_chains: usize,
}

/// Apply a sparse override; a volume's `extruder` resets all three model role defaults.
/// Explicit role assignments in that same override remain more specific.
pub fn apply_config(parent: &Config, patch: &Config) -> Config {
    let mut out = parent.clone();
    if let Some(tool) = patch
        .raw("extruder")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|t| *t > 0)
    {
        for key in [
            "perimeter_extruder",
            "infill_extruder",
            "solid_infill_extruder",
        ] {
            out.set_raw(key, &tool.to_string()).expect("integer tool");
        }
    }
    out.apply(patch);
    out
}

fn region_config(parent: &Config, patch: &Config, object: &Config) -> Config {
    let mut out = apply_config(parent, patch);
    for key in [
        "layer_height",
        "first_layer_height",
        "infill_every_layers",
        "solid_infill_every_layers",
        "xy_hole_compensation",
        "xy_contour_compensation",
    ] {
        if let Some(value) = object.get(key) {
            out.set(key, value.clone());
        }
    }
    out
}

impl RegionModel {
    pub fn validate(&self) -> Result<(), String> {
        for config in self
            .volumes
            .iter()
            .map(|v| &v.config)
            .chain(self.height_ranges.iter().map(|r| &r.config))
        {
            if let Some(raw) = config.raw("extruder")
                && raw.parse::<usize>().is_err()
            {
                return Err(format!("invalid extruder={raw:?}"));
            }
            if let Some(h) = config.float("layer_height")
                && (!h.is_finite() || h <= 0.)
            {
                return Err("layer_height must be finite and positive".into());
            }
        }
        for v in &self.volumes {
            crate::safety::mesh(&v.mesh)?;
            for states in [
                &v.painting.supports,
                &v.painting.seam,
                &v.painting.material,
                &v.painting.fuzzy,
            ] {
                states.validate_mesh(&v.mesh)?;
            }
        }
        let mut end = 0.;
        for range in &self.height_ranges {
            if !range.min_z.is_finite()
                || !range.max_z.is_finite()
                || range.min_z < end
                || range.max_z <= range.min_z
            {
                return Err(
                    "height ranges must be finite, sorted, nonnegative and non-overlapping".into(),
                );
            }
            end = range.max_z;
        }
        if self.layer_height_profile.iter().any(|v| !v.is_finite())
            || self
                .layer_height_profile
                .chunks(2)
                .any(|c| c.len() == 2 && c[1] <= 0.)
        {
            return Err("layer height profile must be finite with positive heights".into());
        }
        if self.brim_ears.iter().any(|e| {
            !e.x.is_finite() || !e.y.is_finite() || !e.z.is_finite() || !e.radius.is_finite()
        }) {
            return Err("brim ears must be finite".into());
        }
        if !self
            .volumes
            .iter()
            .any(|v| v.kind == PartKind::Model && !v.mesh.faces.is_empty())
        {
            return Err("no model volumes".into());
        }
        Ok(())
    }

    /// One layer of one volume in [`Self::slice`]. New regions get `PENDING + k` indices for
    /// the k-th entry of `interns`, the settings in the order a serial pass interns them.
    #[allow(clippy::too_many_arguments)]
    fn slice_layer(
        &self,
        volume: &Volume,
        z: f64,
        base: &Config,
        table: &[PrintRegion],
        xy: f64,
        mut polygons: Vec<ExPolygon>,
        mut colored: Option<Vec<Vec<ExPolygon>>>,
        outline: &mut Vec<ExPolygon>,
        layer: &mut Vec<LayerRegion>,
        (enforcers, blockers): (&mut Vec<ExPolygon>, &mut Vec<ExPolygon>),
        interns: &mut Vec<Config>,
    ) -> Result<(), String> {
        let mut pending = |config: Config| {
            interns.push(config);
            PENDING + interns.len() - 1
        };
        if xy > 0. && matches!(volume.kind, PartKind::Model | PartKind::Modifier) {
            polygons = offset_ex(&polygons, xy * crate::geometry::SCALE, Join::Miter);
        }
        if polygons.is_empty() {
            return Ok(());
        }
        // Keep the model silhouette independent of material-boundary roundoff.
        // Support/bridge queries must not see nanometre gaps between colour regions.
        match volume.kind {
            PartKind::Model => {
                outline.extend(polygons.clone());
                *outline = union_ex(outline);
            }
            PartKind::NegativeVolume => *outline = diff_ex(outline, &polygons),
            _ => {}
        }
        match volume.kind {
            PartKind::Model => {
                let mut config = base.clone();
                if let Some(range) = self
                    .height_ranges
                    .iter()
                    .find(|r| z >= r.min_z && z < r.max_z)
                {
                    config = region_config(&config, &range.config, base);
                }
                config = region_config(&config, &volume.config, base);
                for old in layer.iter_mut() {
                    old.slices = diff_ex(&old.slices, &polygons);
                }
                if let Some(colored) = colored.as_mut() {
                    let mut colored = std::mem::take(colored);
                    let mut covered = vec![];
                    for area in colored.iter_mut().skip(1) {
                        *area = diff_ex(&intersection_ex(area, &polygons), &covered);
                        covered.extend(area.clone());
                    }
                    colored[0] = diff_ex(&polygons, &covered);
                    for (color, area) in crate::painting::segmentation::nonempty(colored) {
                        let mut painted = config.clone();
                        if color > 0 {
                            for key in [
                                "perimeter_extruder",
                                "infill_extruder",
                                "solid_infill_extruder",
                            ] {
                                painted
                                    .set_raw(key, &color.to_string())
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                        let region = pending(painted);
                        layer.push(LayerRegion {
                            region,
                            painted_tool: (color > 0).then_some(color),
                            slices: intersection_ex(&area, &polygons),
                            surfaces: Default::default(),
                        });
                    }
                } else {
                    let region = pending(config);
                    layer.push(LayerRegion {
                        region,
                        painted_tool: None,
                        slices: polygons,
                        surfaces: Default::default(),
                    });
                }
            }
            PartKind::NegativeVolume => {
                for old in layer.iter_mut() {
                    old.slices = diff_ex(&old.slices, &polygons);
                }
            }
            PartKind::Modifier => {
                let mut added = vec![];
                for old in layer.iter_mut() {
                    let overlap = intersection_ex(&old.slices, &polygons);
                    if overlap.is_empty() {
                        continue;
                    }
                    let mut config = region_config(&table[old.region].config, &volume.config, base);
                    if let Some(tool) = old.painted_tool {
                        for key in [
                            "perimeter_extruder",
                            "infill_extruder",
                            "solid_infill_extruder",
                        ] {
                            config
                                .set_raw(key, &tool.to_string())
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    // Interning finds the region's own entry exactly when the settings are
                    // equal (the table holds every setting once).
                    if config == table[old.region].config {
                        continue;
                    }
                    let region = pending(config);
                    old.slices = diff_ex(&old.slices, &polygons);
                    added.push(LayerRegion {
                        painted_tool: old.painted_tool,
                        region,
                        slices: overlap,
                        surfaces: Default::default(),
                    });
                }
                layer.extend(added);
            }
            PartKind::SupportEnforcer => enforcers.extend(polygons),
            PartKind::SupportBlocker => blockers.extend(polygons),
            PartKind::Unknown(_) => {}
        }
        layer.retain(|r| !r.slices.is_empty());
        Ok(())
    }
    /// The object outlines, support masks and dropped chains of [`Self::slice`], without
    /// the material segmentation and regions (its `layers` stay empty).
    ///
    /// `displaced` replaces the sliced mesh of a volume (textures on all surfaces); painting
    /// still reads the volume's own mesh.
    pub(crate) fn slice_outlines(
        &self,
        zs: &[f64],
        base: &Config,
        observer: &dyn crate::progress::Observer,
        displaced: Option<&[Option<TriangleMesh>]>,
    ) -> Result<RegionSlices, String> {
        self.validate()?;
        let mut out = RegionSlices {
            outlines: vec![vec![]; zs.len()],
            table: vec![],
            layers: vec![vec![]; zs.len()],
            enforcers: vec![vec![]; zs.len()],
            blockers: vec![vec![]; zs.len()],
            dropped_chains: 0,
        };
        let xy = base.float("xy_size_compensation").unwrap_or(0.);
        for (index, volume) in self.volumes.iter().enumerate() {
            let slices = if displaced.is_some_and(|d| d.get(index).is_some_and(Option::is_some)) {
                crate::texture::displace::slice(
                    sliced_mesh(volume, index, displaced),
                    zs,
                    crate::slice::closing_radius(base),
                    observer,
                )
            } else {
                crate::slice::slice_mesh_observed(
                    sliced_mesh(volume, index, displaced),
                    zs,
                    crate::slice::closing_radius(base),
                    crate::slice::SliceRounding::Nearest,
                    observer,
                )
            };
            let mut slices = slices;
            spiral_largest_contours(volume, base, zs, &mut slices);
            crate::progress::check(observer)?;
            if volume.kind == PartKind::Model && !volume.painting.supports.is_empty() {
                crate::painting::projection::support_masks(
                    &volume.mesh,
                    &volume.painting.supports,
                    zs,
                    &mut out.enforcers,
                    &mut out.blockers,
                );
            }
            crate::progress::check(observer)?;
            out.dropped_chains += slices.iter().map(|s| s.dropped_chains).sum::<usize>();
            slices
                .into_par_iter()
                .zip(out.outlines.par_iter_mut())
                .zip(
                    out.enforcers
                        .par_iter_mut()
                        .zip(out.blockers.par_iter_mut()),
                )
                .for_each(|((slice, outline), (enforcers, blockers))| {
                    let mut polygons = slice.islands;
                    if xy > 0. && matches!(volume.kind, PartKind::Model | PartKind::Modifier) {
                        polygons = offset_ex(&polygons, xy * crate::geometry::SCALE, Join::Miter);
                    }
                    if polygons.is_empty() {
                        return;
                    }
                    match volume.kind {
                        PartKind::Model => {
                            outline.extend(polygons);
                            *outline = union_ex(outline);
                        }
                        PartKind::NegativeVolume => *outline = diff_ex(outline, &polygons),
                        PartKind::SupportEnforcer => enforcers.extend(polygons),
                        PartKind::SupportBlocker => blockers.extend(polygons),
                        _ => {}
                    }
                });
        }
        out.enforcers
            .par_iter_mut()
            .chain(out.blockers.par_iter_mut())
            .for_each(|mask| *mask = union_ex(mask));
        Ok(out)
    }

    pub(crate) fn slice(
        &self,
        zs: &[f64],
        base: &Config,
        observer: &dyn crate::progress::Observer,
        displaced: Option<&[Option<TriangleMesh>]>,
    ) -> Result<RegionSlices, String> {
        let _timing = crate::timing::Span::new(crate::timing::Stage::Regions);
        self.validate()?;
        // One tick per volume and layer; the mesh slicing of each volume reports as the
        // slicing stage, the booleans and segmentation as the regions stage.
        let ticker = crate::progress::Ticker::new(
            observer,
            crate::progress::Stage::Regions,
            self.volumes.len() * zs.len(),
        );
        let mut out = RegionSlices {
            outlines: vec![vec![]; zs.len()],
            table: vec![],
            layers: vec![vec![]; zs.len()],
            enforcers: vec![vec![]; zs.len()],
            blockers: vec![vec![]; zs.len()],
            dropped_chains: 0,
        };
        // Slice one volume at a time: do not retain an additional volume × layer polygon matrix.
        for (index, volume) in self.volumes.iter().enumerate() {
            let slices = if displaced.is_some_and(|d| d.get(index).is_some_and(Option::is_some)) {
                crate::texture::displace::slice(
                    sliced_mesh(volume, index, displaced),
                    zs,
                    crate::slice::closing_radius(base),
                    observer,
                )
            } else {
                crate::slice::slice_mesh_observed(
                    sliced_mesh(volume, index, displaced),
                    zs,
                    crate::slice::closing_radius(base),
                    crate::slice::SliceRounding::Nearest,
                    observer,
                )
            };
            let mut slices = slices;
            spiral_largest_contours(volume, base, zs, &mut slices);
            crate::progress::check(observer)?;
            let mut colors =
                if volume.kind == PartKind::Model && !volume.painting.material.is_empty() {
                    Some(crate::painting::segmentation::segment_with_virtuals(
                        &volume.mesh,
                        &volume.painting.material,
                        zs,
                        &slices.iter().map(|s| s.islands.clone()).collect::<Vec<_>>(),
                        &apply_config(base, &volume.config),
                        &self.full_spectrum,
                    )?)
                } else {
                    None
                };
            if volume.kind == PartKind::Model && !volume.painting.supports.is_empty() {
                crate::painting::projection::support_masks(
                    &volume.mesh,
                    &volume.painting.supports,
                    zs,
                    &mut out.enforcers,
                    &mut out.blockers,
                );
            }
            crate::progress::check(observer)?;
            out.dropped_chains += slices.iter().map(|s| s.dropped_chains).sum::<usize>();
            if let Some(layers) = colors.as_mut() {
                self.full_spectrum.remap_paint(layers);
            }
            let colored_layers: Vec<Option<Vec<Vec<ExPolygon>>>> = match colors.as_mut() {
                Some(layers) => (0..slices.len())
                    .map(|i| Some(std::mem::take(&mut layers[i])))
                    .collect(),
                None => vec![None; slices.len()],
            };
            let xy = base.float("xy_size_compensation").unwrap_or(0.);
            let table = &out.table;
            // Layers are independent except for the region table: each layer records the
            // settings it would intern, in call order, and the table is filled serially in
            // layer order below, so region indices are those of a serial pass.
            let works: Vec<LayerWork> = slices
                .into_par_iter()
                .zip(colored_layers)
                .zip(out.outlines.par_iter_mut())
                .zip(out.layers.par_iter_mut())
                .zip(
                    out.enforcers
                        .par_iter_mut()
                        .zip(out.blockers.par_iter_mut()),
                )
                .enumerate()
                .map(
                    |(i, ((((slice, colored), outline), layer), (enforcers, blockers)))| {
                        let mut work = LayerWork::default();
                        ticker.tick();
                        if i.is_multiple_of(16)
                            && let Err(e) = crate::progress::check(observer)
                        {
                            work.error = Some(e);
                            return work;
                        }
                        if let Err(e) = self.slice_layer(
                            volume,
                            zs[i],
                            base,
                            table,
                            xy,
                            slice.islands,
                            colored,
                            outline,
                            layer,
                            (enforcers, blockers),
                            &mut work.interns,
                        ) {
                            work.error = Some(e);
                        }
                        work
                    },
                )
                .collect();
            for (layer, work) in out.layers.iter_mut().zip(works) {
                let mut ids = Vec::with_capacity(work.interns.len());
                for config in work.interns {
                    ids.push(intern(&mut out.table, config, &self.full_spectrum)?);
                }
                if let Some(e) = work.error {
                    return Err(e);
                }
                for r in layer.iter_mut() {
                    if r.region >= PENDING {
                        r.region = ids[r.region - PENDING];
                    }
                }
            }
        }
        if !self.full_spectrum.extruders().is_empty() {
            let logical = std::mem::take(&mut out.table);
            // A gradient spreads over the layers its mix occupies in this object.
            let logical = &logical;
            let spans = self
                .full_spectrum
                .spans(out.layers.iter().enumerate().flat_map(|(i, layer)| {
                    layer.iter().filter_map(move |r| {
                        self.full_spectrum
                            .source(&logical[r.region].config)
                            .map(|id| (id, i))
                    })
                }));
            for (i, layer) in out.layers.iter_mut().enumerate() {
                for r in layer {
                    let (physical, _) =
                        self.full_spectrum
                            .remap_config(&logical[r.region].config, i, Some(&spans));
                    r.region = intern(&mut out.table, physical, &Default::default())?;
                }
            }
        }
        out.layers.par_iter_mut().for_each(|layer| {
            for r in layer.iter_mut() {
                r.painted_tool = None;
            }
            layer.sort_by_key(|r| r.region);
            let mut merged: Vec<LayerRegion> = vec![];
            for r in std::mem::take(layer) {
                if let Some(last) = merged
                    .last_mut()
                    .filter(|last| last.region == r.region && last.painted_tool == r.painted_tool)
                {
                    last.slices.extend(r.slices);
                    last.slices = union_ex(&last.slices);
                } else {
                    merged.push(r);
                }
            }
            *layer = merged;
        });
        out.enforcers
            .par_iter_mut()
            .chain(out.blockers.par_iter_mut())
            .for_each(|mask| *mask = union_ex(mask));
        Ok(out)
    }
}

/// Region index placeholder `PENDING + k`: the k-th settings a layer asked to intern.
const PENDING: usize = usize::MAX / 2;
#[derive(Default)]
struct LayerWork {
    interns: Vec<Config>,
    error: Option<String>,
}

fn intern(
    table: &mut Vec<PrintRegion>,
    config: Config,
    spectrum: &crate::full_spectrum::FullSpectrum,
) -> Result<usize, String> {
    if let Some(i) = table.iter().position(|r| r.config == config) {
        return Ok(i);
    }
    ToolAssignments::resolve_with_virtuals(
        &config,
        &Default::default(),
        &Default::default(),
        0,
        config.vector_len("nozzle_diameter").max(1),
        spectrum,
    )?;
    // The intermediate table keeps logical assignments distinct for subsequent modifiers.
    // Settings are always resolved using a physical tool; the final table is entirely physical.
    let (physical, _) = spectrum.remap_config(&config, 0, None);
    let tools = ToolAssignments::resolve(
        &physical,
        &Default::default(),
        &Default::default(),
        0,
        config.vector_len("nozzle_diameter").max(1),
    )?;
    let settings = PrintSettings::from_config(&tool_config(&physical, tools.0[0].unwrap_or(0)))
        .map_err(|e| e.to_string())?;
    let by_tool = crate::print_job::resolved_tools(&physical)?;
    table.push(PrintRegion {
        settings,
        tools,
        config,
        by_tool,
    });
    Ok(table.len() - 1)
}
