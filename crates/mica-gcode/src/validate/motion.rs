// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;
use std::f64::consts::{PI, TAU};

pub(super) fn named<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .filter_map(|s| s.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v))
}
pub(super) fn command(line: &str) -> (String, BTreeMap<char, f64>) {
    let line = line.split('*').next().unwrap_or(line).trim();
    let line = if line.starts_with('N') {
        line.find(char::is_whitespace)
            .map_or(line, |i| line[i..].trim())
    } else {
        line
    };
    let end = if line
        .as_bytes()
        .first()
        .is_some_and(|c| matches!(c.to_ascii_uppercase(), b'G' | b'M' | b'T'))
        && line.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
    {
        line.char_indices()
            .skip(1)
            .find(|(_, c)| !c.is_ascii_digit() && *c != '.')
            .map_or(line.len(), |(i, _)| i)
    } else {
        line.find(char::is_whitespace).unwrap_or(line.len())
    };
    let cmd = line[..end].to_uppercase();
    let mut args = BTreeMap::new();
    if !cmd.starts_with(['G', 'M', 'T']) || cmd.contains('_') {
        return (cmd, args);
    }
    let mut rest = line[end..].trim();
    while let Some(key) = rest.chars().next() {
        rest = &rest[key.len_utf8()..];
        if !key.is_ascii_alphabetic() {
            continue;
        }
        rest = rest.trim_start();
        let n = if rest.starts_with("NaN") || rest.starts_with("inf") {
            3
        } else {
            rest.find(|c: char| !c.is_ascii_digit() && !matches!(c, '.' | '-' | '+'))
                .unwrap_or(rest.len())
        };
        if n == 0
            && matches!(
                key.to_ascii_uppercase(),
                'X' | 'Y' | 'Z' | 'E' | 'F' | 'S' | 'T' | 'P' | 'Q' | 'I' | 'J' | 'R'
            )
        {
            args.insert(key.to_ascii_uppercase(), f64::NAN);
        }
        if n > 0 {
            args.insert(
                key.to_ascii_uppercase(),
                rest[..n].parse().unwrap_or(f64::NAN),
            );
            rest = &rest[n..];
        }
    }
    (cmd, args)
}

