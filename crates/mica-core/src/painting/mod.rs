// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSelector.cpp :: serialize, deserialize, perform_split, decode_leaf_state
// Source: prusaslicer@b028299c77 src/libslic3r/Model.cpp :: FacetsAnnotation::set_triangle_from_string
// Changes: Validated sparse nibble streams; decode leaf geometry on demand, without copying meshes per state; preserve source winding across transforms.

//! Compact volume-local facet annotations. States are zero (unpainted), one/two
//! (enforcer/blocker), or one-based material indices for colour painting.
use crate::mesh::TriangleMesh;

mod interlocking;
pub mod projection;
pub mod query;
pub mod segmentation;
pub mod selector;
mod voronoi;

pub type Triangle = [[f64; 3]; 3];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FacetStates {
    records: Vec<(u32, u32)>,
    // One byte per nibble/node, not one allocation per triangle or one mesh per colour.
    codes: Vec<u8>,
    reversed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FacetPainting {
    pub supports: FacetStates,
    pub seam: FacetStates,
    pub material: FacetStates,
    pub fuzzy: FacetStates,
    /// Mica's texture paint: 0 none, 1..15 texture slots (`mica_texture_paint_slots`). Stored
    /// in Mica's sidecar only; PrusaSlicer and OrcaSlicer have no such stream.
    pub texture: FacetStates,
}

/// The five painting streams, as the GUI's tools address them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaintKind {
    Supports,
    Seam,
    Material,
    Fuzzy,
    Texture,
}
impl PaintKind {
    pub const ALL: [Self; 5] = [
        Self::Supports,
        Self::Seam,
        Self::Material,
        Self::Fuzzy,
        Self::Texture,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Supports => "support",
            Self::Seam => "seam",
            Self::Material => "color",
            Self::Fuzzy => "fuzzy skin",
            Self::Texture => "texture",
        }
    }
}

impl FacetPainting {
    pub fn get(&self, kind: PaintKind) -> &FacetStates {
        match kind {
            PaintKind::Supports => &self.supports,
            PaintKind::Seam => &self.seam,
            PaintKind::Material => &self.material,
            PaintKind::Fuzzy => &self.fuzzy,
            PaintKind::Texture => &self.texture,
        }
    }
    pub fn get_mut(&mut self, kind: PaintKind) -> &mut FacetStates {
        match kind {
            PaintKind::Supports => &mut self.supports,
            PaintKind::Seam => &mut self.seam,
            PaintKind::Material => &mut self.material,
            PaintKind::Fuzzy => &mut self.fuzzy,
            PaintKind::Texture => &mut self.texture,
        }
    }
    pub fn is_empty(&self) -> bool {
        PaintKind::ALL.iter().all(|&k| self.get(k).is_empty())
    }
    /// The kinds that carry paint.
    pub fn kinds(&self) -> impl Iterator<Item = PaintKind> + '_ {
        PaintKind::ALL
            .into_iter()
            .filter(move |&k| !self.get(k).is_empty())
    }
    /// A content hash, so caches know when a painting changed.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for k in PaintKind::ALL {
            self.get(k).hash(&mut h);
        }
        h.finish()
    }
    /// Heap bytes of the five streams (for bounding caches and undo history).
    pub fn heap_bytes(&self) -> usize {
        PaintKind::ALL
            .iter()
            .map(|&k| self.get(k).heap_bytes())
            .sum()
    }
    /// Whether every stream uses the same winding flag (empty streams do not count).
    pub fn reversed(&self) -> Option<bool> {
        let mut flags = self.kinds().map(|k| self.get(k).reversed);
        let first = flags.next()?;
        flags.all(|f| f == first).then_some(first)
    }
    /// Appends `other`'s streams with their triangle indices shifted by `offset` (concatenated
    /// meshes). Fails when a non-empty stream's winding flag disagrees with ours.
    pub fn append_shifted(&mut self, other: &Self, offset: u32) -> Result<(), String> {
        for k in PaintKind::ALL {
            self.get_mut(k).append_shifted(other.get(k), offset)?;
        }
        Ok(())
    }

    pub fn reverse_winding(&mut self) {
        for states in [
            &mut self.supports,
            &mut self.seam,
            &mut self.material,
            &mut self.fuzzy,
            &mut self.texture,
        ] {
            states.reversed = !states.reversed;
        }
    }

    pub fn triangle_range(&self, first: usize, last: usize) -> Self {
        Self {
            supports: self.supports.triangle_range(first, last),
            seam: self.seam.triangle_range(first, last),
            material: self.material.triangle_range(first, last),
            fuzzy: self.fuzzy.triangle_range(first, last),
            texture: self.texture.triangle_range(first, last),
        }
    }
}

impl std::hash::Hash for FacetStates {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.records.hash(state);
        self.codes.hash(state);
        self.reversed.hash(state);
    }
}

