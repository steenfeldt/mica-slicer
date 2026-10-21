// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! G-code output for Mica Slicer.
//!
//! The engine produces machine-independent moves; this crate encodes them for a printer's
//! firmware, either as text or as Prusa binary G-code (`.bgcode`).

pub mod avoid_crossing;
pub mod bgcode;
mod boundary;
pub mod brick_check;
pub mod cooling;
mod custom_gcode;
pub mod export;
mod export_batch;
mod extrusion_speed;
pub mod full_config;
pub mod identity;
pub mod multi;
mod orca;
mod orca_fan;
mod orca_motion;
mod orca_overhang;
mod orca_toolchange;
pub mod overhang;
mod overhang_plan;
pub mod prusa_avoid_crossing;
pub mod prusa_metadata;
mod prusa_wipe;
mod seam;
mod sequential;
mod small_area;
pub mod smooth_path;
mod support_order;
pub mod templates;
pub mod thumbnails;
pub mod time_estimate;
pub mod travels;
pub mod validate;
mod wipe_tower;
pub mod writer;

/// Firmware dialect the output is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// Klipper, used by the Snapmaker U1.
    Klipper,
    /// Prusa Buddy firmware (Marlin-based), used by the Prusa XL and Core One.
    PrusaBuddy,
}

/// Container format the G-code is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Plain text `.gcode`; every printer accepts it.
    Text,
    /// Prusa binary G-code (`.bgcode`); see [`bgcode`] for the codec.
    Binary,
}

mod adhesion;

mod calib;
