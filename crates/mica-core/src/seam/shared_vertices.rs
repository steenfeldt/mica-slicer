// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: ported
// Source: prusaslicer@b028299c77 bundled_deps/admesh/admesh/connect.cpp :: stl_check_facets_exact, HashEdge::load_exact, HashTableEdges::insert_edge, record_neighbors
// Source: prusaslicer@b028299c77 bundled_deps/admesh/admesh/shared.cpp :: stl_generate_shared_vertices
// Changes: Rust port over a mesh's face corner positions; the hash chains become per-edge FIFO queues, which pair equal edges in the same order.
// Changes: Only the exact-match connectivity of an import; the nearby/unconnected/normal repairs are not reproduced.

use crate::mesh::TriangleMesh;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

#[derive(Clone, Copy)]
struct Neighbors {
    neighbor: [i32; 3],
    which_vertex_not: [i32; 3],
}

#[derive(Clone, Copy)]
struct HashEdge {
    facet: i32,
    /// Edge index within the facet, plus 3 when stored backwards.
    which_edge: i32,
}

fn vertex_lower(a: [f32; 3], b: [f32; 3]) -> bool {
    if a[0] != b[0] {
        a[0] < b[0]
    } else if a[1] != b[1] {
        a[1] < b[1]
    } else {
        a[2] < b[2]
    }
}
/// Bit pattern of a coordinate with negative zero folded onto positive zero.
fn key_bits(x: f32) -> u32 {
    let b = x.to_bits();
    if b == 0x8000_0000 { 0 } else { b }
}

/// Re-indexes `mesh` the way PrusaSlicer's STL import does: degenerate facets are removed by
/// moving the last facet into their place, edges are connected by exact coordinates, and a
/// shared vertex is created per triangle fan (so non-manifold vertices are split).
pub(super) fn admesh_shared_vertices(mesh: &TriangleMesh) -> TriangleMesh {
    let mut facets: Vec<[[f32; 3]; 3]> = mesh
        .faces
        .iter()
        .map(|f| f.map(|v| mesh.vertices[v as usize]))
        .collect();
    let mut i = 0;
    while i < facets.len() {
        let f = facets[i];
        if f[0] == f[1] || f[1] == f[2] || f[0] == f[2] {
            let last = facets.pop().unwrap();
            if i < facets.len() {
                facets[i] = last;
            }
        } else {
            i += 1;
        }
    }
    let n = facets.len();
    let mut neighbors = vec![
        Neighbors {
            neighbor: [-1; 3],
            which_vertex_not: [-1; 3],
        };
        n
    ];
    let mut open: HashMap<[u32; 6], Vec<HashEdge>> = HashMap::with_capacity(n * 3 / 2);
    for (facet, f) in facets.iter().enumerate() {
        for j in 0..3 {
            let (mut a, mut b) = (f[j], f[(j + 1) % 3]);
            let mut edge = HashEdge {
                facet: facet as i32,
                which_edge: j as i32,
            };
            if !vertex_lower(a, b) {
                std::mem::swap(&mut a, &mut b);
                edge.which_edge += 3;
            }
            let key = [a[0], a[1], a[2], b[0], b[1], b[2]].map(key_bits);
            match open.entry(key) {
                Entry::Vacant(v) => {
                    v.insert(vec![edge]);
                }
                Entry::Occupied(mut o) => {
                    let list = o.get_mut();
                    // The first unmatched equal edge of another facet, in insertion order.
                    if let Some(k) = list.iter().position(|e| e.facet != edge.facet) {
                        let other = list.remove(k);
                        record_neighbors(&mut neighbors, edge, other);
                    } else {
                        list.push(edge);
                    }
                }
            }
        }
    }

    // stl_generate_shared_vertices
    let mut indices = vec![[-1i32; 3]; n];
    let mut vertices: Vec<[f32; 3]> = Vec::with_capacity(n / 2);
    let mut stamp = 0u32;
    let mut visited = vec![0u32; n];
    for facet_idx in 0..n {
        for j in 0..3 {
            if indices[facet_idx][j] != -1 {
                continue;
            }
            vertices.push(facets[facet_idx][j]);
            let mut facet_in_fan = facet_idx;
            let mut edge_direction = false;
            let mut traversal_reversed = false;
            let mut vnot = ((j + 2) % 3) as i32;
            stamp += 1;
            loop {
                let (pivot, next_edge);
                if vnot > 2 {
                    if !edge_direction {
                        pivot = (vnot + 2) % 3;
                        next_edge = pivot;
                    } else {
                        pivot = (vnot + 1) % 3;
                        next_edge = vnot % 3;
                    }
                    edge_direction = !edge_direction;
                } else if !edge_direction {
                    pivot = (vnot + 1) % 3;
                    next_edge = vnot;
                } else {
                    pivot = (vnot + 2) % 3;
                    next_edge = pivot;
                }
                indices[facet_in_fan][pivot as usize] = (vertices.len() - 1) as i32;
                visited[facet_in_fan] = stamp;
                let next_facet = neighbors[facet_in_fan].neighbor[next_edge as usize];
                if next_facet == -1 {
                    if traversal_reversed {
                        break;
                    }
                    edge_direction = true;
                    vnot = ((j + 1) % 3) as i32;
                    traversal_reversed = true;
                    facet_in_fan = facet_idx;
                } else if next_facet as usize == facet_idx
                    || next_facet as usize >= n
                    || visited[next_facet as usize] == stamp
                {
                    // Closed fan, invalid mesh, or a fan revisiting itself (non-manifold).
                    break;
                } else {
                    vnot = neighbors[facet_in_fan].which_vertex_not[next_edge as usize];
                    facet_in_fan = next_facet as usize;
                }
            }
        }
    }
    TriangleMesh {
        vertices,
        faces: indices.iter().map(|f| f.map(|v| v as u32)).collect(),
    }
}

fn record_neighbors(neighbors: &mut [Neighbors], a: HashEdge, b: HashEdge) {
    let (fa, ea) = (a.facet as usize, (a.which_edge % 3) as usize);
    let (fb, eb) = (b.facet as usize, (b.which_edge % 3) as usize);
    neighbors[fa].neighbor[ea] = b.facet;
    neighbors[fa].which_vertex_not[ea] = (b.which_edge + 2) % 3;
    neighbors[fb].neighbor[eb] = a.facet;
    neighbors[fb].which_vertex_not[eb] = (a.which_edge + 2) % 3;
    if (a.which_edge < 3 && b.which_edge < 3) || (a.which_edge > 2 && b.which_edge > 2) {
        neighbors[fa].which_vertex_not[ea] += 3;
        neighbors[fb].which_vertex_not[eb] += 3;
    }
}
