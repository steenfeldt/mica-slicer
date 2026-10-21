// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Support/SupportLayer.hpp :: SupportGeneratorLayer, SupporLayerType
// Changes: Borrowed, single-region input; owned polygon plan, with toolpaths generated on demand.
// Changes: Base interface layers are a region kind of their own and do not advance the interface id.
// Copyright (c) Prusa Research 2023 Vojtěch Bubník, Pavel Mikuš

//! Normal, tree and organic supports, independent of the object pipeline. Z coordinates are absolute print heights.
//! Call [`generate`] with outlines and optional classified overhangs/bridges, then generate each
//! [`SupportLayer`]'s paths as it is needed. No object toolpaths or mesh are retained.

mod common;
mod material;
mod orca_overhangs;
mod parameters;
mod tree;
/// Organic tree supports: PrusaSlicer 2.9.6 organic port with Orca switches by Rules.
pub mod tree3d;
mod tree_common;
mod tree_paths;
mod tree_volumes;
pub use tree_common::{TreeConfig, TreeStatistics};
pub mod schedule;

use crate::geometry::{ExPolygon, Point};
use crate::toolpath::LayerToolpaths;
pub use parameters::{
    InterfacePattern, SupportConfig, SupportParameters, SupportPattern, SupportStyle,
};

pub(crate) const EPS: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
pub struct SupportObjectLayer<'a> {
    pub print_z: f64,
    pub height: f64,
    pub outlines: &'a [ExPolygon],
    /// Optional preclassified unsupported surfaces; `None` detects them from adjacent outlines.
    pub overhangs: Option<&'a [ExPolygon]>,
    /// Successfully bridged areas to exclude when `dont_support_bridges` is enabled.
    /// `None` is conservative: no bridge regions are assumed from outlines alone.
    pub bridges: Option<Bridges<'a>>,
}

/// A layer's bridge coverage: given, or computed when first asked for.
#[derive(Clone, Copy)]
pub enum Bridges<'a> {
    Ready(&'a [ExPolygon]),
    Lazy(&'a crate::bridge::detector::LazyCoverage<'a>, usize),
}
impl std::fmt::Debug for Bridges<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready(areas) => f.debug_tuple("Ready").field(areas).finish(),
            Self::Lazy(_, layer) => f.debug_tuple("Lazy").field(layer).finish(),
        }
    }
}
impl<'a> Bridges<'a> {
    pub fn get(self) -> &'a [ExPolygon] {
        match self {
            Self::Ready(areas) => areas,
            Self::Lazy(coverage, layer) => coverage.layer(layer),
        }
    }
}

pub type SupportMasks<'a> = (&'a [Vec<ExPolygon>], &'a [Vec<ExPolygon>]);

pub struct SupportInput<'a> {
    /// Enforcer and blocker slices on the object layer grid. Enforcers override automatic detection.
    pub masks: Option<SupportMasks<'a>>,
    pub layers: &'a [SupportObjectLayer<'a>],
    pub config: &'a SupportConfig,
    /// Object centre, for alignment of grid and fill patterns.
    pub origin: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportLayerKind {
    RaftBase,
    RaftInterface,
    BottomContact,
    BottomInterface,
    Base,
    /// A dense layer printed with the base tool between a soluble (or other-filament)
    /// interface and the base (upstream's base interface layers).
    BaseInterface,
    TopInterface,
    TopContact,
}

#[derive(Clone, Debug)]
pub struct SupportRegion {
    pub kind: SupportLayerKind,
    pub polygons: Vec<ExPolygon>,
}

#[derive(Clone, Debug)]
pub struct SupportLayer {
    pub print_z: f64,
    pub height: f64,
    pub interface_id: usize,
    pub regions: Vec<SupportRegion>,
}

#[derive(Clone, Debug)]
pub struct SupportPlan {
    pub tree_statistics: Option<TreeStatistics>,
    pub layers: Vec<SupportLayer>,
    pub parameters: SupportParameters,
    pub origin: Point,
}

