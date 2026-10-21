// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/SeamPainting.cpp :: Painting, is_enforced, is_blocked
// Source: prusaslicer@b028299c77 src/libslic3r/MultiMaterialSegmentation.cpp :: fuzzy_skin_segmentation_by_painting
// Changes: One state-tagged facet BVH for all painted states, no full mesh per state; shared seam/fuzzy proximity queries in bed coordinates.

use super::Triangle;
use crate::fuzzy_skin::{FuzzySkinPainting, FuzzySkinSettings, Mode};
use crate::geometry::Point;
use crate::regions::RegionModel;
use crate::seam::painting::{Paint, SeamPainting};

#[derive(Default)]
pub struct PaintedQuery {
    facets: Vec<(u8, [[f32; 3]; 3])>,
    nodes: Vec<Node>,
}
struct Node {
    min: [f64; 3],
    max: [f64; 3],
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}
impl PaintedQuery {
    pub fn new(model: &RegionModel, fuzzy: bool) -> Self {
        let mut out = Self::default();
        for volume in &model.volumes {
            if volume.kind != crate::threemf::PartKind::Model {
                continue;
            }
            let states = if fuzzy {
                &volume.painting.fuzzy
            } else {
                &volume.painting.seam
            };
            states.visit(&volume.mesh, false, |state, t| {
                out.facets.push((state, t.map(|v| v.map(|x| x as f32))));
            });
        }
        if !out.facets.is_empty() {
            out.build(0, out.facets.len());
        }
        out
    }
    pub fn is_empty(&self) -> bool {
        self.facets.is_empty()
    }
    fn build(&mut self, start: usize, end: usize) -> usize {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for (_, t) in &self.facets[start..end] {
            for p in t {
                for k in 0..3 {
                    min[k] = min[k].min(f64::from(p[k]));
                    max[k] = max[k].max(f64::from(p[k]));
                }
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            start,
            end,
            children: None,
        });
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            let mid = (start + end) / 2;
            self.facets[start..end].select_nth_unstable_by(mid - start, |a, b| {
                let center = |t: &(u8, [[f32; 3]; 3])| t.1.iter().map(|v| v[axis]).sum::<f32>();
                center(a).total_cmp(&center(b))
            });
            let left = self.build(start, mid);
            let right = self.build(mid, end);
            self.nodes[index].children = Some((left, right));
        }
        index
    }
    pub fn state_at(&self, p: [f64; 3], radius: f64) -> u8 {
        if self.nodes.is_empty() {
            return 0;
        }
        let mut result = 0;
        let mut stack = vec![0];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i];
            let d = (0..3)
                .map(|k| (p[k] - p[k].clamp(n.min[k], n.max[k])).powi(2))
                .sum::<f64>();
            if d > radius * radius {
                continue;
            }
            if let Some((a, b)) = n.children {
                stack.extend([a, b]);
            } else {
                for (state, t) in &self.facets[n.start..n.end] {
                    if distance_sq(p, t.map(|v| v.map(f64::from))) <= radius * radius {
                        result = result.max(*state);
                    }
                }
            }
        }
        result
    }
}
impl SeamPainting for PaintedQuery {
    fn classify(&self, p: [f64; 3], radius: f64) -> Paint {
        match self.state_at(p, radius) {
            1 => Paint::Enforcer,
            2 => Paint::Blocker,
            _ => Paint::Common,
        }
    }
}

#[derive(Default)]
pub struct FuzzyPainting {
    pub query: PaintedQuery,
    pub zs: Vec<f64>,
    /// The object's texture skin (Mica's wall texture), shared by every region.
    pub texture: Option<std::sync::Arc<crate::texture_skin::TextureSkin>>,
}
pub struct FuzzyLayer<'a> {
    pub painting: &'a FuzzyPainting,
    pub settings: &'a FuzzySkinSettings,
    pub radius: f64,
    pub origin: Point,
}
impl FuzzySkinPainting for FuzzyLayer<'_> {
    fn needs_sampling(&self) -> bool {
        !self.painting.query.is_empty()
    }
    fn texture_skin(&self) -> Option<crate::texture_skin::Surface<'_>> {
        self.painting
            .texture
            .as_deref()
            .map(|skin| crate::texture_skin::Surface {
                skin,
                offset: self.origin,
            })
    }
    fn settings_at(&self, point: Point, layer: usize) -> Option<FuzzySkinSettings> {
        let z = *self.painting.zs.get(layer)?;
        let x = (point.x + self.origin.x) as f64 / crate::geometry::SCALE;
        let y = (point.y + self.origin.y) as f64 / crate::geometry::SCALE;
        match self.painting.query.state_at([x, y, z], self.radius) {
            0 => None,
            state => Some(FuzzySkinSettings {
                mode: if state == 1 { Mode::All } else { Mode::None },
                ..self.settings.clone()
            }),
        }
    }
}

fn distance_sq(p: [f64; 3], [a, b, c]: Triangle) -> f64 {
    let sub = |a: [f64; 3], b: [f64; 3]| std::array::from_fn::<_, 3, _>(|i| a[i] - b[i]);
    let dot = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f64>();
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let n = cross(sub(b, a), sub(c, a));
    let n2 = dot(n, n);
    if n2 > 1e-20 {
        let h = dot(sub(p, a), n) / n2;
        let q = std::array::from_fn(|i| p[i] - h * n[i]);
        if [(a, b), (b, c), (c, a)]
            .iter()
            .all(|&(u, v)| dot(cross(sub(v, u), sub(q, u)), n) >= -1e-12)
        {
            return dot(sub(p, q), sub(p, q));
        }
    }
    [(a, b), (b, c), (c, a)]
        .iter()
        .map(|&(u, v)| {
            let e = sub(v, u);
            let l = dot(e, e);
            let t = if l > 0. {
                (dot(sub(p, u), e) / l).clamp(0., 1.)
            } else {
                0.
            };
            let d = std::array::from_fn(|i| p[i] - u[i] - t * e[i]);
            dot(d, d)
        })
        .fold(f64::INFINITY, f64::min)
}
