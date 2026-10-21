// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Flow.cpp :: Flow::rounded_rectangle_extrusion_spacing, Flow::mm3_per_mm, Flow::with_spacing
// Changes: Rust port; upstream f32 spacing and truncating scaled dimensions, f64 public values.

//! Extrusion cross-section model.
//!
//! An extrusion is modelled as a rectangle with semicircular ends: `height` tall and `width` wide.
//! Neighbouring lines are placed `spacing` apart, which is less than `width` because the rounded
//! sides overlap.

use crate::geometry::{Coord, SCALE};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flow {
    /// Extrusion width in mm.
    pub width: f64,
    /// Layer height in mm.
    pub height: f64,
}

/// `1 - π/4`: how much narrower the spacing is than the width, per unit of height.
const ROUNDING: f64 = 1.0 - 0.25 * std::f64::consts::PI;

impl Flow {
    pub fn new(width: f64, height: f64) -> Self {
        assert!(
            width > 0.0 && height > 0.0,
            "flow needs positive width and height"
        );
        assert!(
            width - height * ROUNDING > 0.0,
            "flow width {width} too small for height {height}"
        );
        Self { width, height }
    }

    /// Centre-to-centre distance of neighbouring lines, in mm.
    pub fn spacing(&self) -> f64 {
        f64::from((-(self.height as f32)).mul_add(ROUNDING as f32, self.width as f32))
    }

    /// Cross-section area, i.e. mm³ of plastic per mm of travel.
    pub fn mm3_per_mm(&self) -> f64 {
        self.height * (self.width - self.height * ROUNDING)
    }

    /// The same flow widened (or narrowed) so neighbouring lines sit `spacing` mm apart; used when a solid
    /// fill adjusts its line spacing to fit the area.
    pub fn with_spacing(&self, spacing: f64) -> Self {
        Self::new(self.width + spacing - self.spacing(), self.height)
    }

    pub fn scaled_width(&self) -> Coord {
        (f64::from(self.width as f32) * SCALE) as Coord
    }

    pub fn scaled_spacing(&self) -> Coord {
        (self.spacing() * SCALE) as Coord
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_prusaslicer_numbers() {
        let f = Flow::new(0.45, 0.2);
        assert!((f.spacing() - 0.407_08).abs() < 1e-5);
        assert!((f.mm3_per_mm() - 0.081_416).abs() < 1e-6);
        let wider = f.with_spacing(0.42);
        assert!((wider.spacing() - 0.42).abs() < f32::EPSILON as f64);
        assert!((wider.width - 0.46292).abs() < 1e-5);
    }
}
