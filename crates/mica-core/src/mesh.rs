// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Triangle meshes and STL loading.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;

/// An indexed triangle mesh in millimetres. Faces are counter-clockwise seen from outside.
#[derive(Clone, Debug, Default)]
pub struct TriangleMesh {
    pub vertices: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
}

#[derive(Debug)]
pub enum MeshError {
    Io(io::Error),
    Parse(String),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshError::Io(e) => write!(f, "{e}"),
            MeshError::Parse(msg) => write!(f, "invalid STL: {msg}"),
        }
    }
}

impl std::error::Error for MeshError {}

impl From<io::Error> for MeshError {
    fn from(e: io::Error) -> Self {
        MeshError::Io(e)
    }
}

impl TriangleMesh {
    /// Heap bytes of the vertex and face arrays (for bounding caches and undo history).
    pub fn heap_bytes(&self) -> usize {
        self.vertices.capacity() * std::mem::size_of::<[f32; 3]>()
            + self.faces.capacity() * std::mem::size_of::<[u32; 3]>()
    }
    /// Loads a binary or ASCII STL file. Binary files are streamed record by record, so neither the
    /// file nor a copy of its loose triangles is held in memory.
    pub fn load_stl(path: impl AsRef<Path>) -> Result<Self, MeshError> {
        use std::io::Read;
        let path = path.as_ref();
        let len = std::fs::metadata(path)?.len();
        let mut file = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
        let mut header = [0u8; 84];
        let header_len = read_up_to(&mut file, &mut header)?;
        let count = if header_len == 84 {
            u32::from_le_bytes([header[80], header[81], header[82], header[83]]) as u64
        } else {
            0
        };
        if header_len == 84 && len == 84 + count * 50 {
            let mut builder = MeshBuilder::with_capacity(count as usize);
            let mut record = [0u8; 50];
            for i in 0..count as usize {
                file.read_exact(&mut record)?;
                builder.add(binary_triangle(&record, i)?);
            }
            return builder.finish();
        }
        // ASCII: rare and usually small; parse from memory.
        let mut bytes = header[..header_len].to_vec();
        file.read_to_end(&mut bytes)?;
        Self::from_stl_bytes(&bytes)
    }

    /// Parses STL data. Binary is detected by its exact size; anything else is parsed as ASCII.
    pub fn from_stl_bytes(bytes: &[u8]) -> Result<Self, MeshError> {
        if is_binary_stl(bytes) {
            let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
            let mut builder = MeshBuilder::with_capacity(count);
            for (i, record) in bytes[84..].as_chunks::<50>().0.iter().enumerate() {
                builder.add(binary_triangle(record, i)?);
            }
            builder.finish()
        } else {
            let mut builder = MeshBuilder::with_capacity(0);
            parse_ascii(bytes, |tri| builder.add(tri))?;
            builder.finish()
        }
    }

    /// Builds an indexed mesh from loose triangles, merging bit-identical vertices.
    pub fn from_triangles(triangles: &[[[f32; 3]; 3]]) -> Self {
        let mut builder = MeshBuilder::with_capacity(triangles.len());
        for tri in triangles {
            builder.add(*tri);
        }
        builder.mesh
    }

