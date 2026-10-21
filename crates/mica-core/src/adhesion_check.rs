// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::_set_warning_notification
// Source: orca@8500fcdcca src/slic3r/GUI/GLCanvas3D.cpp :: GLCanvas3D::_set_warning_notification
// Changes: Upstream warns about an object outside the print area before slicing and reports "a toolpath outside the print area" as a slicing error after; Mica checks the skirt and brim the same two ways (an estimate from each object's first-layer section before slicing, the generated adhesion paths after), names the object, the bed edge and the distance, and the move that fits it. Both measure extrusion centerlines, as upstream's toolpath test (`BuildVolume::all_paths_inside`) does.

//! Skirts and brims against the bed. Before slicing, [`preflight`] grows an object's
//! first-layer section by its brim (and the skirt around it) and measures how far that passes
//! the bed's edges; after preparing, [`plan_overrun`] measures the generated skirt and brim
//! paths exactly as the G-code export's bounds check does (`mica_gcode::export`), so a print
//! the exporter would refuse fails with a message that says which object, which edge and how
//! far, in the same words for the CLI and the GUI ([`explain`]).

use crate::{
    brim::{AdhesionPlan, BrimType, DraftShield},
    build_volume::{BED_EPSILON, BuildVolume},
    config::Config,
    flow::Flow,
    geometry::unscale,
    mesh::TriangleMesh,
};

/// Which adhesion passes the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Brim,
    Skirt,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Brim => "brim",
            Kind::Skirt => "skirt",
        }
    }
}

/// A bed edge, as seen from the front of the printer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Front,
    Back,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Front => "front",
            Side::Back => "back",
        }
    }
}

/// The bed rectangle (bounds of `bed_shape`), whose edges the messages name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bed {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl Bed {
    pub fn from_config(c: &Config) -> Option<Self> {
        let pts = c.points("bed_shape")?;
        if pts.len() < 3 {
            return None;
        }
        let fold = |f: fn(&(f64, f64)) -> f64, init: f64, op: fn(f64, f64) -> f64| {
            pts.iter().map(f).fold(init, op)
        };
        Some(Self {
            min: [
                fold(|p| p.0, f64::MAX, f64::min),
                fold(|p| p.1, f64::MAX, f64::min),
            ],
            max: [
                fold(|p| p.0, f64::MIN, f64::max),
                fold(|p| p.1, f64::MIN, f64::max),
            ],
        })
    }
}

/// Skirt or brim past the bed's edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Overrun {
    pub kind: Kind,
    /// The edges it passes and how far (mm), farthest first.
    pub sides: Vec<(Side, f64)>,
    /// The XY move (mm) that brings it inside, or `None` when it is wider or deeper than the bed.
    pub fix: Option<[f64; 2]>,
    /// The grown first-layer outline (bed mm) of a [`preflight`], for drawing; empty after
    /// slicing.
    pub outline: Vec<[f64; 2]>,
}

impl Overrun {
    /// "brim doesn't fit on the bed": the short reason for a header.
    pub fn short(&self) -> String {
        format!("{} doesn't fit on the bed", self.kind.name())
    }
}

/// Overruns smaller than this are not reported before slicing (the estimate's own error).
const PREFLIGHT_TOLERANCE: f64 = 0.05;

/// "The brim of Cube goes 8.0 mm past the left edge of the bed" (`predicted`: "would go").
pub fn describe(object: &str, o: &Overrun, predicted: bool) -> String {
    let what = match o.kind {
        Kind::Brim => format!("The brim of {object}"),
        Kind::Skirt => format!("The skirt around {object}"),
    };
    let verb = if predicted { "would go" } else { "goes" };
    let mut parts: Vec<String> = o
        .sides
        .iter()
        .take(2)
        .map(|(side, d)| format!("{d:.1} mm past the {} edge", side.name()))
        .collect();
    if parts.is_empty() {
        parts.push("past the edge".into());
    }
    format!("{what} {verb} {} of the bed", parts.join(" and "))
}

