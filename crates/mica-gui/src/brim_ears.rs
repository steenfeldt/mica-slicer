// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmoBrimEars.cpp :: GLGizmoBrimEars::gizmo_event, GLGizmoBrimEars::first_layer_slicer, GLGizmoBrimEars::auto_generate, GLGizmoBrimEars::find_single, GLGizmoBrimEars::get_brim_default_radius, GLGizmoBrimEars::add_point_to_cache
// Source: orca@8500fcdcca src/slic3r/GUI/ConfigManipulation.cpp :: ConfigManipulation::toggle_print_fff_options
// Changes: Click to add an ear, click an ear to remove it (no selection rectangle); ears live on the scene object (undo snapshots carry them); first layer sliced by Mica's slicer at 0.1 mm.
// Ear detection inherited from SuperSlicer (Remi Durand), as credited in Orca's Brim.cpp.
// Changes: Engine choices read mica_slicing_rules (mica_core::rules), not gcode_flavor or the presence of Orca keys (docs/slicing-rules.md).

//! The brim ears tool: hand-placed ears for `brim_type = painted` (OrcaSlicer's brim ear
//! gizmo), and the brim settings' Orca vocabulary (ear types and their inline options).
//!
//! Ears are stored on `scene::Object::brim_ears` in mesh-local coordinates, placed just below
//! the bed (world Z −0.0001) as Orca places them, and reach the slicer through
//! `Object::placed_region` as bed-coordinate `mica_core::brim::BrimEar`s.

use crate::scene::Object;
use glam::{Vec2, Vec3};
use mica_core::config::Config;
use mica_core::geometry::{ExPolygon, Point, SCALE};

/// Ear diameter limits (Orca's "Head diameter" slider).
pub const DIAMETER_MIN: f32 = 5.;
pub const DIAMETER_MAX: f32 = 20.;
/// World Z of a placed ear (Orca: just below the bed, so it counts as on the first layer).
pub const EAR_Z: f32 = -0.0001;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Diameter of new ears (mm); `None` until the printer's first-layer width sets it.
    pub diameter: Option<f32>,
    /// Auto-placement: corners sharper than this get an ear (degrees, Orca's 125).
    pub max_angle: f32,
    /// Auto-placement: decimation before detecting corners (mm, Orca's 1).
    pub detection: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            diameter: None,
            max_angle: 125.,
            detection: 1.,
        }
    }
}

impl Settings {
    /// The diameter for new ears: chosen, or Orca's default of 16 first-layer line widths.
    pub fn diameter_for(&self, config: Option<&Config>) -> f32 {
        self.diameter
            .unwrap_or_else(|| default_diameter(config))
            .clamp(DIAMETER_MIN, DIAMETER_MAX)
    }
    pub fn set_diameter(&mut self, d: f32) {
        self.diameter = Some(d.clamp(DIAMETER_MIN, DIAMETER_MAX));
    }
}

/// Orca's `get_brim_default_radius`: the first-layer line width × 16 (used as the diameter).
pub fn default_diameter(config: Option<&Config>) -> f32 {
    let Some(c) = config else { return 8. };
    let nozzle = c.float("nozzle_diameter").unwrap_or(0.4);
    let width = c
        .abs_value("first_layer_extrusion_width", nozzle)
        .filter(|w| *w > 0.)
        .unwrap_or(nozzle * 1.125);
    (width * 16.) as f32
}

/// Whether ears take effect for the object: its brim type is "painted".
pub fn painted(config: Option<&Config>, o: &Object) -> bool {
    o.overrides
        .get("brim_type")
        .map(String::as_str)
        .or_else(|| config.and_then(brim_type))
        == Some("painted")
}

fn brim_type(c: &Config) -> Option<&str> {
    c.extra("brim_type").or_else(|| c.string("brim_type"))
}

/// An OrcaSlicer-style configuration (the U1): the ear brim types exist there.
pub fn orca(c: &Config) -> bool {
    mica_core::rules::Rules::of(c).is_orca()
}

