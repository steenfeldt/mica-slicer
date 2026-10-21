// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoPainterBase.cpp :: get_projected_mouse_positions, gizmo_event
// Changes: Stroke continuity only (the rest is original Mica code): pointer samples between the previous and the current position at about half a brush radius (upstream: every pixel), capped per frame; the hits are simplified with Douglas-Peucker in 3D at a tenth of the radius (upstream projects them on a plane through three hits and simplifies the 2D polyline); one capsule per pair of consecutive hits starting its walk at the first hit's facet. A miss ends the chain and the next hit starts a new one (upstream stops at the first miss).

//! The painting tools' state: brush settings shared by the four tools, and the session that
//! holds the engine's [`TriangleSelector`] for the object being painted. Strokes go through
//! [`Session::stroke`]; the result is written back to the object's facet painting after every
//! stroke frame, so the overlay, undo and the slicer all read one source.

use crate::scene::Object;
use glam::{Mat4, Vec3};
use mica_core::painting::{
    FacetPainting, FacetStates, PaintKind,
    selector::{Brush, Cursor, HeightRange, Spread, TriangleSelector},
};
use std::sync::Arc;

/// The brush shapes of the tool card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushKind {
    /// A disc on the screen: paints what the pointer covers, front faces only.
    Circle,
    /// A ball around the hit point: paints through to the back.
    Sphere,
    /// Smart fill: the connected surface within the angle threshold.
    Fill,
    /// A horizontal band of the model around the hit.
    Height,
}
impl BrushKind {
    pub const ALL: [Self; 4] = [Self::Circle, Self::Sphere, Self::Fill, Self::Height];
    pub fn label(self) -> &'static str {
        match self {
            Self::Circle => "Circle",
            Self::Sphere => "Sphere",
            Self::Fill => "Fill",
            Self::Height => "Height",
        }
    }
}

pub const RADIUS_MIN: f32 = 0.4;
pub const RADIUS_MAX: f32 = 25.;
/// Largest fill spread the card takes, mm (0 is no limit).
pub const SPREAD_MAX: f32 = 500.;
/// The spread slider's range, mm (the field takes more).
pub const SPREAD_SLIDER_MAX: f32 = 100.;

/// Brush settings, shared by the tools (the mode differs per kind).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub brush: BrushKind,
    /// Brush radius, mm.
    pub radius: f32,
    /// Smart-fill angle threshold, degrees between neighbouring facets.
    pub fill_angle: f32,
    /// How far a fill may spread from the click along the surface, mm; 0 = no limit.
    pub fill_spread: f32,
    /// Height-range band, mm.
    pub height: f32,
    /// Enforce (true) or block, for supports and seams.
    pub enforce: bool,
    /// Zero-based tool for colour painting.
    pub tool: usize,
    pub erase: bool,
    /// Support painting: highlight facets steeper than `overhang_angle`.
    pub show_overhangs: bool,
    /// Overhang threshold, degrees from the horizontal (like `support_material_threshold`).
    pub overhang_angle: f32,
    /// Support painting: the brush only takes on highlighted overhang facets.
    pub only_overhangs: bool,
    /// Texture painting: the slot (1..) the brush paints.
    pub texture_slot: u8,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            brush: BrushKind::Circle,
            radius: 3.,
            fill_angle: 30.,
            fill_spread: 0.,
            height: 2.,
            enforce: true,
            tool: 1,
            erase: false,
            show_overhangs: false,
            overhang_angle: 45.,
            only_overhangs: false,
            texture_slot: 1,
        }
    }
}
impl Settings {
    /// The facet state a stroke writes for `kind` (0 erases).
    pub fn state_for(&self, kind: PaintKind, erase: bool) -> u8 {
        if erase || self.erase {
            return 0;
        }
        match kind {
            PaintKind::Material => (self.tool + 1).min(255) as u8,
            PaintKind::Fuzzy => 1,
            PaintKind::Texture => self.texture_slot.clamp(1, crate::textures::SLOTS),
            PaintKind::Supports | PaintKind::Seam => {
                if self.enforce {
                    1
                } else {
                    2
                }
            }
        }
    }
    /// `cos` of the overhang threshold, for `-n.z >= cos` tests (an angle measured from the
    /// horizontal; 90° would make vertical walls count).
    pub fn overhang_cos(&self) -> f32 {
        self.overhang_angle.clamp(0., 89.9).to_radians().cos()
    }
    /// Overhang filter for a support stroke, when asked for.
    pub fn stroke_overhang(&self, kind: PaintKind) -> Option<f32> {
        (kind == PaintKind::Supports && self.show_overhangs && self.only_overhangs)
            .then(|| self.overhang_cos())
    }
    pub fn set_radius(&mut self, r: f32) {
        self.radius = r.clamp(RADIUS_MIN, RADIUS_MAX);
    }
    pub fn set_fill_spread(&mut self, mm: f32) {
        self.fill_spread = if mm.is_finite() {
            mm.clamp(0., SPREAD_MAX)
        } else {
            0.
        };
    }
    /// The fill's spread limit from a click at `origin` (`None`: no limit).
    pub fn spread_at(&self, origin: Vec3) -> Option<Spread> {
        (self.fill_spread > 0.).then(|| Spread {
            origin: origin.to_array(),
            max_distance: self.fill_spread,
        })
    }
}

