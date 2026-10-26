// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Painted facets as an overlay mesh: the leaves of every painting stream of an object, in
//! mesh-local coordinates so the renderer draws them with the object's transform.
//!
//! Colour painting is the model's own look: it is drawn solid in the tool's filament colour.
//! Every other kind is a **pattern over** that base colour, never a replacement: support
//! enforcers a blue hatch, blockers a red hatch, seam enforcers mint and blockers pink
//! stripes, fuzzy skin a light stipple. The patterns are screen-space (`paint.wgsl`), so they
//! stay crisp at any zoom and the colour underneath stays recognisable. Each vertex carries
//! its pattern and a strength (the colour's alpha): full for the active tool, subtler for a
//! kind shown alongside another tool.

use bytemuck::{Pod, Zeroable};
use mica_core::mesh::TriangleMesh;
use mica_core::painting::{FacetPainting, PaintKind};
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct PaintVertex {
    pub position: [f32; 3],
    /// Pattern colour; alpha is the strength (1 = the active tool, less = shown alongside).
    pub color: [f32; 4],
    /// `Pattern` as the shader reads it.
    pub pattern: u32,
}

/// How a painted facet is drawn over the model (`paint.wgsl` has the matching constants).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Pattern {
    /// Colour painting: the facet's own colour, opaque.
    Solid = 0,
    /// Diagonal hatch (support enforcers blue, blockers red).
    Hatch = 1,
    /// Stripes the other way (seam enforcers mint, blockers pink).
    Stripes = 2,
    /// Fine stipple with a light tint (fuzzy skin).
    Stipple = 3,
}

/// Strength per paint kind (`PaintKind::ALL` order); 0 hides the kind. Colour painting is
/// always shown solid, so its entry only matters for being non-zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Show(pub [f32; 5]);

impl Show {
    /// Everything at full strength.
    pub const ALL: Self = Self([1.; 5]);
    /// Only colour painting.
    pub const COLOUR_ONLY: Self = Self([0., 0., 1., 0., 0.]);
    /// Strength of a kind shown alongside another tool (half, so the base colour reads).
    pub const SUBTLE: f32 = 0.5;

    pub fn strength(&self, kind: PaintKind) -> f32 {
        self.0[Self::index(kind)]
    }
    pub fn set(&mut self, kind: PaintKind, strength: f32) {
        self.0[Self::index(kind)] = strength;
    }
    fn index(kind: PaintKind) -> usize {
        PaintKind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
    }
    /// For the overlay cache key.
    pub fn bits(&self) -> u64 {
        self.0
            .iter()
            .fold(0u64, |h, v| h.rotate_left(16) ^ u64::from(v.to_bits()))
    }
}

/// One object's overlay for the renderer: `key` changes whenever the vertices do.
#[derive(Clone)]
pub struct PaintOverlay {
    pub object: u64,
    pub key: u64,
    pub vertices: Arc<Vec<PaintVertex>>,
}

pub const SUPPORT_ENFORCER: [f32; 4] = [0.25, 0.66, 0.96, 1.];
pub const SUPPORT_BLOCKER: [f32; 4] = [0.9, 0.33, 0.29, 1.];
pub const SEAM_ENFORCER: [f32; 4] = [0.36, 0.86, 0.6, 1.];
pub const SEAM_BLOCKER: [f32; 4] = [1., 0.48, 0.71, 1.];
pub const FUZZY: [f32; 4] = [0.95, 0.82, 0.35, 1.];
/// Texture paint, one tint per slot.
pub const TEXTURE_SLOTS: [[f32; 4]; 4] = [
    [0.36, 0.78, 0.86, 1.],
    [0.62, 0.52, 0.95, 1.],
    [0.95, 0.6, 0.3, 1.],
    [0.55, 0.85, 0.4, 1.],
];
/// Overhang highlight (support painting with "Show overhangs"): magenta, so it stands out on
/// orange, white and grey filaments alike.
pub const OVERHANG: [f32; 4] = [0.9, 0.25, 0.65, 1.];