/// Ears in world coordinates: (centre on the bed, radius).
pub fn world(o: &Object) -> Vec<(Vec3, f32)> {
    let t = o.transform();
    o.brim_ears
        .iter()
        .map(|e| (t.transform_point3(Vec3::new(e[0], e[1], e[2])), e[3]))
        .collect()
}

/// Adds an ear centred at `at` (world XY) of `diameter`; false if one is there already.
pub fn add(o: &mut Object, at: Vec2, diameter: f32) -> bool {
    let local = o.transform().inverse().transform_point3(at.extend(EAR_Z));
    let ear = [local.x, local.y, local.z, diameter / 2.];
    if o.brim_ears.contains(&ear) {
        return false;
    }
    o.brim_ears.push(ear);
    true
}

/// The ear under `at` (world XY), the nearest of those whose disc contains it.
pub fn ear_at(o: &Object, at: Vec2) -> Option<usize> {
    world(o)
        .iter()
        .enumerate()
        .filter(|(_, (c, r))| c.truncate().distance(at) <= *r)
        .min_by(|a, b| {
            a.1.0
                .truncate()
                .distance(at)
                .total_cmp(&b.1.0.truncate().distance(at))
        })
        .map(|(i, _)| i)
}

/// The object's first layer (world coordinates): its model parts sliced at 0.1 mm, as the
/// gizmo's `first_layer_slicer` does.
pub fn first_layer(o: &Object) -> Vec<ExPolygon> {
    let mesh = o.placed_mesh();
    let bottom = f64::from(o.bounds().0.z);
    mica_core::slice::slice_mesh(&mesh, &[bottom + 0.1], 0.1)
        .into_iter()
        .next()
        .map(|l| l.islands)
        .unwrap_or_default()
}

/// "Auto-place": an ear on every sharp outer corner and every sharp hole corner of the first
/// layer, with the card's angle and detection length. Returns how many were added.
pub fn auto_place(o: &mut Object, s: &Settings, diameter: f32) -> usize {
    let layer = first_layer(o);
    let mut added = 0;
    for island in &layer {
        let mut points = mica_core::brim::ear_points(
            &island.contour,
            f64::from(s.detection),
            f64::from(s.max_angle),
            true,
        );
        for hole in &island.holes {
            points.extend(mica_core::brim::ear_points(
                hole,
                f64::from(s.detection),
                f64::from(s.max_angle),
                false,
            ));
        }
        for p in points {
            let at = Vec2::new((p.x as f64 / SCALE) as f32, (p.y as f64 / SCALE) as f32);
            added += usize::from(add(o, at, diameter));
        }
    }
    added
}

/// Ears that do not touch the first layer, directly or through other ears (Orca's
/// `find_single`, shown as invalid): they would print as loose discs.
pub fn loose(o: &Object, layer: &[ExPolygon]) -> Vec<usize> {
    use mica_core::geometry::clip::{intersection_ex, union_ex};
    let disc = |c: Vec3, r: f32| ExPolygon {
        contour: (0..24)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / 24.;
                Point::from_mm(
                    f64::from(c.x) + f64::from(r) * a.cos(),
                    f64::from(c.y) + f64::from(r) * a.sin(),
                )
            })
            .collect(),
        holes: vec![],
    };
    let ears = world(o);
    let mut attached = layer.to_vec();
    let mut open: Vec<usize> = (0..ears.len()).collect();
    loop {
        let found = open.iter().position(|&i| {
            let (c, r) = ears[i];
            !intersection_ex(&attached, &[disc(c, r)]).is_empty()
        });
        let Some(k) = found else { break };
        let i = open.remove(k);
        attached.push(disc(ears[i].0, ears[i].1));
        attached = union_ex(&attached);
    }
    open
}

/// The ear brim types an Orca configuration offers on top of PrusaSlicer's four (Orca's
/// `brim_type` vocabulary, in plain words).
pub const ORCA_BRIM_TYPES: [(&str, &str); 3] = [
    ("auto_brim", "Auto"),
    ("brim_ears", "Mouse ears (automatic)"),
    ("painted", "Ears where I place them"),
];

