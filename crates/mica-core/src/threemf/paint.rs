// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/TriangleSelector.cpp :: perform_split, serialize, deserialize
// Changes: Reorient selector trees to outward-facing wire triangles; express a two-edge split as two one-edge splits so reflection preserves its diagonal. Stream exact originals only when reorientation is necessary, and always for Mica's texture paint.

use super::{Project, ThreeMfError, invalid};
use crate::painting::{FacetStates, PaintKind, selector::encode_state};
use std::io::{Read, Write};

pub(super) const BACKUP: &str = "Metadata/mica_painting.bin";
const MAGIC: &[u8] = b"MICA_PAINT_1\0";

enum Node {
    Leaf(u8),
    Split { side: usize, children: Vec<Node> },
}
impl Node {
    fn decode(codes: &[u8], pos: &mut usize) -> Self {
        let code = codes[*pos];
        *pos += 1;
        let count = code & 3;
        if count == 0 {
            return Self::Leaf(
                crate::painting::state(code, codes, pos).expect("validated selector"),
            );
        }
        let mut children = (0..=count)
            .map(|_| Self::decode(codes, pos))
            .collect::<Vec<_>>();
        children.reverse();
        Self::Split {
            side: (code >> 2) as usize,
            children,
        }
    }
    fn orient(self, order: [usize; 3]) -> Self {
        let Self::Split { side, children } = self else {
            return self;
        };
        let count = children.len() - 1;
        if count == 2 {
            // [a,ab,ca], [ab,b,ca], [b,c,ca] = split at ca, then at ab.
            // Unlike a two-edge split, one-edge splits are closed under reflection.
            let mut c = children.into_iter();
            let a = c.next().unwrap();
            let b = c.next().unwrap();
            let last = c.next().unwrap();
            return Self::Split {
                side: (side + 1) % 3,
                children: vec![
                    last,
                    Self::Split {
                        side: 0,
                        children: vec![a.orient([2, 0, 1]), b],
                    },
                ],
            }
            .orient(order);
        }
        let target_side = if count == 3 {
            0
        } else {
            order.iter().position(|&i| i == side).unwrap()
        };
        let source = triangles([0, 1, 2], count, side);
        let target = triangles(order, count, target_side);
        let mut children = children.into_iter().map(Some).collect::<Vec<_>>();
        let children = target
            .into_iter()
            .map(|t| {
                let index = source
                    .iter()
                    .position(|s| t.iter().all(|v| s.contains(v)))
                    .expect("same split tiles");
                let permutation = t.map(|v| source[index].iter().position(|&x| x == v).unwrap());
                children[index].take().unwrap().orient(permutation)
            })
            .collect();
        Self::Split {
            side: target_side,
            children,
        }
    }
    fn encode(&self, out: &mut Vec<u8>, depth: usize) -> Result<(), ThreeMfError> {
        if depth > 64 {
            return Err(invalid("reoriented selector exceeds 64 levels"));
        }
        match self {
            Self::Leaf(state) => encode_state(*state, out),
            Self::Split { side, children } => {
                out.push((children.len() - 1) as u8 | (*side as u8) << 2);
                for child in children.iter().rev() {
                    child.encode(out, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}
fn triangles(v: [usize; 3], count: usize, side: usize) -> Vec<[usize; 3]> {
    let [a, b, c] = std::array::from_fn(|i| v[(i + side) % 3]);
    let mid = |a, b| {
        3 + [[0, 1], [1, 2], [0, 2]]
            .iter()
            .position(|edge| edge.contains(&a) && edge.contains(&b))
            .unwrap()
    };
    let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
    match count {
        1 => vec![[a, b, bc], [bc, c, a]],
        3 => vec![[a, ab, ca], [ab, b, bc], [bc, c, ca], [ab, bc, ca]],
        _ => unreachable!("two-edge splits are expanded first"),
    }
}
pub(super) fn mirrored(codes: &[u8]) -> Result<Vec<u8>, ThreeMfError> {
    if codes.len() > 16 * 1024 * 1024 {
        return Err(invalid("Mica facet stream exceeds 16 MiB"));
    }
    if codes[0] & 3 == 0 {
        return Ok(codes.to_vec());
    }
    let node = Node::decode(codes, &mut 0).orient([0, 2, 1]);
    let mut out = Vec::with_capacity(codes.len());
    node.encode(&mut out, 0)?;
    Ok(out)
}
pub(super) fn needs_backup(p: &Project, flavour: super::Flavour) -> bool {
    p.objects.iter().flat_map(|o| &o.parts).any(|p| {
        let flip = super::write::wire_flip(p, flavour);
        // Texture paint exists only here: PrusaSlicer and OrcaSlicer have no attribute for it.
        p.painting
            .kinds()
            .any(|k| k == PaintKind::Texture || p.painting.get(k).reversed() != flip)
    })
}
fn put(w: &mut impl Write, n: u32) -> std::io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn get(r: &mut impl Read) -> Result<u32, ThreeMfError> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
pub(super) fn backup(
    w: &mut impl Write,
    p: &Project,
    flavour: super::Flavour,
) -> Result<(), ThreeMfError> {
    w.write_all(MAGIC)?;
    for (oi, o) in p.objects.iter().enumerate() {
        for (pi, p) in o.parts.iter().enumerate() {
            let flip = super::write::wire_flip(p, flavour);
            for (ki, k) in PaintKind::ALL.into_iter().enumerate() {
                let states = p.painting.get(k);
                if states.is_empty() || (k != PaintKind::Texture && states.reversed() == flip) {
                    continue;
                }
                put(w, oi as u32)?;
                put(w, pi as u32)?;
                put(w, ki as u32)?;
                put(w, states.painted_triangles() as u32)?;
                for (triangle, codes) in states.entries() {
                    put(w, triangle)?;
                    put(w, codes.len() as u32)?;
                    w.write_all(codes)?;
                }
            }
        }
    }
    put(w, u32::MAX)?;
    Ok(())
}
pub(super) fn restore(mut r: impl Read, p: &mut Project) -> Result<(), ThreeMfError> {
    let mut magic = vec![0; MAGIC.len()];
    r.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(invalid("unknown Mica painting format"));
    }
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let oi = get(&mut r)?;
        if oi == u32::MAX {
            break;
        }
        let pi = get(&mut r)?;
        let ki = get(&mut r)?;
        let count = get(&mut r)?;
        if !seen.insert((oi, pi, ki)) {
            return Err(invalid("duplicate Mica painting stream"));
        }
        let part = p
            .objects
            .get_mut(oi as usize)
            .and_then(|o| o.parts.get_mut(pi as usize))
            .ok_or_else(|| invalid("Mica painting part out of bounds"))?;
        let kind = *PaintKind::ALL
            .get(ki as usize)
            .ok_or_else(|| invalid("Mica painting kind out of bounds"))?;
        if count as usize > part.mesh.faces.len() {
            return Err(invalid("Mica painting record count"));
        }
        let states = part.painting.get_mut(kind);
        *states = FacetStates::default().with_reversed(states.reversed());
        for _ in 0..count {
            let triangle = get(&mut r)?;
            let n = get(&mut r)?;
            // Bound the scratch allocation for a single facet, not the whole painting.
            if n > 16 * 1024 * 1024 {
                return Err(invalid("Mica facet stream exceeds 16 MiB"));
            }
            let mut codes = vec![0; n as usize];
            r.read_exact(&mut codes)?;
            states.push_codes(triangle, &codes).map_err(invalid)?;
        }
        states.validate_mesh(&part.mesh).map_err(invalid)?;
    }
    let mut trailing = [0];
    if r.read(&mut trailing)? != 0 {
        return Err(invalid("trailing Mica painting data"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reflected_split_trees_keep_leaf_geometry_and_states() {
        let mesh = crate::mesh::TriangleMesh {
            vertices: vec![[0., 0., 0.], [16., 0., 0.], [0., 16., 0.]],
            faces: vec![[0, 1, 2]],
        };
        fn tree(depth: usize, seed: usize, out: &mut Vec<u8>) {
            if depth == 0 {
                encode_state((seed % 255) as u8, out);
                return;
            }
            let count = 1 + seed % 3;
            let side = if count == 3 { 0 } else { seed / 3 % 3 };
            out.push((count | side << 2) as u8);
            for i in 0..=count {
                tree(depth - 1, seed * 5 + i, out);
            }
        }
        for seed in 0..27 {
            let mut codes = vec![];
            tree(3, seed, &mut codes);
            let mut original = FacetStates::default().with_reversed(true);
            original.push_codes(0, &codes).unwrap();
            let mut output = FacetStates::default();
            output.push_codes(0, &mirrored(&codes).unwrap()).unwrap();
            let leaves = |s: &FacetStates| {
                let mut out = vec![];
                s.visit(&mesh, false, |state, t| {
                    let mut t = t.map(|p| p.map(|v| (v * 1024.) as i64));
                    t.sort();
                    out.push((state, t));
                });
                out.sort();
                out
            };
            assert_eq!(leaves(&original), leaves(&output), "seed {seed}");
        }
    }
}
