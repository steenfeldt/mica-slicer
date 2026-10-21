// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Checks brick-layers G-code against the product rules of
//! [`mica_core::brick_layers`]: it reads the program as a printer would and compares it with
//! the same print sliced without brick layers.
//!
//! - **Bead height**: no extrusion lands on material less than a layer below the nozzle. A
//!   grid of 0.1 mm cells keeps the top of the material laid down along each extrusion's
//!   centreline; every extrusion point is measured against the highest top *below* the
//!   nozzle among centreline points within 0.1 mm (material at the nozzle's own height is the same layer's neighbour).
//!   The 0.3 mm at either end of an extrusion are not measured: they touch the stretch they
//!   continue.
//! - **Declared height**: every extrusion's `;HEIGHT:` is at least its layer's height.
//! - **Flow**: every wall extrusion's filament per mm is at least that of the same width and
//!   height without brick layers (the flow of the reference print, per mm³ of bead).
//! - **Order**: on each layer, nothing prints at the layer's Z after a raised wall (one object
//!   per plate; [`check_with`] leaves this out for plates with several objects).
//! - **Shells**: the extrusions of the first `bottom` and last `top` layers are identical,
//!   line for line, to those of the reference print.

use std::collections::HashMap;

/// One extrusion move.
#[derive(Clone, Debug)]
pub struct Extrusion {
    pub layer: usize,
    pub layer_z: f64,
    pub z: f64,
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub length: f64,
    pub e: f64,
    /// Arc moves: centre, radius, start angle and signed sweep.
    pub arc: Option<[f64; 5]>,
    pub role: String,
    pub width: f64,
    pub height: f64,
    pub line: String,
}

/// A G-code program's extrusions and layers.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub extrusions: Vec<Extrusion>,
    /// `(z, height)` of each `;LAYER_CHANGE`.
    pub layers: Vec<(f64, f64)>,
}

fn word(line: &str, c: char) -> Option<f64> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(c).and_then(|v| v.parse::<f64>().ok()))
}

/// Reads the moves of text G-code (relative or absolute E).
pub fn parse(gcode: &str) -> Program {
    let mut p = Program::default();
    let (mut x, mut y, mut z, mut e) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut relative_e = true;
    let (mut role, mut width, mut height) = (String::new(), 0.0, 0.0);
    let mut layer_z = 0.0;
    for raw in gcode.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if raw.starts_with(";LAYER_CHANGE") {
            p.layers.push((0.0, 0.0));
            continue;
        }
        if let Some(v) = raw.strip_prefix(";Z:") {
            if let (Some(l), Ok(v)) = (p.layers.last_mut(), v.trim().parse::<f64>()) {
                l.0 = v;
                layer_z = v;
            }
            continue;
        }
        if let Some(v) = raw.strip_prefix(";HEIGHT:") {
            if let Ok(v) = v.trim().parse::<f64>() {
                height = v;
                if let Some(l) = p.layers.last_mut()
                    && l.1 == 0.0
                {
                    l.1 = v;
                }
            }
            continue;
        }
        if let Some(v) = raw.strip_prefix(";WIDTH:") {
            width = v.trim().parse().unwrap_or(width);
            continue;
        }
        if let Some(v) = raw.strip_prefix(";TYPE:") {
            role = v.trim().to_string();
            continue;
        }
        if line.starts_with("M83") {
            relative_e = true;
        } else if line.starts_with("M82") {
            relative_e = false;
        } else if line.starts_with("G92") {
            if let Some(v) = word(line, 'E') {
                e = v;
            }
        } else if let Some(cmd) = ["G0 ", "G1 ", "G2 ", "G3 "]
            .iter()
            .find(|c| line.starts_with(**c) || line == c.trim())
        {
            let nx = word(line, 'X').unwrap_or(x);
            let ny = word(line, 'Y').unwrap_or(y);
            let nz = word(line, 'Z').unwrap_or(z);
            let de = match word(line, 'E') {
                Some(v) if relative_e => v,
                Some(v) => {
                    let d = v - e;
                    e = v;
                    d
                }
                None => 0.0,
            };
            let chord = (nx - x).hypot(ny - y);
            let mut arc = None;
            let length = if cmd.starts_with("G2") || cmd.starts_with("G3") {
                let (i, j) = (
                    word(line, 'I').unwrap_or(0.0),
                    word(line, 'J').unwrap_or(0.0),
                );
                let r = i.hypot(j);
                let (cx, cy) = (x + i, y + j);
                let a0 = (y - cy).atan2(x - cx);
                let a1 = (ny - cy).atan2(nx - cx);
                let mut sweep = a1 - a0;
                let ccw = cmd.starts_with("G3");
                if ccw && sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                if !ccw && sweep >= 0.0 {
                    sweep -= std::f64::consts::TAU;
                }
                arc = Some([cx, cy, r, a0, sweep]);
                r * sweep.abs()
            } else {
                chord
            };
            if de > 0.0 && length > 1e-6 && !p.layers.is_empty() {
                p.extrusions.push(Extrusion {
                    layer: p.layers.len() - 1,
                    layer_z,
                    z: nz,
                    from: (x, y),
                    to: (nx, ny),
                    length,
                    e: de,
                    arc,
                    role: role.clone(),
                    width,
                    height,
                    line: line.to_string(),
                });
            }
            x = nx;
            y = ny;
            z = nz;
        }
    }
    p
}