    /// Minimum and maximum corner of the bounding box.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in &self.vertices {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
        (min, max)
    }

    pub fn translate(&mut self, d: [f32; 3]) {
        for v in &mut self.vertices {
            for i in 0..3 {
                v[i] += d[i];
            }
        }
    }

    /// Moves the mesh so it rests on z = 0 with its XY bounding-box centre at (`x`, `y`).
    pub fn place_on_bed(&mut self, x: f32, y: f32) {
        let (min, max) = self.bounds();
        self.translate([
            x - (min[0] + max[0]) / 2.0,
            y - (min[1] + max[1]) / 2.0,
            -min[2],
        ]);
    }

    /// Enclosed volume in mm³ (positive for outward-facing triangles).
    pub fn volume(&self) -> f64 {
        self.faces
            .iter()
            .map(|f| {
                let [a, b, c] = f.map(|i| self.vertices[i as usize].map(f64::from));
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum()
    }
}

/// Builds an indexed mesh triangle by triangle, merging bit-identical vertices.
struct MeshBuilder {
    index: HashMap<[u32; 3], u32>,
    mesh: TriangleMesh,
}

impl MeshBuilder {
    fn with_capacity(triangles: usize) -> Self {
        let mut mesh = TriangleMesh::default();
        mesh.faces.reserve(triangles);
        mesh.vertices.reserve(triangles / 2);
        Self {
            index: HashMap::with_capacity(triangles / 2),
            mesh,
        }
    }

    fn add(&mut self, tri: [[f32; 3]; 3]) {
        let mut face = [0u32; 3];
        for (slot, v) in face.iter_mut().zip(tri) {
            // -0.0 and 0.0 must merge, so normalise before hashing the bit pattern.
            let v = v.map(|c| if c == 0.0 { 0.0 } else { c });
            let mesh = &mut self.mesh;
            *slot = *self.index.entry(v.map(f32::to_bits)).or_insert_with(|| {
                mesh.vertices.push(v);
                (mesh.vertices.len() - 1) as u32
            });
        }
        // Drop triangles that collapsed to a line or point.
        if face[0] != face[1] && face[1] != face[2] && face[0] != face[2] {
            self.mesh.faces.push(face);
        }
    }

    fn finish(self) -> Result<TriangleMesh, MeshError> {
        let mut mesh = self.mesh;
        if mesh.faces.is_empty() {
            return Err(MeshError::Parse("no triangles".into()));
        }
        mesh.vertices.shrink_to_fit();
        mesh.faces.shrink_to_fit();
        Ok(mesh)
    }
}

/// Fills `buf` as far as the reader allows; returns how many bytes were read.
fn read_up_to(r: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn is_binary_stl(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    bytes.len() == 84 + count * 50
}

/// One 50-byte binary STL record: 12 bytes normal (ignored), 3 × 12 bytes vertices, 2 bytes attribute.
fn binary_triangle(record: &[u8], i: usize) -> Result<[[f32; 3]; 3], MeshError> {
    let f = |o: usize| f32::from_le_bytes([record[o], record[o + 1], record[o + 2], record[o + 3]]);
    let v = |k: usize| [f(12 + k * 12), f(16 + k * 12), f(20 + k * 12)];
    let tri = [v(0), v(1), v(2)];
    if tri.iter().flatten().any(|c| !c.is_finite()) {
        return Err(MeshError::Parse(format!(
            "triangle {i} has a non-finite coordinate"
        )));
    }
    Ok(tri)
}

fn parse_ascii(bytes: &[u8], mut add: impl FnMut([[f32; 3]; 3])) -> Result<(), MeshError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| MeshError::Parse("neither binary nor UTF-8 ASCII".into()))?;
    let mut current: Vec<[f32; 3]> = Vec::with_capacity(3);
    for (line_no, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        if words.next() != Some("vertex") {
            continue;
        }
        let coords: Vec<f32> = words
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| MeshError::Parse(format!("line {}: bad vertex", line_no + 1)))?;
        if coords.len() != 3 {
            return Err(MeshError::Parse(format!(
                "line {}: vertex needs 3 coordinates",
                line_no + 1
            )));
        }
        current.push([coords[0], coords[1], coords[2]]);
        if current.len() == 3 {
            add([current[0], current[1], current[2]]);
            current.clear();
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Axis-aligned box from (x0,y0,z0) to (x1,y1,z1), outward-facing.
    pub(crate) fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> TriangleMesh {
        let p = |i: usize, j: usize, k: usize| {
            [[lo[0], hi[0]][i], [lo[1], hi[1]][j], [lo[2], hi[2]][k]]
        };
        let quads = [
            [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)],
            [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)],
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)],
            [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)],
            [(1, 1, 0), (0, 1, 0), (0, 1, 1), (1, 1, 1)],
            [(0, 1, 0), (0, 0, 0), (0, 0, 1), (0, 1, 1)],
        ];
        let mut tris = Vec::new();
        for q in quads {
            let [a, b, c, d] = q.map(|(i, j, k)| p(i, j, k));
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
        TriangleMesh::from_triangles(&tris)
    }

    #[test]
    fn box_is_indexed_and_has_volume() {
        let m = box_mesh([0.0; 3], [20.0, 20.0, 20.0]);
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.faces.len(), 12);
        assert!((m.volume() - 8000.0).abs() < 1e-6);
    }

    #[test]
    fn ascii_and_binary_agree() {
        let ascii = b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let a = TriangleMesh::from_stl_bytes(ascii).unwrap();
        let mut bin = vec![0u8; 80];
        bin.extend(1u32.to_le_bytes());
        bin.extend([0u8; 12]);
        for v in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
            for c in v {
                bin.extend(c.to_le_bytes());
            }
        }
        bin.extend([0u8; 2]);
        let b = TriangleMesh::from_stl_bytes(&bin).unwrap();
        assert_eq!(a.vertices, b.vertices);
        assert_eq!(a.faces, b.faces);
    }

    #[test]
    fn test_models_load() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/");
        let cube = TriangleMesh::load_stl(format!("{dir}cube20.stl")).unwrap();
        assert!((cube.volume() - 8000.0).abs() < 1e-3);
        let block = TriangleMesh::load_stl(format!("{dir}hole_block.stl")).unwrap();
        assert!((block.volume() - 3498.155).abs() < 1e-2);
    }

    #[test]
    fn place_on_bed_centres_and_grounds() {
        let mut m = box_mesh([-5.0, -5.0, 3.0], [5.0, 5.0, 13.0]);
        m.place_on_bed(125.0, 110.0);
        let (min, max) = m.bounds();
        assert_eq!(min, [120.0, 105.0, 0.0]);
        assert_eq!(max, [130.0, 115.0, 10.0]);
    }
}