/// What to do about it: "move it 8.0 mm to the right, or make the brim narrower".
pub fn advice(o: &Overrun) -> String {
    let narrower = match o.kind {
        Kind::Brim => "make the brim narrower",
        Kind::Skirt => "bring the skirt closer or use fewer loops",
    };
    match o.fix {
        Some(shift) => format!("move it {}, or {narrower}", shift_text(shift)),
        None => format!("it does not fit on the bed even centered; {narrower}"),
    }
}

/// [`describe`] and [`advice`] in one sentence: the message the CLI prints and the GUI shows.
pub fn explain(object: &str, o: &Overrun, predicted: bool) -> String {
    format!("{}; {}", describe(object, o, predicted), advice(o))
}

/// "8.0 mm to the right and 2.0 mm to the back"
pub fn shift_text(shift: [f64; 2]) -> String {
    let mut parts = vec![];
    if shift[0].abs() >= 0.05 {
        parts.push(format!(
            "{:.1} mm to the {}",
            shift[0].abs(),
            if shift[0] > 0. { "right" } else { "left" }
        ));
    }
    if shift[1].abs() >= 0.05 {
        parts.push(format!(
            "{:.1} mm to the {}",
            shift[1].abs(),
            if shift[1] > 0. { "back" } else { "front" }
        ));
    }
    if parts.is_empty() {
        "a little inwards".into()
    } else {
        parts.join(" and ")
    }
}

/// How far past each edge a box (`min`, `max`, bed mm) reaches, and the move that fits it.
fn overrun_of(
    min: [f64; 2],
    max: [f64; 2],
    bed: Bed,
    tolerance: f64,
) -> (Vec<(Side, f64)>, Option<[f64; 2]>) {
    let past = [
        (Side::Left, bed.min[0] - min[0]),
        (Side::Right, max[0] - bed.max[0]),
        (Side::Front, bed.min[1] - min[1]),
        (Side::Back, max[1] - bed.max[1]),
    ];
    let mut sides: Vec<(Side, f64)> = past.into_iter().filter(|(_, d)| *d > tolerance).collect();
    sides.sort_by(|a, b| b.1.total_cmp(&a.1));
    let axis = |lo: f64, hi: f64| -> Option<f64> {
        match (lo > 0., hi > 0.) {
            (true, true) => None,
            // A hair more than the overrun, so the moved object is inside, not on the edge.
            (true, false) if hi + lo + 0.01 <= 0. => Some(lo + 0.01),
            (true, false) => None,
            (false, true) if lo + hi + 0.01 <= 0. => Some(-(hi + 0.01)),
            (false, true) => None,
            (false, false) => Some(0.),
        }
    };
    let fix = axis(past[0].1, past[1].1)
        .zip(axis(past[2].1, past[3].1))
        .map(|(x, y)| [x, y]);
    (sides, fix)
}

/// The convex hull of a mesh's section at height `z` (bed mm), with `place` mapping mesh
/// vertices to the bed: the first layer's outline as the brim and skirt surround it. Empty
/// when the mesh does not reach that height.
pub fn section_hull(
    mesh: &TriangleMesh,
    place: &dyn Fn([f32; 3]) -> [f64; 3],
    z: f64,
) -> Vec<[f64; 2]> {
    let placed: Vec<[f64; 3]> = mesh.vertices.iter().map(|&v| place(v)).collect();
    let mut points = Vec::new();
    for f in &mesh.faces {
        let Some(v) = f
            .iter()
            .map(|&i| placed.get(i as usize))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        for k in 0..3 {
            let (a, b) = (v[k], v[(k + 1) % 3]);
            if (a[2] - z) * (b[2] - z) <= 0. && a[2] != b[2] {
                let t = (z - a[2]) / (b[2] - a[2]);
                points.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            } else if a[2] == z {
                points.push([a[0], a[1]]);
            }
        }
    }
    hull(points)
}