/// The pattern a paint kind is drawn with.
pub fn pattern_of(kind: PaintKind) -> Pattern {
    match kind {
        PaintKind::Material => Pattern::Solid,
        PaintKind::Supports => Pattern::Hatch,
        PaintKind::Seam => Pattern::Stripes,
        PaintKind::Fuzzy => Pattern::Stipple,
        PaintKind::Texture => Pattern::Stripes,
    }
}

/// The overlay colour of a painted state; colour painting uses the tool's filament colour
/// (state `n` is tool `n`, one-based), the others fixed role colours.
pub fn state_color(kind: PaintKind, state: u8, tool_colors: &[[f32; 3]]) -> Option<[f32; 4]> {
    if state == 0 {
        return None;
    }
    Some(match kind {
        PaintKind::Supports => {
            if state == 1 {
                SUPPORT_ENFORCER
            } else {
                SUPPORT_BLOCKER
            }
        }
        PaintKind::Seam => {
            if state == 1 {
                SEAM_ENFORCER
            } else {
                SEAM_BLOCKER
            }
        }
        PaintKind::Fuzzy => FUZZY,
        PaintKind::Texture => TEXTURE_SLOTS[usize::from(state - 1) % TEXTURE_SLOTS.len()],
        PaintKind::Material => {
            let t = usize::from(state - 1);
            let c = tool_colors
                .get(t)
                .copied()
                .unwrap_or_else(|| crate::preview::legend::tool_color(t));
            [c[0], c[1], c[2], 1.]
        }
    })
}

/// Vertices of every painted leaf of the kinds `show` gives a strength: colour painting
/// solid first (at its filament's alpha from `tool_alphas`, missing = opaque), then the
/// patterned kinds over it (later kinds draw over earlier ones).
pub fn overlay_vertices(
    mesh: &TriangleMesh,
    painting: &FacetPainting,
    show: Show,
    tool_colors: &[[f32; 3]],
    tool_alphas: &[f32],
) -> Vec<PaintVertex> {
    let mut out = Vec::new();
    let order = [
        PaintKind::Material,
        PaintKind::Supports,
        PaintKind::Seam,
        PaintKind::Fuzzy,
        PaintKind::Texture,
    ];
    for kind in order {
        let strength = show.strength(kind);
        if strength <= 0. {
            continue;
        }
        let states = painting.get(kind);
        if states.is_empty() {
            continue;
        }
        let pattern = pattern_of(kind);
        states.visit(mesh, false, |state, t| {
            if let Some(mut color) = state_color(kind, state, tool_colors) {
                color[3] = if pattern == Pattern::Solid {
                    let tool = usize::from(state).saturating_sub(1);
                    tool_alphas.get(tool).copied().unwrap_or(1.)
                } else {
                    strength
                };
                for p in t {
                    out.push(PaintVertex {
                        position: p.map(|v| v as f32),
                        color,
                        pattern: pattern as u32,
                    });
                }
            }
        });
    }
    out
}

/// Vertices of whole mesh facets in one solid colour (the overhang highlight).
pub fn facet_vertices(mesh: &TriangleMesh, facets: &[usize], color: [f32; 4]) -> Vec<PaintVertex> {
    facets
        .iter()
        .flat_map(|&f| mesh.faces[f].iter().map(|&v| mesh.vertices[v as usize]))
        .map(|position| PaintVertex {
            position,
            color,
            pattern: Pattern::Solid as u32,
        })
        .collect()
}