/// What [`check`] found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Rule violations (empty: the program passes).
    pub problems: Vec<String>,
    /// Extrusions printed above their layer's Z (raised walls).
    pub raised: usize,
    /// Smallest measured bead height relative to the layer height (1.0 = one layer).
    pub min_relative_height: f64,
    /// Largest measured raised bead height relative to the layer height.
    pub max_relative_height: f64,
    /// Smallest wall flow relative to the reference print's flow for the same bead.
    pub min_flow_ratio: f64,
}

/// A centreline point and the top of the bead there: (x, y, top).
type Sample = (f64, f64, f64);

fn rounded_area(width: f64, height: f64) -> f64 {
    height * (width - height * (1.0 - std::f64::consts::FRAC_PI_4))
}

fn is_wall(role: &str) -> bool {
    matches!(role, "Perimeter" | "Inner wall")
}

/// Checks `on` (brick layers) against `off` (the same print without them). `bottom` and `top`
/// are the shell layer counts that must not change. One object per print.
pub fn check(on: &str, off: &str, bottom: usize, top: usize) -> Report {
    check_with(on, off, bottom, top, true)
}

/// [`check`]; with `one_object` false (several objects on the plate) the order rule is not
/// checked, since the next object's walls print at layer height after this one's raised ones.
pub fn check_with(on: &str, off: &str, bottom: usize, top: usize, one_object: bool) -> Report {
    let a = parse(on);
    let b = parse(off);
    let mut r = Report {
        min_relative_height: f64::INFINITY,
        max_relative_height: 0.0,
        min_flow_ratio: f64::INFINITY,
        ..Default::default()
    };
    if a.layers.len() != b.layers.len() {
        r.problems.push(format!(
            "{} layers with brick layers, {} without",
            a.layers.len(),
            b.layers.len()
        ));
        return r;
    }
    let n = a.layers.len();
    let h = |l: usize| a.layers[l].1;
    let nominal = h;
    const EPS: f64 = 1e-3;
    // Centrelines closer than this stand on each other.
    const RADIUS: f64 = 0.1;

    // Flow of the reference print per mm³ of bead, for walls.
    let measured = |x: &&Extrusion| {
        is_wall(&x.role) && x.width > 0.0 && x.height > 0.0 && x.layer > 0 && x.length >= 1.0
    };
    let mut flows: Vec<f64> = b
        .extrusions
        .iter()
        .filter(measured)
        .map(|x| x.e / x.length / rounded_area(x.width, x.height))
        .collect();
    flows.sort_by(f64::total_cmp);
    let reference = flows.get(flows.len() / 2).copied().unwrap_or(f64::NAN);

    // Bead tops along centrelines, per 0.1 mm cell.
    let cell = |p: (f64, f64)| ((p.0 * 10.0).floor() as i64, (p.1 * 10.0).floor() as i64);
    // Each cell keeps the centreline samples that ended in it: (x, y, top).
    let mut tops: HashMap<(i64, i64), Vec<Sample>> = HashMap::new();
    let mut raised_seen: Option<usize> = None;
    for x in &a.extrusions {
        let layer_h = h(x.layer);
        if x.height < nominal(x.layer) - EPS && x.layer > 0 {
            r.problems.push(format!(
                "layer {}: declared height {} below the layer height {} ({})",
                x.layer,
                x.height,
                nominal(x.layer),
                x.line
            ));
        }
        if measured(&x) && reference.is_finite() {
            let ratio = x.e / x.length / rounded_area(x.width, x.height) / reference;
            r.min_flow_ratio = r.min_flow_ratio.min(ratio);
            if ratio < 1.0 - 5e-3 {
                r.problems.push(format!(
                    "layer {}: wall flow {:.4} of the reference for its bead ({})",
                    x.layer, ratio, x.line
                ));
            }
        }
        let raised = x.z > x.layer_z + EPS;
        if raised {
            r.raised += 1;
            raised_seen = Some(x.layer);
        } else if one_object && raised_seen == Some(x.layer) {
            r.problems.push(format!(
                "layer {}: an extrusion at the layer's Z after a raised wall ({})",
                x.layer, x.line
            ));
        }
        // Measured bead height along the centreline.
        let steps = (x.length / 0.05).ceil().max(1.0) as usize;
        let mut samples = Vec::with_capacity(steps + 1);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let along = t * x.length;
            let p = match x.arc {
                Some([cx, cy, radius, a0, sweep]) => {
                    let a = a0 + sweep * t;
                    (cx + radius * a.cos(), cy + radius * a.sin())
                }
                None => (
                    x.from.0 + (x.to.0 - x.from.0) * t,
                    x.from.1 + (x.to.1 - x.from.1) * t,
                ),
            };
            let c = cell(p);
            samples.push((c, p));
            if x.layer == 0 || along < 0.3 || along > x.length - 0.3 {
                continue;
            }
            let mut below = f64::NEG_INFINITY;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(v) = tops.get(&(c.0 + dx, c.1 + dy)) {
                        for &(sx, sy, t) in v {
                            if t < x.z - EPS && (sx - p.0).hypot(sy - p.1) <= RADIUS {
                                below = below.max(t);
                            }
                        }
                    }
                }
            }
            if below.is_finite() {
                let span = x.z - below;
                let rel = span / layer_h;
                r.min_relative_height = r.min_relative_height.min(rel);
                if raised {
                    r.max_relative_height = r.max_relative_height.max(rel);
                }
                if span < nominal(x.layer) - EPS {
                    r.problems.push(format!(
                        "layer {}: bead {:.3} mm tall at ({:.2}, {:.2}) ({})",
                        x.layer, span, p.0, p.1, x.line
                    ));
                }
            }
        }
        for (c, p) in samples {
            let v = tops.entry(c).or_default();
            // One sample per cell and height is enough at this resolution.
            if !v
                .iter()
                .any(|&(sx, sy, t)| (t - x.z).abs() < 1e-6 && (sx - p.0).hypot(sy - p.1) < 0.03)
            {
                v.push((p.0, p.1, x.z));
            }
        }
    }

    // Shells: extrusions identical line for line.
    let lines = |p: &Program, l: usize| -> Vec<String> {
        p.extrusions
            .iter()
            .filter(|x| x.layer == l)
            .map(|x| x.line.clone())
            .collect()
    };
    for l in (0..bottom.min(n)).chain(n.saturating_sub(top)..n) {
        if lines(&a, l) != lines(&b, l) {
            r.problems.push(format!(
                "layer {l}: extrusions differ from the print without brick layers"
            ));
        }
    }
    r.problems.truncate(50);
    r
}