pub(crate) fn inside(p: (f64, f64), polygon: &[(f64, f64)]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    for (a, b) in polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
    {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let t =
            (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy).max(1e-20)).clamp(0., 1.);
        if (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy) <= 0.02 {
            return true;
        }
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}
impl Simulator<'_> {
    pub(super) fn movement(&mut self, command: &str, args: &BTreeMap<char, f64>) {
        let start = self.xyz;
        let was_known = self.known;
        for (i, axis) in ['X', 'Y', 'Z'].into_iter().enumerate() {
            if let Some(v) = args.get(&axis) {
                self.xyz[i] = if self.absolute {
                    v * self.unit
                } else {
                    self.xyz[i] + v * self.unit
                };
                self.known[i] = self.absolute || self.known[i];
            }
        }
        if self.xyz.iter().any(|v| !v.is_finite() || v.abs() > 1e7) {
            self.error("motion", "coordinate outside safe numeric range");
            return;
        }
        let mut de = 0.;
        if let Some(e) = args.get(&'E') {
            let e = e * self.unit;
            de = if self.relative_e { e } else { e - self.e };
            self.e = if self.relative_e { self.e + e } else { e };
        }
        if de.abs() > 100. {
            self.error(
                "huge-extrusion",
                format!("single move E delta {de:.3} mm exceeds 100 mm"),
            );
        }
        let Some(profile) = self.p.tools.get(self.tool) else {
            self.error("profile", "profile has no active tool");
            return;
        };
        if de < 0. {
            self.tools[self.tool].retracted -= de;
            if -de > profile.retract_limit {
                self.error(
                    "retraction",
                    format!("retraction {:.2} mm exceeds profile safety limit", -de),
                );
            }
        }
        let mut deposited = de.max(0.);
        if de > 0. {
            let restored = deposited.min(self.tools[self.tool].retracted);
            self.tools[self.tool].retracted -= restored;
            deposited -= restored;
        }
        let printing = self.layer_z.is_some() && !self.custom;
        let mut distance = (self.xyz[0] - start[0]).hypot(self.xyz[1] - start[1]);
        let mut samples = Vec::new();
        if matches!(command, "G2" | "G02" | "G3" | "G03") && was_known[0] && was_known[1] {
            if args.contains_key(&'R') {
                self.warn(
                    "arc",
                    "R-format arc not simulated; use I/J arcs for complete validation",
                );
            } else {
                let center = (
                    start[0] + args.get(&'I').unwrap_or(&0.) * self.unit,
                    start[1] + args.get(&'J').unwrap_or(&0.) * self.unit,
                );
                let radius = (start[0] - center.0).hypot(start[1] - center.1);
                let end_radius = (self.xyz[0] - center.0).hypot(self.xyz[1] - center.1);
                if radius < 1e-6 || (end_radius - radius).abs() > 0.1 {
                    self.error("arc", "arc radii disagree or radius is zero");
                    return;
                }
                let from = (start[1] - center.1).atan2(start[0] - center.0);
                let to = (self.xyz[1] - center.1).atan2(self.xyz[0] - center.0);
                let cw = matches!(command, "G2" | "G02");
                let mut sweep = if cw {
                    (from - to).rem_euclid(TAU)
                } else {
                    (to - from).rem_euclid(TAU)
                };
                if sweep < 1e-8 {
                    sweep = TAU;
                }
                distance = radius * sweep;
                let n = ((sweep / 0.02).ceil() as usize).max(1);
                for i in 0..=n {
                    let a = from + if cw { -1. } else { 1. } * sweep * i as f64 / n as f64;
                    samples.push((center.0 + radius * a.cos(), center.1 + radius * a.sin()));
                }
            }
        }
        if samples.is_empty() && was_known[0] && was_known[1] {
            // Include every polygon boundary crossing, then test each interval midpoint.
            let a = (start[0], start[1]);
            let b = (self.xyz[0], self.xyz[1]);
            let mut ts = vec![0., 1.];
            for polygon in std::iter::once(&self.p.bed).chain(self.p.exclusions.iter()) {
                for (c, d) in polygon
                    .iter()
                    .zip(polygon.iter().cycle().skip(1))
                    .take(polygon.len())
                {
                    let cross = |u: (f64, f64), v: (f64, f64)| u.0 * v.1 - u.1 * v.0;
                    let ab = (b.0 - a.0, b.1 - a.1);
                    let cd = (d.0 - c.0, d.1 - c.1);
                    let ac = (c.0 - a.0, c.1 - a.1);
                    let det = cross(ab, cd);
                    if det.abs() > 1e-12 {
                        let t = cross(ac, cd) / det;
                        let u = cross(ac, ab) / det;
                        if (0. ..=1.).contains(&t) && (0. ..=1.).contains(&u) {
                            ts.push(t);
                        }
                    }
                }
            }
            ts.sort_by(f64::total_cmp);
            for w in ts.windows(2) {
                let t = (w[0] + w[1]) * 0.5;
                samples.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
        }
        samples.push((self.xyz[0], self.xyz[1]));
        if printing && (self.printed || deposited > 0.001) && self.known[0] && self.known[1] {
            if samples.iter().any(|&p| !inside(p, &self.p.bed)) {
                self.error(
                    "print-volume",
                    format!(
                        "motion outside bed at X{:.3} Y{:.3}",
                        self.xyz[0], self.xyz[1]
                    ),
                );
            }
            if samples
                .iter()
                .any(|&p| self.p.exclusions.iter().any(|poly| inside(p, poly)))
            {
                self.error("exclusion-zone", "motion crosses a bed exclusion zone");
            }
        }
        if printing && self.known[2] {
            if self.xyz[2] < -0.01 || self.xyz[2] > self.p.max_z + 0.01 {
                self.error(
                    "print-height",
                    format!("Z{:.3} outside 0–{} mm", self.xyz[2], self.p.max_z),
                );
            }
            if !self.tower && !self.wipe && self.xyz[2] < self.floor_z - 0.02 {
                self.warn(
                    "below-layer",
                    format!(
                        "Z{:.3} below layer {:.3}; inspect scarf/toolchange motion",
                        self.xyz[2], self.floor_z
                    ),
                );
            }
        }
        // A deretraction in place (no XY) after leaving the tower still carries the tower's
        // ;TYPE until the next path; it is not tower geometry.
        if self.tower && deposited > 0. && (start[0] != self.xyz[0] || start[1] != self.xyz[1]) {
            self.tower_z = self.tower_z.max(self.xyz[2]);
            let bounds = self
                .tower_bounds
                .get_or_insert([(self.xyz[0], self.xyz[1]); 2]);
            for p in [(start[0], start[1]), (self.xyz[0], self.xyz[1])] {
                bounds[0].0 = bounds[0].0.min(p.0);
                bounds[0].1 = bounds[0].1.min(p.1);
                bounds[1].0 = bounds[1].0.max(p.0);
                bounds[1].1 = bounds[1].1.max(p.1);
            }
        }
        if printing
            && !self.tower
            && self.xyz[2] < self.tower_z + 0.01
            && let Some([min, max]) = self.p.tower
            && samples
                .iter()
                .any(|p| p.0 > min.0 && p.0 < max.0 && p.1 > min.1 && p.1 < max.1)
        {
            self.error(
                "tower-collision",
                "non-tower motion intersects tower at its printed height",
            );
        }
        if printing
            && !self.tower
            && self.xyz[2] < self.tower_z - 0.01
            && self.p.tower.is_none()
            && let Some([min, max]) = self.tower_bounds
        {
            let inside = |p: (f64, f64)| {
                p.0 > min.0 + 0.02 && p.0 < max.0 - 0.02 && p.1 > min.1 + 0.02 && p.1 < max.1 - 0.02
            };
            if samples.iter().any(|&p| inside(p))
                && ((deposited > 0.001 && distance > 0.25)
                    || (!inside((start[0], start[1])) && !inside((self.xyz[0], self.xyz[1]))))
            {
                self.warn("tower-collision", "motion intersects the observed tower bounds below its printed top; inspect rotated/sparse tower clearance");
            }
        }
        if self
            .layer_z
            .is_some_and(|z| self.known[2] && self.xyz[2] >= z - 0.001)
        {
            self.floor_z = self.layer_z.unwrap_or(0.);
        }
        if deposited <= 0.00001 {
            if self.known[2] {
                // Moving across, below the printed height, after having cleared it.
                if self.xyz[2] < self.printed_z - 0.001 && distance > 0.001 {
                    if self.since_extrusion_z > self.printed_z + 0.01 {
                        self.dipped = true;
                    } else {
                        self.low_travel += distance;
                    }
                }
                self.since_extrusion_z = self.since_extrusion_z.max(self.xyz[2]);
            }
            return;
        }
        if printing && self.known[2] {
            self.printed_z = self.printed_z.max(self.xyz[2]);
            self.since_extrusion_z = self.xyz[2];
            self.dipped = false;
            self.low_travel = 0.;
        }
        // Wipe tower blocks may come before the first layer marker (the first tool change).
        if !self.custom && self.role != "Unknown" {
            self.first_layer_tool.get_or_insert(self.tool);
        }
        self.report.filament_mm[self.tool] += deposited;
        if !printing {
            return;
        }
        self.printed = true;
        self.initial_tool.get_or_insert(self.tool);
        // Brick layers raise every other inner wall by up to half a layer.
        let raised_wall = matches!(self.role.as_str(), "Perimeter" | "Inner wall")
            && self
                .layer_z
                .is_some_and(|z| self.xyz[2] <= z + self.p.max_layer / 2. + 0.002);
        if !self.tower
            && !raised_wall
            && distance > 0.25
            && self.known[2]
            && self.layer_z.is_some_and(|z| self.xyz[2] > z + 0.03)
        {
            self.warn(
                "layer-position",
                format!(
                    "extrusion Z{:.3} differs from layer marker {:.3}",
                    self.xyz[2],
                    self.layer_z.unwrap_or(0.)
                ),
            );
        }
        let tool = &self.tools[self.tool];
        if tool.target < profile.min_temperature {
            self.error(
                "cold-extrusion",
                format!(
                    "T{} extrusion at commanded {:.0}°C; minimum {:.0}°C",
                    self.tool, tool.target, profile.min_temperature
                ),
            );
        }
        if self.tools[self.tool].target > profile.max_temperature {
            self.error(
                "hot-extrusion",
                "hotend exceeds filament maximum temperature",
            );
        }
        if self.tools[self.tool].waited < self.tools[self.tool].target - 1.
            && !self.tools[self.tool].used
        {
            self.error(
                "hotend-wait",
                "extrusion before hotend reached a waited printing temperature",
            );
        }
        // The tool that prints the first feature (a role other than custom G-code, the wipe
        // tower included, even ahead of the first layer marker) sets the bed: its filament's
        // first-layer temperature. GUI monkey test, seed 1008: on the XL, T3's wipe tower block
        // and the tool change to T2 come before the first layer marker; the bed is at T3's
        // 60 °C, as the start G-code set it, while T2's filament would want 90 °C. Extrusions
        // before any feature (start G-code purges) need the lowest of the tools' temperatures.
        let tool_bed = |t: usize| {
            self.p
                .tools
                .get(t)
                .map(|t| t.bed_temperature)
                .filter(|t| *t > 0.)
        };
        let bed = match self.first_layer_tool {
            Some(t) => tool_bed(t),
            None => (0..self.p.tools.len())
                .filter_map(tool_bed)
                .reduce(f64::min),
        }
        .unwrap_or(self.p.bed_temperature);
        if bed > 0. && (self.bed_target <= 0. || self.bed_waited < bed - 5.) {
            self.error(
                "bed-wait",
                "printing before bed was set and waited to profile temperature",
            );
        }
        self.tools[self.tool].used = true;
        if self.preextrude_pending {
            self.warn(
                "tool-sequence",
                "U1 extrusion before SM_PRINT_PREEXTRUDE_FILAMENT after tool change",
            );
            self.preextrude_pending = false;
        }
        *self
            .report
            .role_filament_mm
            .entry(self.role.clone())
            .or_default() += deposited;
        if let Some(roles) = self.report.tool_role_filament_mm.get_mut(self.tool) {
            *roles.entry(self.role.clone()).or_default() += deposited;
        }
        if self.role.eq_ignore_ascii_case("travel") {
            self.error(
                "extruding-travel",
                "positive extrusion on a travel-labeled move",
            );
        }
        if distance < 0.001 {
            if deposited > 0.05 && !self.tower && !self.filament_change_prime {
                self.warn("deretraction", format!("{deposited:.3} mm stationary extrusion beyond outstanding retraction (prime/restart extra?)"));
            }
            return;
        }
        self.filament_change_prime = false;
        if !was_known[0]
            || !was_known[1]
            || distance < 0.25
            || self.wipe
            || self.tower
            || self.role.to_lowercase().contains("ironing")
        {
            return;
        }
        let area = mica_core::flow::Flow {
            width: self.width,
            height: self.height,
        }
        .mm3_per_mm();
        if area <= 0. {
            self.warn(
                "flow-model",
                "width/height cannot form a positive rounded extrusion",
            );
            return;
        }
        let expected =
            area * distance * profile.flow_ratio / (PI * profile.diameter * profile.diameter / 4.);
        let ratio = deposited / expected;
        if !(0.75..=1.25).contains(&ratio) && (deposited - expected).abs() > 0.001 {
            self.warn(
                "flow",
                format!(
                    "{} T{}: E{deposited:.5} vs model {expected:.5} mm ({:+.0}%)",
                    self.role,
                    self.tool,
                    (ratio - 1.) * 100.
                ),
            );
        }
    }
}