impl SupportPlan {
    /// Only this layer's paths are allocated. The returned paths precede object paths at equal Z.
    pub fn toolpaths(&self, index: usize) -> LayerToolpaths {
        common::generate_support_toolpaths(
            &self.layers[index],
            index,
            &self.parameters,
            self.origin,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SupportError(pub String);
impl std::fmt::Display for SupportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SupportError {}

/// Detect contacts, project support columns, assign independent print heights and interfaces,
/// and generate a raft when requested. Input layers must be strictly ordered and non-overlapping.
pub fn generate(input: &SupportInput<'_>) -> Result<SupportPlan, SupportError> {
    generate_observed(input, &crate::progress::Silent)
}

/// [`generate`] reporting its phases as 0.1 .. 0.9 of the support stage, with cancel checks
/// between them (and per layer inside the tree pathing).
pub fn generate_observed(
    input: &SupportInput<'_>,
    observer: &dyn crate::progress::Observer,
) -> Result<SupportPlan, SupportError> {
    use crate::progress::Stage;
    let check = || crate::progress::check(observer).map_err(SupportError);
    if let Some((enforcers, blockers)) = input.masks
        && (enforcers.len() != input.layers.len() || blockers.len() != input.layers.len())
    {
        return Err(SupportError(
            "support masks must use the object layer grid".into(),
        ));
    }
    let parameters = SupportParameters::new(input.config)?;
    if let Some(first) = input.layers.first()
        && first.print_z - first.height < parameters.object_z_offset - EPS
    {
        return Err(SupportError(
            "raft input layers must be lifted by SupportParameters::object_z_offset".into(),
        ));
    }
    let mut previous = 0.0;
    for layer in input.layers {
        if !layer.print_z.is_finite()
            || !layer.height.is_finite()
            || layer.height <= 0.0
            || layer.print_z <= previous
            || layer.print_z - layer.height < previous - EPS
        {
            return Err(SupportError("support input layers must have positive heights and increasing, non-overlapping Z intervals".into()));
        }
        previous = layer.print_z;
    }
    let mut tree_statistics = None;
    let mut layers = if input.layers.is_empty() {
        Vec::new()
    } else if parameters.config.style.is_tree() {
        let (layers, statistics) = if parameters.config.style == SupportStyle::Organic {
            tree3d::generate(input, &parameters, observer)?
        } else {
            tree::generate(input, &parameters, observer)?
        };
        tree_statistics = Some(statistics);
        layers
    } else {
        let contacts = material::top_contact_layers(input, &parameters);
        check()?;
        observer.progress(Stage::Supports, 0.3);
        let (bottoms, areas) = material::project_columns(input, &contacts, &parameters, observer)?;
        check()?;
        observer.progress(Stage::Supports, 0.5);
        let mut layers = common::generate_support_layers(input, &contacts, &bottoms, &parameters);
        check()?;
        observer.progress(Stage::Supports, 0.6);
        material::generate_base_layers(input, &contacts, &areas, &mut layers, &parameters);
        check()?;
        observer.progress(Stage::Supports, 0.8);
        common::generate_interface_layers(&mut layers, &contacts, &bottoms, &parameters, None);
        common::generate_raft_base(input, &mut layers, &parameters);
        observer.progress(Stage::Supports, 0.9);
        layers
    };
    check()?;
    layers.retain(|layer| layer.regions.iter().any(|r| !r.polygons.is_empty()));
    let mut interface_id = 0;
    for layer in &mut layers {
        layer.interface_id = interface_id;
        if layer.regions.iter().any(|r| {
            !matches!(
                r.kind,
                SupportLayerKind::Base
                    | SupportLayerKind::BaseInterface
                    | SupportLayerKind::RaftBase
            )
        }) {
            interface_id += 1;
        }
    }
    Ok(SupportPlan {
        tree_statistics,
        layers,
        parameters,
        origin: input.origin,
    })
}

#[cfg(test)]
mod tests;