/// A cache key for an overlay built from these inputs.
pub fn overlay_key(object: u64, fingerprint: u64, show: Show, extra: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    object.hash(&mut h);
    fingerprint.hash(&mut h);
    show.bits().hash(&mut h);
    extra.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> TriangleMesh {
        TriangleMesh {
            vertices: vec![[0., 0., 0.], [10., 0., 0.], [10., 10., 0.], [0., 10., 0.]],
            faces: vec![[0, 1, 2], [0, 2, 3]],
        }
    }

    #[test]
    fn overlay_colours_follow_kind_state_and_filament() {
        let tools = [[1., 0., 0.], [0., 1., 0.]];
        assert_eq!(state_color(PaintKind::Supports, 0, &tools), None);
        assert_eq!(
            state_color(PaintKind::Supports, 1, &tools),
            Some(SUPPORT_ENFORCER)
        );
        assert_eq!(
            state_color(PaintKind::Supports, 2, &tools),
            Some(SUPPORT_BLOCKER)
        );
        assert_eq!(state_color(PaintKind::Seam, 2, &tools), Some(SEAM_BLOCKER));
        assert_eq!(state_color(PaintKind::Fuzzy, 1, &tools), Some(FUZZY));
        assert_eq!(
            state_color(PaintKind::Material, 2, &tools),
            Some([0., 1., 0., 1.])
        );
        // Tools beyond the printer's colours fall back to the legend palette.
        assert!(state_color(PaintKind::Material, 9, &tools).is_some());
    }

    #[test]
    fn overlay_vertices_cover_painted_leaves_only() {
        let mesh = square();
        let mut painting = FacetPainting::default();
        // Facet 0 whole as tool 2 (nibble 8 = state 2), facet 1 a support enforcer (4).
        painting.material.push(0, "8").unwrap();
        painting.supports.push(1, "4").unwrap();
        let tools = [[1., 0., 0.], [0., 0., 1.]];
        let all = overlay_vertices(&mesh, &painting, Show::ALL, &tools, &[]);
        assert_eq!(all.len(), 6);
        // Colour painting is solid in the tool's colour; the support paint is a hatch at
        // full strength.
        assert_eq!(
            all.iter()
                .filter(|v| v.color == [0., 0., 1., 1.] && v.pattern == Pattern::Solid as u32)
                .count(),
            3
        );
        assert_eq!(
            all.iter()
                .filter(|v| v.color == SUPPORT_ENFORCER && v.pattern == Pattern::Hatch as u32)
                .count(),
            3
        );
        let material = overlay_vertices(&mesh, &painting, Show::COLOUR_ONLY, &tools, &[]);
        assert_eq!(material.len(), 3);
        // A translucent filament's colour painting carries its alpha; patterns keep theirs.
        let translucent = overlay_vertices(&mesh, &painting, Show::ALL, &tools, &[1., 0.5]);
        assert!(
            translucent
                .iter()
                .filter(|v| v.pattern == Pattern::Solid as u32)
                .all(|v| v.color == [0., 0., 1., 0.5])
        );
        assert!(
            translucent
                .iter()
                .filter(|v| v.pattern == Pattern::Hatch as u32)
                .all(|v| v.color == SUPPORT_ENFORCER)
        );
        // A kind shown alongside another tool is subtler: the strength is the alpha.
        let mut show = Show::COLOUR_ONLY;
        show.set(PaintKind::Supports, Show::SUBTLE);
        let subtle = overlay_vertices(&mesh, &painting, show, &tools, &[]);
        assert_eq!(subtle.len(), 6);
        assert!(
            subtle
                .iter()
                .filter(|v| v.pattern == Pattern::Hatch as u32)
                .all(|v| v.color[3] == Show::SUBTLE)
        );
        // Fuzzy skin over colour painting keeps the colour: the colour stays solid underneath
        // and the fuzzy stipple is a pattern, not a yellow fill.
        painting.fuzzy.push(0, "4").unwrap();
        let both = overlay_vertices(&mesh, &painting, Show::ALL, &tools, &[]);
        assert_eq!(both.len(), 9);
        let fuzzy: Vec<_> = both
            .iter()
            .filter(|v| v.pattern == Pattern::Stipple as u32)
            .collect();
        assert_eq!(fuzzy.len(), 3);
        assert_eq!(fuzzy[0].color[..3], FUZZY[..3]);
        assert_eq!(
            both.iter().filter(|v| v.color == [0., 0., 1., 1.]).count(),
            3
        );
        assert_ne!(overlay_key(1, 2, Show::ALL, 0), overlay_key(1, 2, show, 0));
        assert_eq!(
            overlay_vertices(&mesh, &painting, Show([0.; 5]), &tools, &[]).len(),
            0
        );
    }
}
