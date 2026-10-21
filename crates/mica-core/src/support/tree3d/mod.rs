// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Support/TreeSupportCommon.hpp :: FFFTreeSupport
// Source: orca@8500fcdcca src/libslic3r/Support/TreeSupportCommon.hpp :: TreeSupport3D
// Changes: Default-off pipeline adapter, owned geometry and shared implementation selected by Rules.
// Copyright (c) Prusa Research 2022 - 2023 Vojtěch Bubník @bubnikv, Pavel Mikuš @Godrak
// PrusaSlicer is released under the terms of the AGPLv3 or higher.
// Tree supports by Thomas Rahm, losely based on Tree Supports by CuraEngine.
// Original source of Thomas Rahm's tree supports: https://github.com/ThomasRahm/CuraEngine
// Copyright (c) 2021 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

//! Organic tree supports (PrusaSlicer organic port, Orca tweaks selected by Rules).
//! Lengths are integer scaled coordinates unless a field explicitly says millimetres.

mod branch_mesh;
mod drawing;
mod element;
mod geometry;
mod nudge;
mod pathing;
mod pipeline;
mod settings;
mod smoothing;
mod tips;
mod volumes;
pub use drawing::{TreeDrawing, organic_draw_branches};
pub use element::*;
pub use geometry::{polygons_simplify, safe_offset_inc};
pub use pathing::*;
pub use pipeline::Tree3dConfig;
pub(super) use pipeline::generate;
pub use settings::*;
pub use tips::*;
pub use volumes::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod pathing_tests;

#[cfg(test)]
mod drawing_tests;