/// Orca's two automatic-ear options, shown under the brim type while it is "brim_ears":
/// (key, label, unit, min, max, default, tooltip).
pub const EAR_OPTIONS: [(&str, &str, &str, f64, f64, f64, &str); 2] = [
    (
        "brim_ears_max_angle",
        "Brim ear max angle",
        "°",
        0.,
        180.,
        125.,
        "Maximum angle to let a brim ear appear.\nIf set to 0, no brim will be created.\n\
         If set to ~180, brim will be created on everything but straight sections.",
    ),
    (
        "brim_ears_detection_length",
        "Brim ear detection radius",
        "mm",
        0.,
        1000.,
        1.,
        "The geometry will be decimated before detecting sharp angles. This parameter \
         indicates the minimum length of the deviation for the decimation.\n0 to deactivate.",
    ),
];

/// One of `EAR_OPTIONS` by key.
pub fn ear_option(
    key: &str,
) -> Option<&'static (
    &'static str,
    &'static str,
    &'static str,
    f64,
    f64,
    f64,
    &'static str,
)> {
    EAR_OPTIONS.iter().find(|o| o.0 == key)
}

/// Whether an inline brim option shows under the brim type (Orca hides the ear options
/// unless the type is "brim_ears").
pub fn option_shown(key: &str, c: &Config) -> bool {
    ear_option(key).is_none() || (orca(c) && brim_type(c) == Some("brim_ears"))
}

/// The current value of an ear option (an Orca extra, or its default).
pub fn option_value(key: &str, c: &Config) -> f64 {
    c.extra(key)
        .and_then(|v| v.parse().ok())
        .or_else(|| c.float(key))
        .or_else(|| ear_option(key).map(|o| o.5))
        .unwrap_or(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_auto_and_loose_ears() {
        let mut o = crate::layer_edit::tests::cube();
        let (min, max) = o.bounds();
        assert!(add(&mut o, min.truncate(), 8.));
        assert!(!add(&mut o, min.truncate(), 8.), "no duplicate");
        assert_eq!(ear_at(&o, min.truncate() + Vec2::splat(1.)), Some(0));
        assert_eq!(ear_at(&o, min.truncate() - Vec2::splat(10.)), None);
        // Placed just below the bed, in world coordinates.
        let w = world(&o);
        assert!((w[0].0.z - EAR_Z).abs() < 1e-4 && w[0].1 == 4.);
        // A far ear does not touch the first layer.
        add(&mut o, max.truncate() + Vec2::splat(30.), 8.);
        let layer = first_layer(&o);
        assert_eq!(layer.len(), 1);
        assert_eq!(loose(&o, &layer), vec![1]);
        o.brim_ears.clear();
        // A cube's four corners are sharp: four ears.
        assert_eq!(auto_place(&mut o, &Settings::default(), 8.), 4);
        assert!(loose(&o, &layer).is_empty());
        // The ears move with the object.
        let before = world(&o)[0].0;
        o.position.x += 10.;
        assert!((world(&o)[0].0.x - before.x - 10.).abs() < 1e-4);
        let region = o.placed_region().expect("ears need the regions path");
        assert_eq!(region.brim_ears.len(), 4);
        assert!((region.brim_ears[0].x - f64::from(world(&o)[0].0.x)).abs() < 1e-4);
    }

    #[test]
    fn settings_vocabulary() {
        let mut c = Config::defaults();
        assert!(!orca(&c));
        assert!(option_shown("brim_width", &c));
        assert!(!option_shown("brim_ears_max_angle", &c));
        // An Orca key alone is not Orca's rules any more; the switch is.
        c.set_raw("initial_layer_print_height", "0.2").unwrap();
        assert!(!orca(&c));
        c.set_raw("mica_slicing_rules", "orcaslicer").unwrap();
        c.set_raw("brim_type", "brim_ears").unwrap();
        assert!(option_shown("brim_ears_max_angle", &c));
        assert_eq!(option_value("brim_ears_max_angle", &c), 125.);
        c.set_raw("brim_ears_max_angle", "90").unwrap();
        assert_eq!(option_value("brim_ears_max_angle", &c), 90.);
        let s = Settings::default();
        assert!((s.diameter_for(Some(&c)) - default_diameter(Some(&c))).abs() < 1e-6);
    }
}