impl FacetStates {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    /// Heap bytes of the stream (records and codes).
    pub fn heap_bytes(&self) -> usize {
        self.records.capacity() * std::mem::size_of::<(u32, u32)>() + self.codes.capacity()
    }
    /// Whether the streams were written for the swapped winding (b, c) of the mesh faces.
    pub fn reversed(&self) -> bool {
        self.reversed
    }
    pub fn with_reversed(mut self, reversed: bool) -> Self {
        self.reversed = reversed;
        self
    }
    /// `(triangle, nibble codes)` in ascending triangle order.
    pub fn entries(&self) -> impl Iterator<Item = (u32, &[u8])> + '_ {
        (0..self.records.len()).map(|i| (self.records[i].0, self.codes_at(i)))
    }
    /// The 3MF attribute text of one triangle's codes (hex digits, reversed as upstream
    /// writes them).
    pub fn wire_string(codes: &[u8]) -> String {
        codes
            .iter()
            .rev()
            .map(|&c| {
                char::from_digit(u32::from(c), 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase()
            })
            .collect()
    }
    /// Appends `other` with its triangle indices shifted by `offset`; the winding flags must
    /// agree (an empty side takes the other's flag).
    pub fn append_shifted(&mut self, other: &Self, offset: u32) -> Result<(), String> {
        if other.is_empty() {
            return Ok(());
        }
        if self.is_empty() {
            self.reversed = other.reversed;
        } else if self.reversed != other.reversed {
            return Err("facet streams with different windings cannot be joined".into());
        }
        for (triangle, codes) in other.entries() {
            self.push_codes(
                triangle
                    .checked_add(offset)
                    .ok_or("facet triangle index overflow")?,
                codes,
            )?;
        }
        Ok(())
    }

    /// Append validated nibble codes for one triangle (the decoded form of [`Self::push`]).
    pub fn push_codes(&mut self, triangle: u32, codes: &[u8]) -> Result<(), String> {
        if codes.is_empty() {
            return Ok(());
        }
        if self.records.last().is_some_and(|r| r.0 >= triangle) {
            return Err("facet annotations must have increasing triangle indices".into());
        }
        let mut pos = 0;
        validate_node(codes, &mut pos, 0)?;
        if pos != codes.len() {
            return Err("trailing facet painting nodes".into());
        }
        let offset = u32::try_from(self.codes.len()).map_err(|_| "facet painting too large")?;
        if self
            .codes
            .len()
            .checked_add(codes.len())
            .is_none_or(|n| n > u32::MAX as usize)
        {
            return Err("facet painting too large".into());
        }
        if codes != [0] {
            self.records.push((triangle, offset));
            self.codes.extend_from_slice(codes);
        }
        Ok(())
    }
    pub fn painted_triangles(&self) -> usize {
        self.records.len()
    }
    pub fn storage_bytes(&self) -> usize {
        self.codes.len() + self.records.len() * 8
    }

    /// Append a 3MF TriangleSelector string in ascending original triangle order.
    /// Hex digits and child order are both reversed by the upstream wire format.
    pub fn push(&mut self, triangle: u32, text: &str) -> Result<(), String> {
        let _timing = crate::timing::Span::new(crate::timing::Stage::DecodePainting);
        if text.is_empty() {
            return Ok(());
        }
        if self.records.last().is_some_and(|r| r.0 >= triangle) {
            return Err("facet annotations must have increasing triangle indices".into());
        }
        let codes: Vec<u8> = text
            .bytes()
            .rev()
            .map(|b| match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'A'..=b'F' => Ok(b - b'A' + 10),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                _ => Err("invalid facet painting hex digit".to_string()),
            })
            .collect::<Result<_, _>>()?;
        self.push_codes(triangle, &codes)
    }

    fn codes_at(&self, index: usize) -> &[u8] {
        let end = self
            .records
            .get(index + 1)
            .map_or(self.codes.len(), |r| r.1 as usize);
        &self.codes[self.records[index].1 as usize..end]
    }
    /// Every non-zero state painted on any facet (for material painting: one-based tools),
    /// without walking the mesh.
    pub fn states(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..self.records.len() {
            let codes = self.codes_at(i);
            let mut pos = 0;
            states_of(codes, &mut pos, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The same painting with every leaf state passed through `f` (renumbering tools).
    pub fn map_states(&self, f: impl Fn(u8) -> u8) -> Self {
        fn node(codes: &[u8], pos: &mut usize, f: &impl Fn(u8) -> u8, out: &mut Vec<u8>) {
            let code = codes[*pos];
            *pos += 1;
            let count = code & 3;
            if count == 0 {
                let s = state(code, codes, pos).expect("validated facet state");
                selector::encode_state(f(s), out);
                return;
            }
            out.push(code);
            for _ in 0..=count {
                node(codes, pos, f, out);
            }
        }
        let mut out = Self {
            reversed: self.reversed,
            ..Self::default()
        };
        for (triangle, codes) in self.entries() {
            let mut mapped = Vec::with_capacity(codes.len());
            node(codes, &mut 0, &f, &mut mapped);
            out.push_codes(triangle, &mapped)
                .expect("re-encoded facet states stay valid");
        }
        out
    }

    pub fn triangle_range(&self, first: usize, last: usize) -> Self {
        let mut out = Self {
            reversed: self.reversed,
            ..Self::default()
        };
        let start = self.records.partition_point(|r| (r.0 as usize) < first);
        for i in start..self.records.len() {
            let triangle = self.records[i].0 as usize;
            if triangle > last {
                break;
            }
            out.records
                .push(((triangle - first) as u32, out.codes.len() as u32));
            out.codes.extend_from_slice(self.codes_at(i));
        }
        out
    }

    /// Visit leaves, optionally including unpainted source facets and zero-state split leaves.
    /// Output winding follows the mesh, even when its source winding was repaired or mirrored.
    pub fn visit(
        &self,
        mesh: &TriangleMesh,
        include_default: bool,
        mut f: impl FnMut(u8, Triangle),
    ) {
        if !include_default && self.records.is_empty() {
            return;
        }
        let mut record = 0;
        for (index, face) in mesh.faces.iter().enumerate() {
            let mut tri = face.map(|v| mesh.vertices[v as usize].map(f64::from));
            if self.reversed {
                tri.swap(1, 2);
            }
            if self
                .records
                .get(record)
                .is_some_and(|r| r.0 as usize == index)
            {
                let mut pos = 0;
                visit_node(
                    self.codes_at(record),
                    &mut pos,
                    tri,
                    &mut |state, mut t: Triangle| {
                        if state != 0 || include_default {
                            if self.reversed {
                                t.swap(1, 2);
                            }
                            f(state, t);
                        }
                    },
                );
                record += 1;
            } else if include_default {
                if self.reversed {
                    tri.swap(1, 2);
                }
                f(0, tri);
            }
        }
    }

    pub fn validate_mesh(&self, mesh: &TriangleMesh) -> Result<(), String> {
        if self
            .records
            .last()
            .is_some_and(|r| r.0 as usize >= mesh.faces.len())
        {
            Err("facet annotation triangle outside volume mesh".into())
        } else {
            Ok(())
        }
    }
}

fn next(codes: &[u8], pos: &mut usize) -> Result<u8, String> {
    let code = *codes.get(*pos).ok_or("truncated facet painting tree")?;
    *pos += 1;
    Ok(code)
}
pub(crate) fn state(code: u8, codes: &[u8], pos: &mut usize) -> Result<u8, String> {
    if code >> 2 != 3 {
        return Ok(code >> 2);
    }
    let ext = next(codes, pos)?;
    if ext != 14 {
        return Ok(ext + 3);
    }
    let lo = u16::from(next(codes, pos)?);
    let hi = u16::from(next(codes, pos)?);
    u8::try_from(17 + lo + (hi << 4)).map_err(|_| "facet state exceeds 255".into())
}
fn validate_node(codes: &[u8], pos: &mut usize, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("facet painting split depth exceeds 64".into());
    }
    let code = next(codes, pos)?;
    let count = code & 3;
    if count == 0 {
        state(code, codes, pos)?;
    } else {
        if code >> 2 > 2 || (count == 3 && code >> 2 != 0) {
            return Err("invalid facet painting split side".into());
        }
        for _ in 0..=count {
            validate_node(codes, pos, depth + 1)?;
        }
    }
    Ok(())
}
/// The leaf states of one facet's tree (`visit_node` without the geometry).
fn states_of(codes: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    let Some(&code) = codes.get(*pos) else {
        return;
    };
    *pos += 1;
    let count = code & 3;
    if count == 0 {
        if let Ok(s) = state(code, codes, pos)
            && s != 0
        {
            out.push(s);
        }
        return;
    }
    for _ in 0..=count {
        states_of(codes, pos, out);
    }
}
fn visit_node(codes: &[u8], pos: &mut usize, triangle: Triangle, f: &mut impl FnMut(u8, Triangle)) {
    let code = codes[*pos];
    *pos += 1;
    let count = code & 3;
    if count == 0 {
        f(
            state(code, codes, pos).expect("validated facet state"),
            triangle,
        );
        return;
    }
    let side = (code >> 2) as usize;
    let [a, b, c] = std::array::from_fn(|i| triangle[(i + side) % 3]);
    let mid = |a: [f64; 3], b: [f64; 3]| std::array::from_fn(|i| (a[i] + b[i]) * 0.5);
    let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
    let children = match count {
        1 => vec![[a, b, bc], [bc, c, a]],
        2 => vec![[a, ab, ca], [ab, b, ca], [b, c, ca]],
        _ => vec![[a, ab, ca], [ab, b, bc], [bc, c, ca], [ab, bc, ca]],
    };
    for child in children.into_iter().rev() {
        visit_node(codes, pos, child, f);
    }
}
