// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica Slicer engine.
//!
//! Takes meshes plus resolved settings and turns them into toolpaths, one layer at a time.
//! It has no GUI and no knowledge of G-code dialects; see `mica-gcode` for output.
//! What works and what comes next is in `ROADMAP.md` at the repository root.

pub mod arachne;
pub mod brick_layers;
pub mod bridge;
pub mod build_volume;
pub mod calib;
pub mod config;
pub mod custom_gcode;
pub mod cxx_sort;
pub mod emboss;
pub mod export_name;
pub(crate) mod extrusion_order;
pub mod fdm_mixer;
pub mod features;
pub mod filament_overrides;
pub mod fill;
pub mod flow;
pub mod full_spectrum;
pub mod geometry;
pub mod instances;
pub mod layer_profile;
pub mod merged;
pub mod mesh;
mod perimeter_frame;
mod perimeter_order;
pub mod perimeters;
pub mod print;
pub mod print_job;
pub mod regions;
pub mod resolve;
pub mod rules;
pub mod seam;
pub mod settings;
pub mod shared_file;
pub mod shells;
pub mod shortest_path;
pub mod slice;
pub mod small_features;
pub mod spiral_vase;
pub mod support;
pub mod threemf;
pub mod tool_ordering;
pub mod toolpath;
pub mod user_data;
pub mod wall_options;
pub mod wipe_tower;

/// Version of the engine, taken from the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod brim;
pub mod fuzzy_skin;
pub mod ironing;
pub mod texture;
pub mod texture_skin;

pub mod input;

pub mod arrange;
pub mod builtin_models;
pub mod cut_connectors;
pub mod flatten;
pub mod mesh_boolean;
pub mod mesh_cut;
pub mod mesh_edit;
pub mod mesh_repair;
pub mod mesh_simplify;
pub mod orient;
pub mod painting;

pub mod parallel;
pub mod progress;
pub mod timing;

pub mod adhesion_check;
pub mod job;
pub mod safety;
pub mod sequential;