/// One stroke event from the viewport, in bed coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The original facet under the pointer.
    pub face: usize,
    pub point: Vec3,
    /// The camera position (the brush looks along `point - camera`).
    pub camera: Vec3,
}

/// The selector over the object being painted, kept while the tool is active.
pub struct Session {
    pub object_id: u64,
    pub kind: PaintKind,
    pub selector: TriangleSelector,
    transform: Mat4,
    /// Fingerprint of the object painting the selector was loaded from or last wrote.
    fingerprint: u64,
}

impl Session {
    pub fn new(o: &Object, kind: PaintKind) -> Self {
        let states = o.painting.get(kind);
        let mut selector = TriangleSelector::new(
            &o.mesh,
            &o.transform().to_cols_array_2d(),
            states.reversed(),
        );
        selector.load(states);
        Self {
            object_id: o.id,
            kind,
            selector,
            transform: o.transform(),
            fingerprint: o.painting.fingerprint(),
        }
    }

    /// Whether the selector still mirrors this object (same object, kind, placement and
    /// painting; undo or a move makes it stale).
    pub fn matches(&self, o: &Object, kind: PaintKind) -> bool {
        self.object_id == o.id
            && self.kind == kind
            && self.fingerprint == o.painting.fingerprint()
            && self
                .transform
                .to_cols_array()
                .iter()
                .zip(o.transform().to_cols_array())
                .all(|(a, b)| (a - b).abs() < 1e-5)
    }

    /// Applies one stroke frame (a dab); returns whether the painting changed.
    #[cfg(test)]
    pub fn stroke(&mut self, settings: &Settings, hit: Hit, erase: bool) -> bool {
        self.stroke_path(settings, &[hit], erase)
    }

    /// Applies one stroke frame along `path` (oldest first): the circle and sphere brushes
    /// sweep a capsule over each pair of consecutive points, so the stroke is continuous at
    /// any drag speed (upstream's `DoublePointCursor` per pair of projected mouse positions);
    /// fill and height range apply at every point. Returns whether the painting changed.
    pub fn stroke_path(&mut self, settings: &Settings, path: &[Hit], erase: bool) -> bool {
        let Some(last) = path.last() else {
            return false;
        };
        let state = settings.state_for(self.kind, erase);
        let before = self.selector.serialize();
        let overhang = settings.stroke_overhang(self.kind);
        match settings.brush {
            BrushKind::Circle | BrushKind::Sphere => {
                let brush = if settings.brush == BrushKind::Circle {
                    Brush::Circle
                } else {
                    Brush::Sphere
                };
                if path.len() == 1 {
                    let cursor = Cursor::new(
                        brush,
                        last.point.to_array(),
                        last.camera.to_array(),
                        settings.radius,
                    );
                    self.selector
                        .paint(last.face, &cursor, state, true, overhang);
                }
                for w in path.windows(2) {
                    let cursor = Cursor::capsule(
                        brush,
                        w[0].point.to_array(),
                        w[1].point.to_array(),
                        w[0].camera.to_array(),
                        settings.radius,
                    );
                    self.selector
                        .paint(w[0].face, &cursor, state, true, overhang);
                }
            }
            BrushKind::Fill => {
                let mut done = vec![];
                for h in path {
                    if !done.contains(&h.face) {
                        done.push(h.face);
                        self.selector.fill_within(
                            h.face,
                            settings.fill_angle,
                            state,
                            overhang,
                            settings.spread_at(h.point),
                        );
                    }
                }
            }
            BrushKind::Height => {
                for h in path {
                    self.selector.paint_height_range(
                        &HeightRange::around(h.point.z, settings.height),
                        state,
                    );
                }
            }
        }
        self.selector.serialize() != before
    }