/// Convex hull (monotone chain, counter-clockwise).
fn hull(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.retain(|p| p[0].is_finite() && p[1].is_finite());
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for &p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0. {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0. {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// How far the adhesion reaches beyond an object's first-layer section: the outer brim's
/// edge (PrusaSlicer's whole loops of `spacing` from the elephant-foot-compensated outline,
/// Orca's brim filling its width), and the skirt's outer edge when there is one (around the
/// brim, as `brim::generate` places it).
pub fn reach(settings: &crate::settings::PrintSettings) -> Option<(Kind, f64)> {
    let s = &settings.adhesion;
    let h = settings.first_layer_height;
    if s.extrusion_width <= h * (1. - std::f64::consts::FRAC_PI_4) {
        return None;
    }
    let flow = Flow::new(s.extrusion_width, h);
    let spacing = flow.spacing();
    let efc = if !s.orca || s.brim_efc {
        settings.elefant_foot_compensation.max(0.)
    } else {
        0.
    };
    let brim = match s.brim.kind {
        BrimType::Outer | BrimType::OuterAndInner | BrimType::Ears if s.brim.width > 0. => {
            if s.orca {
                s.brim.separation + s.brim.width - efc
            } else {
                let loops = (s.brim.width / spacing).floor();
                if loops < 1. {
                    0.
                } else {
                    s.brim.separation + loops * spacing - spacing / 2. + s.extrusion_width / 2.
                        - efc
                }
            }
        }
        _ => 0.,
    };
    let loops = if s.draft_shield == DraftShield::Enabled {
        s.skirts.max(1)
    } else {
        s.skirts
    };
    let skirt = loops > 0 && (s.skirt_height > 0 || s.draft_shield == DraftShield::Enabled);
    if skirt {
        // A draft shield keeps its distance from the objects; the brim is trimmed around it.
        let base = if s.draft_shield == DraftShield::Disabled {
            brim.max(0.)
        } else {
            0.
        };
        let outer = s.skirt_distance + loops as f64 * spacing + (s.extrusion_width - spacing) / 2.;
        return Some((Kind::Skirt, base + outer));
    }
    (brim > 0.).then_some((Kind::Brim, brim))
}

/// The pre-slice check for one object: its first-layer section (`mesh` placed on the bed by
/// `place`) grown by its brim and the skirt around it, against the bed of `config` (the
/// object's own configuration). `None` when it fits, or when the object has no adhesion.
pub fn preflight(
    mesh: &TriangleMesh,
    place: &dyn Fn([f32; 3]) -> [f64; 3],
    config: &Config,
) -> Option<Overrun> {
    let bed = Bed::from_config(config)?;
    let settings = crate::settings::PrintSettings::from_config(config).ok()?;
    let (kind, reach) = reach(&settings)?;
    let hull = section_hull(mesh, place, settings.first_layer_height / 2.);
    if hull.len() < 3 {
        return None;
    }
    let outline = grow(&hull, reach);
    // The export tests extrusion centerlines (`BuildVolume::all_paths_inside`): the outermost
    // loop's center lies half its width inside the outline's edge.
    let half = settings.adhesion.extrusion_width / 2.;
    let (mut min, mut max) = bounds(&outline);
    for i in 0..2 {
        min[i] += half;
        max[i] -= half;
    }
    let (sides, fix) = overrun_of(min, max, bed, PREFLIGHT_TOLERANCE);
    if sides.is_empty() {
        return None;
    }
    Some(Overrun {
        kind,
        sides,
        fix,
        outline,
    })
}

/// `hull` grown by `distance` with round corners (a convex outline stays convex).
fn grow(hull: &[[f64; 2]], distance: f64) -> Vec<[f64; 2]> {
    const STEPS: usize = 24;
    let mut points = Vec::with_capacity(hull.len() * STEPS);
    for p in hull {
        for k in 0..STEPS {
            let a = std::f64::consts::TAU * k as f64 / STEPS as f64;
            // The polygon's corners circumscribe the circle, so straight edges reach `distance`.
            let r = distance / (std::f64::consts::PI / STEPS as f64).cos();
            points.push([p[0] + r * a.cos(), p[1] + r * a.sin()]);
        }
    }
    let mut out = self::hull(points);
    // Exact extremes along the axes, which the bed check reads.
    let (min, max) = bounds(hull);
    let (gmin, gmax) = bounds(&out);
    if gmin[0] < min[0] - distance - 1e-9 || gmax[0] > max[0] + distance + 1e-9 {
        for p in &mut out {
            p[0] = p[0].clamp(min[0] - distance, max[0] + distance);
        }
    }
    if gmin[1] < min[1] - distance - 1e-9 || gmax[1] > max[1] + distance + 1e-9 {
        for p in &mut out {
            p[1] = p[1].clamp(min[1] - distance, max[1] + distance);
        }
    }
    out
}

fn bounds(points: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for p in points {
        for i in 0..2 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    (min, max)
}

/// An object of the plate, for naming the owner of a skirt or brim path.
pub struct Owner<'a> {
    pub name: &'a str,
    /// XY bounds of its placed model (bed mm).
    pub min: [f64; 2],
    pub max: [f64; 2],
}

/// The generated skirt and brim of a prepared plate against the bed, by the exporter's own
/// rule (every centerline point on the bed outline within `BedEpsilon`, as
/// `BuildVolume::all_paths_inside`). `brim_owner` maps a brim path to its object (Orca's
/// per-object brims), otherwise the nearest object owns it. Returns the owner's index and the
/// overrun past the bed's edges, the farthest one when several pass. A path off a round or
/// polygonal bed but inside its bounding box has no edge to name; the export refuses it.
pub fn plan_overrun(
    plan: &AdhesionPlan,
    owners: &[Owner<'_>],
    volume: &BuildVolume,
) -> Option<(usize, Overrun)> {
    if owners.is_empty() {
        return None;
    }
    let nearest = |x: f64, y: f64| -> usize {
        let d = |o: &Owner<'_>| {
            let dx = (o.min[0] - x).max(x - o.max[0]).max(0.);
            let dy = (o.min[1] - y).max(y - o.max[1]).max(0.);
            dx.hypot(dy)
        };
        (0..owners.len())
            .min_by(|&a, &b| d(&owners[a]).total_cmp(&d(&owners[b])))
            .unwrap_or(0)
    };
    // Per (owner, kind): the farthest reach past each edge.
    let mut found: Vec<(usize, Kind, [f64; 2], [f64; 2])> = Vec::new();
    let paths = plan
        .brim
        .iter()
        .enumerate()
        .map(|(i, p)| (Kind::Brim, plan.brim_objects.get(i).map(|&o| o as usize), p))
        .chain(plan.skirt.iter().map(|p| (Kind::Skirt, None, p)));
    let bed = Bed {
        min: volume.min,
        max: volume.max,
    };
    for (kind, owner, path) in paths {
        for p in &path.points {
            let (x, y) = (unscale(p.x), unscale(p.y));
            if volume.contains_xy(x, y, BED_EPSILON) {
                continue;
            }
            let owner = owner
                .filter(|&o| o < owners.len())
                .unwrap_or_else(|| nearest(x, y));
            let entry = match found.iter_mut().position(|f| f.0 == owner && f.1 == kind) {
                Some(i) => &mut found[i],
                None => {
                    found.push((owner, kind, [f64::INFINITY; 2], [f64::NEG_INFINITY; 2]));
                    found.last_mut().expect("just pushed")
                }
            };
            entry.2 = [entry.2[0].min(x), entry.2[1].min(y)];
            entry.3 = [entry.3[0].max(x), entry.3[1].max(y)];
        }
    }
    found
        .into_iter()
        .map(|(owner, kind, min, max)| {
            // Include the object itself, so the move that fits the paths keeps it on the bed.
            let o = &owners[owner];
            let (sides, _) = overrun_of(min, max, bed, 0.);
            let all_min = [min[0].min(o.min[0]), min[1].min(o.min[1])];
            let all_max = [max[0].max(o.max[0]), max[1].max(o.max[1])];
            let (_, fix) = overrun_of(all_min, all_max, bed, 0.);
            (
                owner,
                Overrun {
                    kind,
                    sides,
                    fix,
                    outline: Vec::new(),
                },
            )
        })
        .filter(|(_, o)| !o.sides.is_empty())
        .max_by(|a, b| a.1.sides[0].1.total_cmp(&b.1.sides[0].1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bed() -> Bed {
        Bed {
            min: [0., 0.],
            max: [250., 220.],
        }
    }

    #[test]
    fn overrun_names_side_distance_and_fix() {
        let (sides, fix) = overrun_of([-8., 100.], [22., 120.], bed(), 0.);
        assert_eq!(sides, vec![(Side::Left, 8.)]);
        let fix = fix.unwrap();
        assert!((fix[0] - 8.01).abs() < 1e-9 && fix[1] == 0.);
        let o = Overrun {
            kind: Kind::Brim,
            sides,
            fix: Some(fix),
            outline: vec![],
        };
        assert_eq!(
            explain("Cube", &o, false),
            "The brim of Cube goes 8.0 mm past the left edge of the bed; move it 8.0 mm to the \
             right, or make the brim narrower"
        );
        assert_eq!(o.short(), "brim doesn't fit on the bed");
    }

    #[test]
    fn too_wide_for_the_bed_has_no_fix() {
        let (sides, fix) = overrun_of([-5., 10.], [260., 20.], bed(), 0.);
        assert_eq!(sides.len(), 2);
        assert_eq!(sides[0].0, Side::Right);
        assert!(fix.is_none());
    }

    /// A cube 2 mm from the bed's left edge with a 10 mm brim: the print builder warns before
    /// preparing and refuses after, naming the object, the edge and the distance; moved by the
    /// suggested distance it prints.
    #[test]
    fn the_print_builder_warns_then_refuses_a_brim_past_the_bed() {
        let mut config = Config::defaults();
        config
            .set_raw("bed_shape", "0x0,250x0,250x220,0x220")
            .unwrap();
        config.set_raw("brim_width", "10").unwrap();
        config.set_raw("skirts", "0").unwrap();
        config.set_raw("use_relative_e_distances", "1").unwrap();
        let cube = |x: f32| {
            let mut mesh = crate::mesh::TriangleMesh::load_stl(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/models/cube20.stl"),
            )
            .unwrap();
            let (min, _) = mesh.bounds();
            mesh.translate([x - min[0], 100. - min[1], -min[2]]);
            crate::input::InputObject::plain(
                "Cube",
                0,
                0,
                mesh,
                crate::print_job::ToolAssignments([None; 5]),
                config.clone(),
            )
        };
        let silent = |_: crate::input::Preparing<'_>| -> Box<dyn crate::progress::Observer> {
            Box::new(crate::progress::Silent)
        };
        let plate = crate::input::prepare_plate(vec![cube(2.)], &config, &silent).unwrap();
        let warning = plate
            .warnings_for_test()
            .iter()
            .find(|w| w.contains("would go"))
            .cloned();
        let warning = warning.expect("a warning before slicing");
        assert!(
            warning.starts_with("The brim of Cube would go ") && warning.contains("left edge"),
            "{warning}"
        );
        let error = match plate.finish(&config, Default::default()) {
            Ok(_) => panic!("a brim past the bed must not print"),
            Err(e) => e,
        };
        assert!(
            error.starts_with("The brim of Cube goes ")
                && error.contains("mm past the left edge of the bed; move it ")
                && error.ends_with("mm to the right, or make the brim narrower"),
            "{error}"
        );
        // The distance it names is enough.
        let distance: f32 = error["The brim of Cube goes ".len()..]
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(distance > 5. && distance < 10., "{error}");
        let moved = crate::input::build_print(
            vec![cube(2. + distance + 0.1)],
            &config,
            Default::default(),
            &silent,
        );
        assert!(moved.is_ok(), "{:?}", moved.err());
    }

    #[test]
    fn section_hull_of_a_cube_is_its_square() {
        let mut mesh = crate::mesh::TriangleMesh::load_stl(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl"),
        )
        .unwrap();
        let (min, _) = mesh.bounds();
        mesh.translate([2. - min[0], 100. - min[1], -min[2]]);
        let hull = section_hull(&mesh, &|v| v.map(f64::from), 0.1);
        let (min, max) = bounds(&hull);
        assert!(
            (min[0] - 2.).abs() < 1e-6 && (max[0] - 22.).abs() < 1e-6,
            "{min:?} {max:?}"
        );
        // Grown by 10 mm its left edge reaches −8 mm, 8 mm past the left edge.
        let grown = grow(&hull, 10.);
        let (gmin, _) = bounds(&grown);
        assert!((gmin[0] + 8.).abs() < 1e-6, "{gmin:?}");
    }
}
