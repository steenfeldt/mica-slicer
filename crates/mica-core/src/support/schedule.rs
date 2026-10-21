// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Thin integration adapter: merge independent support/object Z schedules, retaining object indices.
use super::{EPS, SupportConfig, SupportError, SupportInput, SupportObjectLayer, SupportPlan};
use crate::geometry::{BoundingBox, ExPolygon, Point};
use crate::toolpath::LayerToolpaths;

#[derive(Clone, Copy, Debug)]
pub struct PrintLayer {
    pub print_z: f64,
    pub height: f64,
    pub object: Option<usize>,
    pub support: Option<usize>,
}

/// Merge by physical print height, coalescing nearly equal Z. Object-layer IDs remain unchanged
/// for infill orientation and surface classification; output IDs count support-only layers too.
pub fn merge(tops: &[f64], heights: &[f64], supports: &SupportPlan) -> Vec<PrintLayer> {
    let mut layers = Vec::with_capacity(tops.len() + supports.layers.len());
    let (mut i, mut j) = (0, 0);
    while i < tops.len() || j < supports.layers.len() {
        let object_z = tops.get(i).copied().unwrap_or(f64::INFINITY);
        let support = supports.layers.get(j);
        let support_z = support.map_or(f64::INFINITY, |s| s.print_z);
        let both = (object_z - support_z).abs() < EPS;
        let take_object = both || object_z < support_z;
        let take_support = both || support_z < object_z;
        layers.push(PrintLayer {
            print_z: object_z.min(support_z),
            height: if take_object {
                heights[i]
            } else {
                support.unwrap().height
            },
            object: take_object.then_some(i),
            support: take_support.then_some(j),
        });
        i += usize::from(take_object);
        j += usize::from(take_support);
    }
    layers
}

pub fn toolpaths(
    index: usize,
    layer: PrintLayer,
    supports: &SupportPlan,
    object: impl FnOnce(usize) -> LayerToolpaths,
) -> LayerToolpaths {
    let mut result = LayerToolpaths {
        support_groups: Vec::new(),
        index,
        print_z: layer.print_z,
        height: layer.height,
        paths: Vec::new(),
    };
    if let Some(i) = layer.support {
        let support = supports.toolpaths(i);
        result.paths = support.paths;
        result.support_groups = support.support_groups;
    }
    if let Some(i) = layer.object {
        result.paths.extend(object(i).paths);
    }
    result
}

/// Adapt the pipeline's parallel outline arrays without retaining a borrow or cloning outlines.
pub fn prepare(
    tops: &[f64],
    heights: &[f64],
    outlines: &[Vec<ExPolygon>],
    config: &SupportConfig,
    origin: Point,
) -> Result<(SupportPlan, Vec<PrintLayer>), SupportError> {
    prepare_with_masks(tops, heights, outlines, config, origin, None)
}

pub fn prepare_with_masks(
    tops: &[f64],
    heights: &[f64],
    outlines: &[Vec<ExPolygon>],
    config: &SupportConfig,
    origin: Point,
    masks: Option<super::SupportMasks<'_>>,
) -> Result<(SupportPlan, Vec<PrintLayer>), SupportError> {
    prepare_with_masks_observed(
        tops,
        heights,
        outlines,
        config,
        origin,
        masks,
        &crate::progress::Silent,
    )
}

/// [`prepare_with_masks`] reporting the support stage and stopping early when cancelled.
pub fn prepare_with_masks_observed(
    tops: &[f64],
    heights: &[f64],
    outlines: &[Vec<ExPolygon>],
    config: &SupportConfig,
    origin: Point,
    masks: Option<super::SupportMasks<'_>>,
    observer: &dyn crate::progress::Observer,
) -> Result<(SupportPlan, Vec<PrintLayer>), SupportError> {
    observer.progress(crate::progress::Stage::Supports, 0.);
    // Facet/volume masks do not enable supports. Avoid the expensive bridge-coverage
    // prepass when neither automatic/manual supports nor forced layers can use it.
    let bridge_masks =
        (config.dont_support_bridges && (config.enabled || config.enforce_layers > 0)).then(|| {
            crate::bridge::detector::LazyCoverage::new(
                outlines,
                config.nozzle_diameter,
                config.bridge_angle,
            )
        });
    let layers: Vec<_> = tops
        .iter()
        .zip(heights)
        .zip(outlines)
        .enumerate()
        .map(|(i, ((&print_z, &height), outlines))| SupportObjectLayer {
            print_z,
            height,
            outlines,
            overhangs: None,
            bridges: bridge_masks.as_ref().map(|m| super::Bridges::Lazy(m, i)),
        })
        .collect();
    crate::progress::check(observer).map_err(SupportError)?;
    observer.progress(crate::progress::Stage::Supports, 0.1);
    let supports = super::generate_observed(
        &SupportInput {
            masks,
            layers: &layers,
            config,
            origin,
        },
        observer,
    )?;
    let schedule = merge(tops, heights, &supports);
    observer.progress(crate::progress::Stage::Supports, 1.);
    Ok((supports, schedule))
}

/// Include support flanges/rafts in bed probing and start-code bounds.
pub fn first_layer_bbox(supports: &SupportPlan, object: &[ExPolygon]) -> Option<BoundingBox> {
    let regions = supports
        .layers
        .first()
        .filter(|l| l.print_z <= supports.parameters.config.first_layer_height + EPS);
    BoundingBox::from_points(
        regions
            .into_iter()
            .flat_map(|l| l.regions.iter())
            .flat_map(|r| r.polygons.iter())
            .chain(object.iter())
            .flat_map(|e| e.contour.iter()),
    )
}
