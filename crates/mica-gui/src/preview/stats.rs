// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Legend statistics accumulated while the preview file is written: per-feature and per-tool
//! time and length, value ranges for the gradient colour modes, and tool-change layers.

use super::{Attr, Segment, TRAVEL, attrs};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bucket {
    pub time_s: f64,
    pub length_mm: f64,
    pub count: u64,
    /// Extruded volume (path cross-section × length), mm³.
    pub volume_mm3: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
}
impl Default for Range {
    fn default() -> Self {
        Self {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }
}
impl Range {
    pub fn add(&mut self, v: f32) {
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }
    pub fn is_valid(&self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }
    /// A usable `[min, max]`, widened when all values are equal.
    pub fn span(&self, fallback: [f32; 2]) -> [f32; 2] {
        if !self.is_valid() {
            return fallback;
        }
        if (self.max - self.min).abs() < 1e-6 {
            [self.min - 0.5, self.max + 0.5]
        } else {
            [self.min, self.max]
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub roles: [Bucket; 16],
    pub tools: [Bucket; 16],
    pub speed: Range,
    pub fan: Range,
    pub width: Range,
    pub height: Range,
    pub layer_time: Range,
    /// Ranges of the per-record attributes (`attrs.bin`), extrusions only.
    pub actual: Range,
    pub flow: Range,
    pub temp: Range,
    pub accel: Range,
    /// Layer indices at which the active tool changes.
    pub tool_changes: Vec<usize>,
    pub last_tool: Option<usize>,
    pub layer_seconds: f64,
}
impl Stats {
    pub fn add(&mut self, s: &Segment, layer: usize) {
        let role = s.role() as usize;
        let tool = s.tool();
        let speed = s.speed();
        let length = glam::Vec3::from(s.a).distance(glam::Vec3::from(s.b)) as f64;
        let time = if speed > 0. {
            length / speed as f64
        } else {
            0.
        };
        let bucket = &mut self.roles[role];
        bucket.time_s += time;
        bucket.length_mm += length;
        bucket.count += 1;
        self.layer_seconds += time;
        if role as u32 == TRAVEL {
            return;
        }
        let volume = extruded_volume(s);
        bucket.volume_mm3 += volume;
        let t = &mut self.tools[tool];
        t.time_s += time;
        t.length_mm += length;
        t.count += 1;
        t.volume_mm3 += volume;
        self.speed.add(speed);
        self.fan.add(((s.meta >> 8) & 255) as f32);
        self.width.add((s.size & 65535) as f32 / 1000.);
        self.height.add((s.size >> 16) as f32 / 1000.);
        if self.last_tool.is_some_and(|t| t != tool) && self.tool_changes.last() != Some(&layer) {
            self.tool_changes.push(layer);
        }
        self.last_tool = Some(tool);
    }
    /// Adds a record's attributes to their ranges (travel is left out, as for speeds).
    pub fn add_attr(&mut self, s: &Segment, a: &Attr) {
        if s.role() == TRAVEL {
            return;
        }
        self.actual.add(a.actual());
        self.flow.add(attrs::volumetric(s) as f32);
        self.temp.add(a.temp());
        self.accel.add(a.accel());
    }
    /// Closes a layer: returns its estimated time and resets the per-layer accumulator.
    /// The caller records the final per-layer time in `layer_time` (tower paths may extend it).
    pub fn finish_layer(&mut self) -> f32 {
        let t = self.layer_seconds as f32;
        self.layer_seconds = 0.;
        t
    }
    pub fn total_time(&self) -> f64 {
        self.roles.iter().map(|b| b.time_s).sum()
    }
}
/// Extruded volume of a record (path cross-section × length, mm³): a rectangle with
/// semicircular ends of the line's width and height, as the flow model.
pub fn extruded_volume(s: &Segment) -> f64 {
    let length = glam::Vec3::from(s.a).distance(glam::Vec3::from(s.b)) as f64;
    let w = (s.size & 65535) as f64 / 1000.;
    let h = (s.size >> 16) as f64 / 1000.;
    (h * (w - h * (1. - std::f64::consts::FRAC_PI_4))).max(0.) * length
}
/// `6.2 s`, `48 s`, `2m 05s`: a layer time.
pub fn format_seconds(seconds: f64) -> String {
    if seconds < 10. {
        format!("{seconds:.1} s")
    } else if seconds < 60. {
        format!("{seconds:.0} s")
    } else {
        format_time(seconds)
    }
}

/// `1h 02m`, `12m 05s`, `48s`
pub fn format_time(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buckets_percentages_and_ranges() {
        let mut st = Stats::default();
        // 100 mm external perimeter at 50 mm/s (2 s), 100 mm infill at 100 mm/s (1 s), a travel.
        st.add(
            &Segment::new([0.; 3], [100., 0., 0.2], [0.45, 0.2], 2, 0, 50., 100.),
            0,
        );
        st.add(
            &Segment::new([0.; 3], [100., 0., 0.2], [0.45, 0.2], 5, 1, 100., 30.),
            0,
        );
        st.add(
            &Segment::new([0.; 3], [10., 0., 0.2], [0.08, 0.04], TRAVEL, 1, 200., 0.),
            0,
        );
        assert!((st.finish_layer() - 3.05).abs() < 1e-3);
        assert!((st.total_time() - 3.05).abs() < 1e-3);
        assert!((st.roles[2].time_s - 2.).abs() < 1e-4);
        assert!((st.roles[5].time_s - 1.).abs() < 1e-4);
        assert_eq!(
            st.speed,
            Range {
                min: 50.,
                max: 100.
            }
        );
        assert_eq!(
            st.fan,
            Range {
                min: 30.,
                max: 100.
            }
        );
        assert_eq!(st.tool_changes, vec![0]);
        assert!((st.tools[0].time_s - 2.).abs() < 1e-4);
        st.layer_time.add(3.05);
        assert_eq!(st.layer_time.span([0., 1.]), [3.05 - 0.5, 3.05 + 0.5]);
        assert_eq!(Range::default().span([1., 2.]), [1., 2.]);
    }
    #[test]
    fn tool_changes_once_per_layer() {
        let mut st = Stats::default();
        for layer in 0..3 {
            for tool in [0, 1, 0] {
                st.add(
                    &Segment::new([0.; 3], [1., 0., 0.], [0.4, 0.2], 4, tool, 60., 0.),
                    layer,
                );
            }
            st.finish_layer();
        }
        assert_eq!(st.tool_changes, vec![0, 1, 2]);
    }
    #[test]
    fn time_formatting() {
        assert_eq!(format_seconds(6.21), "6.2 s");
        assert_eq!(format_seconds(48.), "48 s");
        assert_eq!(format_seconds(125.), "2m 05s");
        assert_eq!(format_time(48.), "48s");
        assert_eq!(format_time(725.), "12m 05s");
        assert_eq!(format_time(3720.), "1h 02m");
    }
}
