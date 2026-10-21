// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/PrintObject.cpp :: PrintObject::detect_surfaces_type, PrintObject::bridge_over_infill
// Changes: Two-phase snapshots over Mica's compact surfaces; normal bridge roles carry second layers, without adding a whole-object extrusion cache.

use super::BridgeSurface;
use crate::{
    config::Config,
    geometry::{
        ExPolygon, SCALE,
        clip::{Join, diff_ex, intersection_ex, offset_ex, opening_ex, union_ex},
    },
    resolve::ResolveError,
    settings::PrintSettings,
    shells::{FillSurfaces, ShellLayer},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtraBridgeLayer {
    #[default]
    Disabled,
    External,
    Internal,
    All,
}
impl ExtraBridgeLayer {
    pub fn from_config(c: &Config) -> Result<Self, ResolveError> {
        Ok(match c.string("enable_extra_bridge_layer") {
            None | Some("disabled") => Self::Disabled,
            Some("external_bridge_only") => Self::External,
            Some("internal_bridge_only") => Self::Internal,
            Some("apply_to_all") => Self::All,
            _ => return Err(ResolveError("invalid enable_extra_bridge_layer".into())),
        })
    }
}

/// External extra bridges are classified before shell propagation. Exposed tops stay untouched.
pub(crate) fn external(surfaces: &mut [FillSurfaces], layers: &[ShellLayer], s: &PrintSettings) {
    if !matches!(
        s.extra_bridge_layer,
        ExtraBridgeLayer::External | ExtraBridgeLayer::All
    ) {
        return;
    }
    let source: Vec<_> = surfaces.iter().map(|f| f.bottom_bridge.clone()).collect();
    for i in 1..surfaces.len() {
        let band = (layers[i - 1].external.width
            + s.perimeters.saturating_sub(1) as f64 * layers[i - 1].perimeter.width)
            * SCALE;
        let f = &mut surfaces[i];
        let overlap = opening_ex(&intersection_ex(&f.internal, &source[i - 1]), band);
        let overlap = diff_ex(&overlap, &offset_ex(&f.top, band, Join::Miter));
        f.internal = diff_ex(&f.internal, &overlap);
        f.bottom_bridge.extend(overlap);
    }
}

/// Read only original internal bridges so an extra layer can never create another extra layer.
pub(crate) fn internal(surfaces: &mut [FillSurfaces], layers: &[ShellLayer], s: &PrintSettings) {
    if !matches!(
        s.extra_bridge_layer,
        ExtraBridgeLayer::Internal | ExtraBridgeLayer::All
    ) {
        return;
    }
    let source: Vec<_> = surfaces.iter().map(|f| f.internal_bridge.clone()).collect();
    for i in 1..surfaces.len() {
        let Some(last) = source[i - 1].last() else {
            continue;
        };
        let band = layers[i - 1].solid.width * SCALE;
        let mask: Vec<ExPolygon> = source[i - 1].iter().map(|b| b.polygon.clone()).collect();
        let mask = opening_ex(&union_ex(&mask), band);
        let f = &mut surfaces[i];
        let mut new_bridges = Vec::new();
        for areas in [&mut f.internal, &mut f.internal_solid] {
            let mut leftovers = Vec::new();
            for polygon in std::mem::take(areas) {
                let one = std::slice::from_ref(&polygon);
                let overlap = opening_ex(&intersection_ex(one, &mask), band);
                if overlap.is_empty() {
                    leftovers.push(polygon);
                } else {
                    leftovers.extend(opening_ex(&diff_ex(one, &mask), band));
                    new_bridges.extend(overlap);
                }
            }
            *areas = leftovers;
        }
        f.internal_bridge
            .extend(new_bridges.into_iter().map(|polygon| BridgeSurface {
                polygon,
                angle: (last.angle + std::f64::consts::FRAC_PI_2) % std::f64::consts::PI,
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{flow::Flow, geometry::Point};
    fn rect(x: f64, y: f64) -> ExPolygon {
        ExPolygon {
            contour: [(0., 0.), (x, 0.), (x, y), (0., y)]
                .map(|(x, y)| Point::from_mm(x, y))
                .to_vec(),
            holes: vec![],
        }
    }
    #[test]
    fn modes_filter_tiny_areas_preserve_top_and_do_not_cascade() {
        let layers = vec![
            ShellLayer {
                print_z: 1.,
                height: 0.2,
                external: Flow::new(0.45, 0.2),
                perimeter: Flow::new(0.45, 0.2),
                solid: Flow::new(0.45, 0.2)
            };
            4
        ];
        for mode in [
            ExtraBridgeLayer::Disabled,
            ExtraBridgeLayer::External,
            ExtraBridgeLayer::Internal,
            ExtraBridgeLayer::All,
        ] {
            let mut s = PrintSettings::core_one_hf04_speed_pla();
            s.extra_bridge_layer = mode;
            let mut f = vec![FillSurfaces::default(); 4];
            f[0].bottom_bridge = vec![rect(12., 12.)];
            f[0].internal_bridge = vec![BridgeSurface {
                polygon: rect(12., 12.),
                angle: 0.3,
            }];
            for above in &mut f[1..] {
                above.internal = vec![rect(12., 12.)];
            }
            external(&mut f, &layers, &s);
            assert_eq!(
                !f[1].bottom_bridge.is_empty(),
                matches!(mode, ExtraBridgeLayer::External | ExtraBridgeLayer::All)
            );
            assert!(f[2].bottom_bridge.is_empty());
            internal(&mut f, &layers, &s);
            assert_eq!(
                !f[1].internal_bridge.is_empty(),
                mode == ExtraBridgeLayer::Internal
            );
            if let Some(b) = f[1].internal_bridge.first() {
                assert!((b.angle - 0.3 - std::f64::consts::FRAC_PI_2).abs() < 1e-8);
            }
            assert!(f[2].internal_bridge.is_empty());
        }
        let mut s = PrintSettings::core_one_hf04_speed_pla();
        s.extra_bridge_layer = ExtraBridgeLayer::All;
        let mut f = vec![FillSurfaces::default(); 2];
        f[0].bottom_bridge = vec![rect(0.5, 12.)];
        f[1].internal = vec![rect(12., 12.)];
        external(&mut f, &layers[..2], &s);
        assert!(f[1].bottom_bridge.is_empty());
        f[0].bottom_bridge = vec![rect(12., 12.)];
        f[1].top = vec![rect(12., 12.)];
        external(&mut f, &layers[..2], &s);
        assert!(f[1].bottom_bridge.is_empty());
    }
}