    /// What a Fill click at `hit` would paint, as mesh-local triangles (the hover preview):
    /// the same fill, spread limit and overhang filter on an unpainted copy of the selector.
    pub fn fill_preview(&self, settings: &Settings, hit: Hit) -> Vec<[[f32; 3]; 3]> {
        let mut probe = self.selector.blank();
        probe.fill_within(
            hit.face,
            settings.fill_angle,
            1,
            settings.stroke_overhang(self.kind),
            settings.spread_at(hit.point),
        );
        let mut out = vec![];
        probe.leaves(false, |_, t| out.push(t));
        out
    }

    /// Paints every overhang facet (support enforcers) and returns how many.
    pub fn paint_overhangs(&mut self, settings: &Settings) -> usize {
        self.selector
            .paint_overhangs(settings.overhang_cos(), 0., 1)
    }

    pub fn clear(&mut self) {
        self.selector.clear();
    }

    /// Facets steeper than the overhang threshold, for the highlight overlay.
    pub fn overhang_facets(&self, settings: &Settings) -> Vec<usize> {
        self.selector.overhang_facets(settings.overhang_cos(), 0.)
    }

    /// Writes the selector's state into the object's painting.
    pub fn store(&mut self, o: &mut Object) {
        let states: FacetStates = self.selector.serialize();
        let mut painting: FacetPainting = (*o.painting).clone();
        *painting.get_mut(self.kind) = states;
        o.painting = Arc::new(painting);
        o.painting_edited = true;
        self.fingerprint = o.painting.fingerprint();
    }
}

/// The Fill brush's hover preview (the editor keeps it while its inputs stay the same).
pub struct FillPreview {
    /// The inputs it was built from (object, placement, brush settings, colours, and with a
    /// spread limit the hovered point).
    pub key: u64,
    /// Without a spread limit: the facets of the filled region, sorted (any of them shows the
    /// same preview). Empty with a limit.
    pub region: Vec<usize>,
    pub overlay: crate::scene::paint::PaintOverlay,
}

impl FillPreview {
    /// Strength of the preview hatch over the object.
    pub const ALPHA: f32 = 0.65;

    pub fn new(
        session: &Session,
        settings: &Settings,
        hit: Hit,
        key: u64,
        tool_colors: &[[f32; 3]],
    ) -> Self {
        use crate::scene::paint::{self as overlay, Pattern};
        let state = settings.state_for(session.kind, false);
        let mut color = if settings.erase {
            [0.95, 0.4, 0.35, 1.]
        } else {
            overlay::state_color(session.kind, state, tool_colors).unwrap_or([0.95, 0.4, 0.35, 1.])
        };
        color[3] = Self::ALPHA;
        let vertices = session
            .fill_preview(settings, hit)
            .into_iter()
            .flatten()
            .map(|position| overlay::PaintVertex {
                position,
                color,
                pattern: Pattern::Hatch as u32,
            })
            .collect();
        let region = if settings.fill_spread > 0. {
            vec![]
        } else {
            let mut r = session.selector.fill_region(
                hit.face,
                settings.fill_angle,
                settings.stroke_overhang(session.kind),
            );
            r.sort_unstable();
            r
        };
        // The region's first facet tells regions apart (they are disjoint), so the renderer
        // sees a new key for a new region.
        let overlay_key = key ^ region.first().map_or(hit.face, |f| *f) as u64;
        Self {
            key,
            region,
            overlay: overlay::PaintOverlay {
                object: session.object_id,
                key: overlay_key.rotate_left(1),
                vertices: Arc::new(vertices),
            },
        }
    }
}

