// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/BeadingStrategy/DistributedBeadingStrategy.cpp :: DistributedBeadingStrategy
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/BeadingStrategy/RedistributeBeadingStrategy.cpp :: RedistributeBeadingStrategy
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/BeadingStrategy/WideningBeadingStrategy.cpp :: WideningBeadingStrategy
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/BeadingStrategy/LimitedBeadingStrategy.cpp :: LimitedBeadingStrategy
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/BeadingStrategy/BeadingStrategy.cpp :: BeadingStrategy
// Source: prusaslicer@b028299c77 src/libslic3r/Arachne/BeadingStrategy/OuterWallInsetBeadingStrategy.cpp :: OuterWallInsetBeadingStrategy
// Changes: Compose the fixed strategy stack in one value; distances are scaled f64, truncated at bead boundaries.
// Copyright (c) 2022 Ultimaker B.V.
// CuraEngine is released under the terms of the AGPLv3 or higher.

#[derive(Clone, Debug, Default)]
pub(crate) struct Beading {
    pub total: f64,
    pub widths: Vec<f64>,
    pub locations: Vec<f64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Strategy {
    pub outer: f64,
    pub inner: f64,
    pub min_feature: f64,
    pub min_width: f64,
    pub split: f64,
    pub add: f64,
    pub max_beads: usize,
    pub distribution: usize,
    pub transition_length: f64,
    pub angle: f64,
    pub outer_inset: f64,
}

impl Strategy {
    fn optimal_width(&self) -> f64 {
        if self.max_beads <= 2 {
            self.outer
        } else {
            self.inner
        }
    }
    pub fn optimal_thickness(&self, count: usize) -> f64 {
        count.saturating_sub(2) as f64 * self.optimal_width() + count.min(2) as f64 * self.outer
    }
    pub fn transition_thickness(&self, count: usize) -> f64 {
        if count >= self.max_beads {
            return self.optimal_thickness(self.max_beads + 1) - 10_000.;
        }
        match count {
            0 => self.min_feature,
            1 => ((1. + self.split) * self.outer).trunc(),
            _ => (2. * self.outer
                + (count as f64 - 2. + if count % 2 == 1 { self.split } else { self.add })
                    * self.optimal_width())
            .trunc(),
        }
    }
    pub fn count(&self, thickness: f64) -> usize {
        if thickness < self.min_feature {
            return 0;
        }
        let count = if thickness < self.outer * 0.5 {
            0
        } else if thickness <= self.outer * 2. {
            if thickness > (1. + self.split) * self.outer {
                2
            } else {
                1
            }
        } else {
            let t = thickness - 2. * self.outer;
            let n = (t / self.optimal_width()).floor() as usize;
            n + usize::from(
                t - n as f64 * self.optimal_width()
                    >= self.optimal_width() * if n % 2 == 1 { self.split } else { self.add },
            ) + 2
        }
        .max(1);
        if count <= self.max_beads {
            count
        } else if count == self.max_beads + 1
            && thickness < self.transition_thickness(self.max_beads)
        {
            self.max_beads
        } else {
            self.max_beads + 1
        }
    }
    pub fn length(&self, lower: usize) -> f64 {
        if lower == 0 {
            10_000.
        } else {
            self.transition_length
        }
    }
    // Redistribute and Widening delegate this to Distributed, whose optimums are uniform.
    pub fn anchor(&self, lower: usize) -> f64 {
        1. - if lower % 2 == 1 { self.split } else { self.add }
    }
    fn distributed(&self, thickness: f64, mut count: usize) -> Beading {
        let mut ret = Beading {
            total: thickness,
            ..Beading::default()
        };
        if count == 0 {
            return ret;
        }
        let optimal = self.optimal_width();
        let rest = thickness - count as f64 * optimal;
        let radius = self.distribution.saturating_sub(1).max(1) as f64;
        let middle = (count - 1) as f64 / 2.;
        let mut weights: Vec<_> = (0..count)
            .map(|i| (1. - ((i as f64 - middle) / radius).powi(2)).max(0.))
            .collect();
        let total: f64 = weights.iter().sum();
        for w in &mut weights {
            *w /= total;
        }
        let allowed = -optimal / rest / total;
        let adjustment = weights
            .iter()
            .filter(|w| **w > allowed)
            .map(|w| w - allowed)
            .fold(0., f64::max);
        let adjusted = weights.iter().filter(|w| **w > allowed).count();
        if adjusted > 0 && adjusted < count {
            weights = weights
                .into_iter()
                .filter(|w| *w <= allowed)
                .map(|w| (w + adjustment / (count - adjusted) as f64).max(0.))
                .collect();
            count = weights.len();
        }
        match count {
            0 => (),
            1 => {
                ret.widths.push(thickness);
                ret.locations.push((thickness / 2.).trunc());
            }
            2 => {
                let w = (thickness / 2.).trunc();
                ret.widths = vec![w, w];
                ret.locations = vec![(w / 2.).trunc(), thickness - (w / 2.).trunc()];
            }
            _ => {
                let mut accumulated = 0.;
                for (i, weight) in weights.iter().enumerate() {
                    let width = if i + 1 == count {
                        thickness - accumulated
                    } else {
                        (optimal + (rest * weight).trunc()).max(0.)
                    };
                    let location = if i == 0 {
                        (width / 2.).trunc()
                    } else {
                        ret.locations[i - 1] + ((ret.widths[i - 1] + width) / 2.).trunc()
                    };
                    ret.widths.push(width);
                    ret.locations.push(location);
                    accumulated += width;
                }
            }
        }
        ret
    }
    fn parent(&self, thickness: f64, count: usize) -> Beading {
        if thickness < self.optimal_width() {
            return if thickness >= self.min_feature {
                Beading {
                    total: thickness,
                    widths: vec![thickness.max(self.min_width)],
                    locations: vec![(thickness / 2.).trunc()],
                }
            } else {
                Beading {
                    total: thickness,
                    ..Beading::default()
                }
            };
        }
        let mut ret = Beading {
            total: thickness,
            ..Beading::default()
        };
        if count == 0 || thickness < self.outer * 0.5 {
            return ret;
        }
        if count > 2 && thickness > 2. * self.outer {
            ret = self.distributed(thickness - 2. * self.outer, count - 2);
            for x in &mut ret.locations {
                *x += self.outer;
            }
        }
        let width = if count > 2 {
            (thickness / 2.).trunc().min(self.outer)
        } else {
            (thickness / count as f64).trunc()
        };
        ret.widths.insert(0, width);
        ret.locations.insert(0, (width / 2.).trunc());
        if count > 1 {
            ret.widths.push(width);
            ret.locations.push(thickness - (width / 2.).trunc());
        }
        if count >= 2 {
            ret.locations[0] += self.outer_inset;
        }
        ret.total = thickness;
        ret
    }
    pub fn compute(&self, thickness: f64, count: usize) -> Beading {
        if self.max_beads == 0 {
            return Beading {
                total: thickness,
                ..Beading::default()
            };
        }
        let limited = count > self.max_beads;
        let mut ret = self.parent(
            if limited {
                self.optimal_thickness(self.max_beads)
            } else {
                thickness
            },
            count.min(self.max_beads),
        );
        let n = ret.widths.len();
        if limited {
            ret.total = thickness;
            if n % 2 == 1 {
                ret.locations[n / 2] = thickness / 2.;
                ret.widths[n / 2] = thickness - self.optimal_thickness(self.max_beads);
            }
            for i in 0..n.div_ceil(2) {
                ret.locations[n - 1 - i] = thickness - ret.locations[i];
            }
        }
        if n == self.max_beads && n.is_multiple_of(2) {
            let i = n / 2;
            let location = ret.locations[i - 1] + (ret.widths[i - 1] / 2.).trunc();
            ret.locations.insert(i, location);
            ret.widths.insert(i, 0.);
            if limited {
                let j = n - (i - 1);
                let location = ret.locations[j] - (ret.widths[j] / 2.).trunc();
                ret.locations.insert(j, location);
                ret.widths.insert(j, 0.);
            }
        }
        ret
    }
}

pub(crate) fn interpolate(left: &Beading, ratio: f64, right: &Beading) -> Beading {
    let ratio = ratio.clamp(0., 1.);
    let mut ret = if left.total > right.total {
        left.clone()
    } else {
        right.clone()
    };
    for i in 0..left
        .widths
        .len()
        .min(right.widths.len())
        .min(ret.widths.len())
    {
        ret.widths[i] = if left.widths[i] == 0. || right.widths[i] == 0. {
            0.
        } else {
            (ratio * left.widths[i] + (1. - ratio) * right.widths[i]).trunc()
        };
        ret.locations[i] = (ratio * left.locations[i] + (1. - ratio) * right.locations[i]).trunc();
    }
    ret
}
