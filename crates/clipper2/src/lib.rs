// SPDX-License-Identifier: BSL-1.0
// Provenance: ported
// Source: clipper2-rust@32519f102e0113119823b9649739f311613ca042 src/lib.rs :: clipper2_rust
// Changes: Vendored from clipper2-rust 1.2.0 (Lars Brubaker's Rust port of Angus Johnson's Clipper2, BSL-1.0);
// Changes: SVG/file/timer `utils` module removed; opt-in `capture` module (feature) added; Clipper 6.4 offsetter module `offset64`.
#![forbid(unsafe_code)]

//! # clipper2-rust
//!
//! A pure Rust port of the [Clipper2](https://github.com/AngusJohnson/Clipper2) polygon
//! clipping and offsetting library by Angus Johnson.
//!
//! ## Features
//!
//! - **Boolean operations**: Intersection, Union, Difference, and XOR on polygons
//! - **Polygon offsetting**: Inflate/deflate with Miter, Square, Bevel, and Round joins
//! - **Rectangle clipping**: High-performance rectangular clipping
//! - **Minkowski sum/difference**: Geometric Minkowski operations
//! - **Path simplification**: Ramer-Douglas-Peucker and Clipper2's simplification
//! - **PolyTree**: Hierarchical parent/child/hole polygon representation
//! - **Dual precision**: Integer (`i64`) and floating-point (`f64`) coordinate support
//!
//! ## Quick Start
//!
//! ```rust
//! use clipper2_rust::core::FillRule;
//!
//! let subject = vec![clipper2_rust::make_path64(&[100, 100, 300, 100, 300, 300, 100, 300])];
//! let clip = vec![clipper2_rust::make_path64(&[200, 200, 400, 200, 400, 400, 200, 400])];
//!
//! let result = clipper2_rust::intersect_64(&subject, &clip, FillRule::NonZero);
//! ```
//!
//! ## Coordinate Systems
//!
//! - [`Path64`] / [`Paths64`]: Integer coordinates (`i64`) — recommended for precision
//! - [`PathD`] / [`PathsD`]: Floating-point coordinates (`f64`) — convenient for external data

pub mod core;
pub mod engine;
pub mod engine_fns;
pub mod engine_public;
pub mod rectclip;
pub mod version;

#[cfg(feature = "capture")]
mod capture;
pub mod clipper;
pub mod minkowski;
pub mod offset;
pub mod offset64;

pub use clipper::*;
pub use core::*;
pub use engine::*;
pub use engine_fns::*;
pub use engine_public::*;
pub use minkowski::*;
pub use offset::*;
pub use offset64::*;
pub use rectclip::*;
pub use version::*;

// Include using_z feature tests from separate file
#[cfg(all(test, feature = "using_z"))]
#[path = "using_z_tests.rs"]
mod using_z_tests;