/// Most pointer samples raycast per stroke frame (each one is a pick over the mesh).
pub const MAX_STROKE_SAMPLES: usize = 16;

/// Screen positions after `from` up to and including `to`, about `step` apart (upstream's
/// `get_projected_mouse_positions` interpolation between the last and current mouse
/// position), at most [`MAX_STROKE_SAMPLES`]: a faster drag gets sparser samples, which the
/// capsules between them still join.
pub fn stroke_samples(from: glam::Vec2, to: glam::Vec2, step: f32) -> Vec<glam::Vec2> {
    let dist = from.distance(to);
    if !dist.is_finite() {
        return vec![to];
    }
    let step = step.max(1.).max(dist / MAX_STROKE_SAMPLES as f32);
    let n = ((dist / step).ceil() as usize).clamp(1, MAX_STROKE_SAMPLES);
    (1..=n)
        .map(|k| from.lerp(to, k as f32 / n as f32))
        .collect()
}

/// Drops stroke points that lie within `tolerance` (mm) of the line through their
/// neighbours (Douglas–Peucker; upstream simplifies the projected hits with a tenth of the
/// brush radius), keeping the first and last point.
pub fn simplify_stroke(path: &[Hit], tolerance: f32) -> Vec<Hit> {
    if path.len() <= 2 {
        return path.to_vec();
    }
    let mut keep = vec![false; path.len()];
    keep[0] = true;
    keep[path.len() - 1] = true;
    let mut stack = vec![(0, path.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (path[a].point, path[b].point);
        let ab = pb - pa;
        let len2 = ab.length_squared();
        let mut worst = (0., a);
        for (k, h) in path.iter().enumerate().take(b).skip(a + 1) {
            let t = if len2 > 0. {
                ((h.point - pa).dot(ab) / len2).clamp(0., 1.)
            } else {
                0.
            };
            let d = h.point.distance(pa + ab * t);
            if d > worst.0 {
                worst = (d, k);
            }
        }
        if worst.0 > tolerance {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    path.iter()
        .zip(keep)
        .filter_map(|(h, k)| k.then_some(*h))
        .collect()
}

/// A tool's name for titles and undo labels.
pub fn tool_name(kind: PaintKind) -> &'static str {
    match kind {
        PaintKind::Supports => "Support painting",
        PaintKind::Seam => "Seam painting",
        PaintKind::Material => "Color painting",
        PaintKind::Fuzzy => "Fuzzy skin painting",
        PaintKind::Texture => "Texture painting",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        config::Config, input::InputObject, mesh::TriangleMesh, print_job::ToolAssignments,
    };

    fn cube() -> Object {
        let s = 20.;
        let mesh = TriangleMesh {
            vertices: vec![
                [0., 0., 0.],
                [s, 0., 0.],
                [s, s, 0.],
                [0., s, 0.],
                [0., 0., s],
                [s, 0., s],
                [s, s, s],
                [0., s, s],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        Object::new(InputObject::plain(
            "cube",
            0,
            0,
            mesh,
            ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ))
    }

    #[test]
    fn states_follow_mode_tool_and_erase() {
        let mut s = Settings::default();
        assert_eq!(s.state_for(PaintKind::Supports, false), 1);
        s.enforce = false;
        assert_eq!(s.state_for(PaintKind::Supports, false), 2);
        assert_eq!(s.state_for(PaintKind::Seam, false), 2);
        assert_eq!(s.state_for(PaintKind::Fuzzy, false), 1);
        s.tool = 3;
        assert_eq!(s.state_for(PaintKind::Material, false), 4);
        assert_eq!(s.state_for(PaintKind::Material, true), 0);
        s.erase = true;
        assert_eq!(s.state_for(PaintKind::Seam, false), 0);
        s.set_radius(100.);
        assert_eq!(s.radius, RADIUS_MAX);
        s.set_radius(0.);
        assert_eq!(s.radius, RADIUS_MIN);
        assert!((Settings::default().overhang_cos() - 45f32.to_radians().cos()).abs() < 1e-6);
    }

    fn painted_area(o: &Object) -> f32 {
        let mut a = 0.;
        o.painting.supports.visit(&o.mesh, false, |_, t| {
            let [p, q, r] = t.map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32));
            a += (q - p).cross(r - p).length() * 0.5;
        });
        a
    }

    /// Two pointer samples 19 mm apart with a 1 mm brush: dabs leave two spots, a stroke
    /// frame through both paints the band between them (a fast drag without gaps).
    #[test]
    fn a_fast_drag_paints_a_continuous_band() {
        let mut o = cube();
        o.position = Vec3::new(50., 30., 0.);
        let mut settings = Settings::default();
        settings.set_radius(1.);
        let at = |x: f32, z: f32, face| Hit {
            face,
            point: Vec3::new(x, 30., z),
            camera: Vec3::new(x, -200., z),
        };
        let (a, b) = (at(52., 15., 5), at(68., 5., 4));
        let mut dabs = Session::new(&o, PaintKind::Supports);
        dabs.stroke_path(&settings, &[a], false);
        dabs.stroke_path(&settings, &[b], false);
        let mut spots = o.clone();
        dabs.store(&mut spots);
        let mut stroke = Session::new(&o, PaintKind::Supports);
        assert!(stroke.stroke_path(&settings, &[a, b], false));
        let mut band = o.clone();
        stroke.store(&mut band);
        let (spots, band) = (painted_area(&spots), painted_area(&band));
        let length = a.point.distance(b.point);
        assert!(spots < 10., "{spots}");
        assert!(band > 1.6 * length && band < 3. * length + 10., "{band}");
        // The same frame again changes nothing.
        assert!(!stroke.stroke_path(&settings, &[a, b], false));
    }

    /// The Fill brush's hover preview shows what a click paints: the face within the angle,
    /// or with a max spread only the disc around the click, the same as the click itself.
    #[test]
    fn fill_preview_honours_the_spread_limit() {
        let mut o = cube();
        o.position = Vec3::new(50., 30., 0.);
        let area = |tris: &[[[f32; 3]; 3]]| -> f32 {
            tris.iter()
                .map(|t| {
                    let [p, q, r] = t.map(Vec3::from);
                    (q - p).cross(r - p).length() * 0.5
                })
                .sum()
        };
        let mut settings = Settings {
            brush: BrushKind::Fill,
            ..Settings::default()
        };
        let hit = Hit {
            face: 4,
            point: Vec3::new(60., 30., 10.),
            camera: Vec3::new(60., -200., 10.),
        };
        let session = Session::new(&o, PaintKind::Supports);
        // No limit: the whole front face (20 × 20), as before.
        assert!((area(&session.fill_preview(&settings, hit)) - 400.).abs() < 1e-2);
        // 5 mm: a disc of 5 mm around the click, in mesh coordinates around (10, 0, 10).
        settings.set_fill_spread(5.);
        let preview = session.fill_preview(&settings, hit);
        let disc = std::f32::consts::PI * 25.;
        let shown = area(&preview);
        assert!((shown - disc).abs() < disc * 0.1, "{shown} vs {disc}");
        for t in &preview {
            for p in t {
                let d = Vec3::from(*p).distance(Vec3::new(10., 0., 10.));
                assert!(d < 5.5, "preview reaches {d} mm");
            }
        }
        // The click paints exactly what the preview showed.
        let mut painting = Session::new(&o, PaintKind::Supports);
        assert!(painting.stroke(&settings, hit, false));
        painting.store(&mut o);
        assert!((painted_area(&o) - shown).abs() < 1e-3);
        // The preview does not paint.
        assert!(!session.selector.has_painting());
        settings.set_fill_spread(-3.);
        assert_eq!(settings.fill_spread, 0.);
        settings.set_fill_spread(f32::NAN);
        assert_eq!(settings.fill_spread, 0.);
    }

    #[test]
    fn stroke_samples_and_simplification() {
        use glam::Vec2;
        let s = stroke_samples(Vec2::ZERO, Vec2::new(10., 0.), 2.);
        assert_eq!(s.len(), 5);
        assert_eq!(*s.last().unwrap(), Vec2::new(10., 0.));
        // A long, fast move is capped and the samples spread out.
        let s = stroke_samples(Vec2::ZERO, Vec2::new(1000., 0.), 2.);
        assert_eq!(s.len(), MAX_STROKE_SAMPLES);
        assert_eq!(stroke_samples(Vec2::ONE, Vec2::ONE, 2.).len(), 1);
        // Collinear points collapse to the ends; a corner stays.
        let h = |x: f32, y: f32| Hit {
            face: 0,
            point: Vec3::new(x, y, 0.),
            camera: Vec3::new(x, y, 100.),
        };
        let line: Vec<Hit> = (0..10).map(|k| h(k as f32, 0.)).collect();
        assert_eq!(simplify_stroke(&line, 0.1).len(), 2);
        let corner = [h(0., 0.), h(5., 0.), h(10., 0.), h(10., 5.), h(10., 10.)];
        let kept = simplify_stroke(&corner, 0.1);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[1].point, Vec3::new(10., 0., 0.));
    }

    #[test]
    fn a_stroke_paints_stores_and_round_trips_through_the_object() {
        let mut o = cube();
        o.position = Vec3::new(50., 30., 0.);
        let mut session = Session::new(&o, PaintKind::Supports);
        assert!(session.matches(&o, PaintKind::Supports));
        let settings = Settings::default();
        // The front face (y = 30 in the bed) hit at its centre, seen from the front.
        let hit = Hit {
            face: 4,
            point: Vec3::new(60., 30., 10.),
            camera: Vec3::new(60., -200., 10.),
        };
        assert!(session.stroke(&settings, hit, false));
        session.store(&mut o);
        assert!(!o.painting.supports.is_empty());
        assert!(o.painting_edited);
        assert!(session.matches(&o, PaintKind::Supports));
        // A new session over the stored painting sees the same leaves.
        let again = Session::new(&o, PaintKind::Supports);
        assert_eq!(again.selector.serialize(), session.selector.serialize());
        // Moving the object makes the session stale; painting again does not.
        let moved = {
            let mut m = o.clone();
            m.position.x += 1.;
            m
        };
        assert!(!session.matches(&moved, PaintKind::Supports));
        assert!(!session.matches(&o, PaintKind::Seam));
        // Erasing the same spot removes it; the slicer then sees an edited, empty painting.
        assert!(session.stroke(&settings, hit, true));
        session.store(&mut o);
        assert!(o.painting.supports.is_empty());
        assert!(
            o.placed_region().is_some(),
            "edited painting still reaches the slicer"
        );
        // Colour fill paints the whole face with tool 2 (state 2).
        let mut colour = Session::new(&o, PaintKind::Material);
        let mut s = Settings {
            brush: BrushKind::Fill,
            ..Settings::default()
        };
        assert!(colour.stroke(&s, hit, false));
        colour.store(&mut o);
        let mut leaves = vec![];
        o.painting
            .material
            .visit(&o.mesh, false, |state, t| leaves.push((state, t)));
        assert_eq!(leaves.len(), 2);
        assert!(leaves.iter().all(|(s, _)| *s == 2));
        // Height range paints a band around z = 10 on every side.
        let mut band = Session::new(&o, PaintKind::Seam);
        s.brush = BrushKind::Height;
        assert!(band.stroke(&s, hit, false));
        band.store(&mut o);
        let mut zs = (f32::MAX, f32::MIN);
        o.painting.seam.visit(&o.mesh, false, |_, t| {
            for p in t {
                zs.0 = zs.0.min(p[2] as f32);
                zs.1 = zs.1.max(p[2] as f32);
            }
        });
        assert!(zs.0 >= 9. - 1e-3 && zs.1 <= 11. + 1e-3, "{zs:?}");
        // Overhangs of a lifted cube: the bottom; painting them gives two enforcer facets.
        o.position.z = 5.;
        let mut sup = Session::new(&o, PaintKind::Supports);
        assert_eq!(sup.overhang_facets(&s).len(), 2);
        assert_eq!(sup.paint_overhangs(&s), 2);
        sup.clear();
        assert!(!sup.selector.has_painting());
    }
}
