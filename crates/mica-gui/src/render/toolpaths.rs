// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The G-code preview on the GPU (`segments.wgsl`):
//! - **Resident records.** A slice result's records and attributes are uploaded once, in
//!   chunks of [`CHUNK`] records (each within the default storage-binding limit), and stay on
//!   the GPU; a frame reads nothing from disk and waits for nothing.
//! - **Visible lists.** Each chunk keeps the indices of the records whose role and tool are
//!   shown, rebuilt only when the legend's visibility changes (like libvgcode's enabled
//!   segments). A layer or move range is a sub-range of those lists, found by binary search,
//!   so hidden roles cost nothing and the sliders upload nothing.
//! - **Level of detail.** [`plan`] splits the shown range into runs: the current layer (top
//!   of the layer slider, including the move slider's range) is always a full tube with tube
//!   shading; layers whose height projects to at least [`NEAR_LAYER_PX`] are tubes; the rest
//!   are tubes when the view is still and ribbons (2 triangles, a round-tube impostor) while
//!   the camera or the sliders move. The section view draws tubes throughout. Sub-pixel
//!   layers shade with their surface's macro normal in either form, which removes the moiré.
//! - **Hover pick.** When the view is still, the visible moves are also drawn into an index
//!   buffer (record + 1, depth-tested and cut by the section plane like the image); a hover
//!   reads the texels around the pointer back asynchronously ([`Pick`]), so every drawn move
//!   in the whole range can be picked and a hidden one never is.

use crate::preview::{Layer, Preview, Segment, TRAVEL};
use crate::scene::camera::Camera;
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::Vec3;
use std::{
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

/// Records per GPU chunk: 64 MiB of records, within wgpu's default 128 MiB storage binding.
pub const CHUNK: u64 = 1 << 21;
/// A layer at least this many pixels high is near: always drawn as tubes.
pub const NEAR_LAYER_PX: f32 = 1.5;
/// How long the view must be still before distant layers are refined to tubes.
pub const REFINE_DELAY: Duration = Duration::from_millis(150);
pub const TUBE_VERTICES: u32 = 36;
pub const RIBBON_VERTICES: u32 = 6;
/// The hover pick searches this many pixels around the pointer at most (one 256-byte row).
pub const MAX_PICK_RADIUS: u32 = 31;

/// How a run of moves is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// A six-sided tube with tube shading at every size (the current layer).
    Full,
    /// A tube whose shading turns to the surface's macro normal on sub-pixel layers.
    Tube,
    /// A camera-facing quad shaded like the tube (distant layers while the view moves).
    Ribbon,
}
impl Detail {
    pub fn vertices(self) -> u32 {
        match self {
            Self::Ribbon => RIBBON_VERTICES,
            _ => TUBE_VERTICES,
        }
    }
}
/// Consecutive records drawn at one level of detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub records: Range<u64>,
    pub detail: Detail,
}

/// A layer's extent (bed coordinates, extrusions only) and its thickest move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerExtent {
    pub min: Vec3,
    pub max: Vec3,
    pub height: f32,
}
impl LayerExtent {
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
        height: 0.,
    };
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
    pub fn include(&mut self, s: &Segment) {
        for p in [s.a, s.b] {
            self.min = self.min.min(Vec3::from(p));
            self.max = self.max.max(Vec3::from(p));
        }
        self.height = self.height.max((s.size >> 16) as f32 / 1000.);
    }
}

/// Pixels per millimetre of the view at a depth: what a layer projects to.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    eye: Vec3,
    forward: Vec3,
    perspective: bool,
    /// Perspective: pixels per mm at unit depth; orthographic: pixels per mm.
    k: f32,
    /// The toolpaths' world offset (`render::preview_model`).
    offset: Vec3,
}
impl Scale {
    pub fn new(camera: &Camera, offset: Vec3, height_px: f32) -> Self {
        let k = if camera.perspective {
            height_px / (2. * (camera.fov / 2.).tan())
        } else {
            height_px / camera.span.max(1e-3)
        };
        Self {
            eye: camera.eye(),
            forward: -camera.direction(),
            perspective: camera.perspective,
            k,
            offset,
        }
    }
    /// Perspective: pixels per mm at unit depth; orthographic: pixels per mm (the shader's
    /// `extra.z`).
    pub fn pixels_per_mm(&self) -> f32 {
        self.k
    }
    /// How many pixels the layer's height covers where it comes closest to the camera
    /// (infinite when it reaches the camera); 0 for a layer without extrusions.
    pub fn layer_px(&self, e: &LayerExtent) -> f32 {
        if e.is_empty() {
            return 0.;
        }
        if !self.perspective {
            return e.height * self.k;
        }
        let (lo, hi) = (e.min + self.offset, e.max + self.offset);
        // The nearest depth of a box is at one of its corners.
        let depth = (0..8)
            .map(|i| {
                let c = Vec3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                );
                (c - self.eye).dot(self.forward)
            })
            .fold(f32::INFINITY, f32::min);
        if depth <= 1e-3 {
            return f32::INFINITY;
        }
        e.height * self.k / depth
    }
}

/// The runs that draw `range` (records) when `top` is the layer slider's upper layer:
/// `near(layer)` says whether a layer projects to at least [`NEAR_LAYER_PX`]; `moving` is set
/// while the camera or a slider moves; `full` (the section view) draws every layer as tubes.
pub fn plan(
    layers: &[Layer],
    range: Range<u64>,
    top: usize,
    near: impl Fn(usize) -> bool,
    moving: bool,
    full: bool,
) -> Vec<Run> {
    let mut runs: Vec<Run> = vec![];
    if range.is_empty() || layers.is_empty() {
        return runs;
    }
    let top = top.min(layers.len() - 1);
    let top_start = layers[top].start.clamp(range.start, range.end);
    let mut push = |records: Range<u64>, detail| {
        if records.is_empty() {
            return;
        }
        match runs.last_mut() {
            Some(r) if r.detail == detail && r.records.end == records.start => {
                r.records.end = records.end
            }
            _ => runs.push(Run { records, detail }),
        }
    };
    for (i, l) in layers.iter().enumerate().take(top) {
        let end = layers.get(i + 1).map_or(l.start + l.count, |n| n.start);
        let records = l.start.max(range.start)..end.min(top_start);
        if records.is_empty() {
            continue;
        }
        let detail = if full || !moving || near(i) {
            Detail::Tube
        } else {
            Detail::Ribbon
        };
        push(records, detail);
    }
    push(top_start..range.end, Detail::Full);
    runs
}

/// The record kinds (`Segment::meta & 0xff`: role, tool) and the visible-record lists built
/// from them, one per chunk.
#[derive(Clone, Debug, Default)]
pub struct VisibleLists {
    kinds: Vec<u8>,
    chunk: u64,
    mask: Option<(u32, u32)>,
    pub lists: Vec<Vec<u32>>,
}
impl VisibleLists {
    pub fn new(kinds: Vec<u8>, chunk: u64) -> Self {
        Self {
            kinds,
            chunk,
            mask: None,
            lists: vec![],
        }
    }
    /// Rebuilds the lists for these role / tool masks unless they are current; whether it did.
    pub fn update(&mut self, mask: u32, tool_mask: u32) -> bool {
        if self.mask == Some((mask, tool_mask)) {
            return false;
        }
        self.lists = self
            .kinds
            .chunks(self.chunk.max(1) as usize)
            .map(|k| visible_indices(k, mask, tool_mask))
            .collect();
        self.mask = Some((mask, tool_mask));
        true
    }
}
/// The indices of the records whose role and tool pass the masks (`View::shows`).
pub fn visible_indices(kinds: &[u8], mask: u32, tool_mask: u32) -> Vec<u32> {
    kinds
        .iter()
        .enumerate()
        .filter(|(_, k)| mask & (1 << (**k & 15)) != 0 && tool_mask & (1 << (**k >> 4)) != 0)
        .map(|(i, _)| i as u32)
        .collect()
}
/// The instances of a visible list (records from `base`) that fall in `records`.
pub fn instances(list: &[u32], base: u64, records: &Range<u64>) -> Range<u32> {
    let lo = list.partition_point(|&i| base + (i as u64) < records.start);
    let hi = list.partition_point(|&i| base + (i as u64) < records.end);
    lo as u32..hi.max(lo) as u32
}

/// One draw: a chunk's instances at one level of detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draw {
    pub chunk: usize,
    pub instances: Range<u32>,
    pub detail: Detail,
}

struct Chunk {
    base: u64,
    group: wgpu::BindGroup,
    visible: wgpu::Buffer,
    _records: wgpu::Buffer,
    _attrs: wgpu::Buffer,
    _uniform: wgpu::Buffer,
}
/// A preview's records on the GPU.
pub struct Resident {
    /// The preview it holds: job folder and record count.
    pub key: (std::path::PathBuf, u64),
    chunks: Vec<Chunk>,
    lists: VisibleLists,
    pub extents: Vec<LayerExtent>,
}
impl Resident {
    /// Uploads every record and attribute of `preview` (`preview.bin`, `attrs.bin`).
    pub fn load(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        preview: &Preview,
    ) -> std::io::Result<Self> {
        let count = preview.count;
        let layers = &preview.layers;
        let mut kinds = Vec::with_capacity(count as usize);
        let mut extents = vec![LayerExtent::EMPTY; layers.len()];
        let mut chunks = vec![];
        let mut layer = 0;
        let mut index = 0u64;
        for base in (0..count).step_by(CHUNK as usize) {
            let len = CHUNK.min(count - base);
            let mapped = |label, size: u64| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: true,
                })
            };
            let records = mapped("Mica preview records", len * 32);
            let attrs = mapped(
                "Mica preview attributes",
                len * crate::preview::attrs::SIZE as u64,
            );
            {
                let error = |e: wgpu::MapRangeError| std::io::Error::other(e.to_string());
                let mut rv = records.slice(..).get_mapped_range_mut().map_err(error)?;
                let mut av = attrs.slice(..).get_mapped_range_mut().map_err(error)?;
                let mut off = 0usize;
                preview.read_batches_with_attrs(base..base + len, |s, a| {
                    let n = s.len();
                    rv.slice(off * 32..(off + n) * 32)
                        .copy_from_slice(bytemuck::cast_slice(s));
                    let size = crate::preview::attrs::SIZE;
                    av.slice(off * size..(off + n) * size)
                        .copy_from_slice(bytemuck::cast_slice(a));
                    for seg in s {
                        kinds.push((seg.meta & 0xff) as u8);
                        while layer + 1 < layers.len() && layers[layer + 1].start <= index {
                            layer += 1;
                        }
                        if seg.role() != TRAVEL
                            && let Some(e) = extents.get_mut(layer)
                        {
                            e.include(seg);
                        }
                        index += 1;
                    }
                    off += n;
                })?;
            }
            records.unmap();
            attrs.unmap();
            let visible = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mica visible records"),
                size: len * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Mica preview chunk"),
                contents: bytemuck::cast_slice(&[base as u32, 0, 0, 0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mica preview chunk"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: records.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: attrs.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: visible.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            });
            chunks.push(Chunk {
                base,
                group,
                visible,
                _records: records,
                _attrs: attrs,
                _uniform: uniform,
            });
        }
        Ok(Self {
            key: (preview.job.path.clone(), count),
            chunks,
            lists: VisibleLists::new(kinds, CHUNK),
            extents,
        })
    }
    /// Uploads the visible lists for these masks when they changed.
    pub fn ensure_visible(&mut self, queue: &wgpu::Queue, mask: u32, tool_mask: u32) {
        if !self.lists.update(mask, tool_mask) {
            return;
        }
        for (chunk, list) in self.chunks.iter().zip(&self.lists.lists) {
            if !list.is_empty() {
                queue.write_buffer(&chunk.visible, 0, bytemuck::cast_slice(list));
            }
        }
    }
    /// The draws of `runs`.
    pub fn draws(&self, runs: &[Run]) -> Vec<Draw> {
        let mut out = vec![];
        for run in runs {
            for (c, (chunk, list)) in self.chunks.iter().zip(&self.lists.lists).enumerate() {
                let instances = instances(list, chunk.base, &run.records);
                if !instances.is_empty() {
                    out.push(Draw {
                        chunk: c,
                        instances,
                        detail: run.detail,
                    });
                }
            }
        }
        out
    }
    pub fn group(&self, chunk: usize) -> &wgpu::BindGroup {
        &self.chunks[chunk].group
    }
}

/// Where to look for a move: a pixel of the viewport (render target pixels) and the search
/// radius around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PickRequest {
    pub px: [u32; 2],
    pub radius: u32,
}
/// The answer to a request: the record drawn nearest the pixel, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickResult {
    pub request: PickRequest,
    pub record: Option<u64>,
    /// Counts answers, so a new answer to the same request (the view changed) is seen.
    pub serial: u64,
}
/// Shared between the viewport (which sets the request each frame and reads the answers) and
/// the renderer (which answers from the index buffer).
#[derive(Debug, Default)]
pub struct Pick {
    pub request: Option<PickRequest>,
    pub result: Option<PickResult>,
}
pub type SharedPick = Arc<Mutex<Pick>>;

/// The record nearest `center` within `radius` in a block of index texels (record + 1, 0 =
/// nothing), `width` texels per row.
pub fn nearest_record(texels: &[u32], width: usize, center: [i64; 2], radius: u32) -> Option<u64> {
    let r2 = (radius as i64).pow(2);
    let mut best: Option<(i64, u32)> = None;
    for (i, &id) in texels.iter().enumerate() {
        if id == 0 {
            continue;
        }
        let (x, y) = ((i % width) as i64, (i / width) as i64);
        let d = (x - center[0]).pow(2) + (y - center[1]).pow(2);
        if d <= r2 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    best.map(|(_, id)| id as u64 - 1)
}

/// A read-back of the index buffer in flight.
pub struct PickReadback {
    pub request: PickRequest,
    /// The frame (hash) whose index buffer it reads.
    pub key: u64,
    pub buffer: wgpu::Buffer,
    pub width: u32,
    pub rows: u32,
    /// The pointer within the block.
    pub center: [i64; 2],
    /// 0 waiting, 1 mapped, 2 failed.
    pub state: Arc<AtomicU8>,
}
impl PickReadback {
    /// Copies the texels around `request` from the index texture and starts mapping them.
    pub fn start(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        size: [u32; 2],
        request: PickRequest,
        key: u64,
    ) -> Option<Self> {
        let r = request.radius.min(MAX_PICK_RADIUS);
        let [x, y] = request.px;
        if x >= size[0] || y >= size[1] {
            return None;
        }
        let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
        let (x1, y1) = ((x + r + 1).min(size[0]), (y + r + 1).min(size[1]));
        let (width, rows) = (x1 - x0, y1 - y0);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mica pick readback"),
            size: 256 * rows as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(rows),
                },
            },
            wgpu::Extent3d {
                width,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let state = Arc::new(AtomicU8::new(0));
        let s = state.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            s.store(if r.is_ok() { 1 } else { 2 }, Ordering::Release);
        });
        Some(Self {
            request,
            key,
            buffer,
            width,
            rows,
            center: [(x - x0) as i64, (y - y0) as i64],
            state,
        })
    }
    /// The answer once the copy is mapped: `None` while waiting, `Some(None)` when it failed.
    pub fn finish(&self) -> Option<Option<Option<u64>>> {
        match self.state.load(Ordering::Acquire) {
            0 => None,
            2 => Some(None),
            _ => {
                let data = self.buffer.slice(..).get_mapped_range().ok()?;
                let mut texels = Vec::with_capacity((self.width * self.rows) as usize);
                for row in data.as_chunks::<256>().0.iter().take(self.rows as usize) {
                    let row: &[u32] = bytemuck::cast_slice(row.as_slice());
                    texels.extend_from_slice(&row[..self.width as usize]);
                }
                drop(data);
                self.buffer.unmap();
                Some(Some(nearest_record(
                    &texels,
                    self.width as usize,
                    self.center,
                    self.request.radius.min(MAX_PICK_RADIUS),
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layers(counts: &[u64]) -> Vec<Layer> {
        let mut start = 0;
        counts
            .iter()
            .enumerate()
            .map(|(i, &count)| {
                let l = Layer {
                    start,
                    count,
                    z: 0.2 * (i + 1) as f32,
                    time_s: 1.,
                };
                start += count;
                l
            })
            .collect()
    }

    /// The current layer is always a full tube, near layers are tubes, far layers are ribbons
    /// only while moving, and the section view keeps tubes.
    #[test]
    fn plan_keeps_the_current_layer_and_near_layers_at_full_detail() {
        let l = layers(&[10, 10, 10, 10]);
        let near = |i: usize| i == 1;
        // Still: everything tubes, the current layer full.
        assert_eq!(
            plan(&l, 0..40, 3, near, false, false),
            [
                Run {
                    records: 0..30,
                    detail: Detail::Tube
                },
                Run {
                    records: 30..40,
                    detail: Detail::Full
                }
            ]
        );
        // Moving: far layers ribbons, the near one a tube, the current layer full.
        assert_eq!(
            plan(&l, 0..40, 3, near, true, false)
                .iter()
                .map(|r| (r.records.clone(), r.detail))
                .collect::<Vec<_>>(),
            [
                (0..10, Detail::Ribbon),
                (10..20, Detail::Tube),
                (20..30, Detail::Ribbon),
                (30..40, Detail::Full)
            ]
        );
        // The section view: tubes while moving too.
        assert!(
            plan(&l, 0..40, 3, near, true, true)
                .iter()
                .all(|r| r.detail != Detail::Ribbon)
        );
        // The move slider cuts the current layer; the layer slider's range limits the rest.
        assert_eq!(
            plan(&l, 10..34, 3, |_| false, true, false)
                .iter()
                .map(|r| (r.records.clone(), r.detail))
                .collect::<Vec<_>>(),
            [(10..30, Detail::Ribbon), (30..34, Detail::Full)]
        );
        // A single layer is all current layer.
        assert_eq!(
            plan(&l, 20..30, 2, |_| false, true, false)
                .iter()
                .map(|r| (r.records.clone(), r.detail))
                .collect::<Vec<_>>(),
            [(20..30, Detail::Full)]
        );
        assert!(plan(&l, 5..5, 3, near, true, false).is_empty());
    }

    /// A layer is near once its height projects to 1.5 px, by depth in perspective and by
    /// span in orthographic views.
    #[test]
    fn layer_pixels_follow_the_camera() {
        let mut c = Camera::looking_at(Vec3::new(0., 0., 0.), 330.);
        c.pitch = 0.;
        c.yaw = 0.;
        let e = LayerExtent {
            min: Vec3::new(-1., -1., 0.),
            max: Vec3::new(1., 1., 0.2),
            height: 0.2,
        };
        // Perspective: pixels per mm at depth d is H / (2 tan(fov/2) d).
        let s = Scale::new(&c, Vec3::ZERO, 1000.);
        let d = c.distance() - 1.;
        let want = 0.2 * 1000. / (2. * (c.fov / 2.).tan() * d);
        assert!((s.layer_px(&e) - want).abs() < 1e-3, "{}", s.layer_px(&e));
        assert!(s.layer_px(&e) < NEAR_LAYER_PX);
        // Zoomed in to a 20 mm span the same layer is near.
        let mut z = c.clone();
        z.span = 20.;
        assert!(Scale::new(&z, Vec3::ZERO, 1000.).layer_px(&e) >= NEAR_LAYER_PX);
        // The plate offset moves the layer away.
        let far = Scale::new(&z, Vec3::new(-500., 0., 0.), 1000.).layer_px(&e);
        assert!(far < Scale::new(&z, Vec3::ZERO, 1000.).layer_px(&e));
        // Orthographic: span only.
        let mut o = c.clone();
        o.perspective = false;
        o.span = 100.;
        assert!((Scale::new(&o, Vec3::ZERO, 1000.).layer_px(&e) - 2.).abs() < 1e-4);
        o.span = 200.;
        assert!((Scale::new(&o, Vec3::ZERO, 1000.).layer_px(&e) - 1.).abs() < 1e-4);
        // A layer without extrusions is never near; one reaching the camera always is.
        assert_eq!(s.layer_px(&LayerExtent::EMPTY), 0.);
        let around = LayerExtent {
            min: Vec3::splat(-1e4),
            max: Vec3::splat(1e4),
            height: 0.2,
        };
        assert!(s.layer_px(&around).is_infinite());
    }

    /// Visibility changes rebuild the lists; the same masks do not; ranges map onto them.
    #[test]
    fn visible_lists_follow_the_legend() {
        // role | tool << 4: outer wall (2) tool 0, travel (14) tool 0, infill (5) tool 1 …
        let kinds: Vec<u8> = (0..10u8)
            .map(|i| [2, 14, 5 | 1 << 4][i as usize % 3])
            .collect();
        let mut v = VisibleLists::new(kinds, 4);
        let all_tools = u32::MAX;
        assert!(v.update(!(1 << TRAVEL), all_tools));
        // Chunks of 4: [0 1 2 3] [4 5 6 7] [8 9] without travel (1, 4, 7).
        assert_eq!(v.lists, [vec![0, 2, 3], vec![1, 2], vec![0, 1]]);
        assert!(!v.update(!(1 << TRAVEL), all_tools));
        // Hiding tool 1 drops the infill.
        assert!(v.update(!(1 << TRAVEL), 1));
        assert_eq!(v.lists, [vec![0, 3], vec![2], vec![1]]);
        // Outer walls only.
        assert!(v.update(1 << 2, all_tools));
        assert_eq!(v.lists, [vec![0, 3], vec![2], vec![1]]);
        assert!(v.update(0, all_tools));
        assert!(v.lists.iter().all(Vec::is_empty));
        // A record range picks the matching instances of each chunk's list.
        let list = [0u32, 2, 3];
        assert_eq!(instances(&list, 0, &(0..4)), 0..3);
        assert_eq!(instances(&list, 0, &(1..3)), 1..2);
        assert_eq!(instances(&list, 0, &(3..10)), 2..3);
        assert_eq!(instances(&[1, 2], 4, &(0..5)), 0..0);
        assert_eq!(instances(&[1, 2], 4, &(0..6)), 0..1);
        assert_eq!(instances(&[1, 2], 4, &(9..12)), 2..2);
    }

    /// The pick takes the drawn record nearest the pointer within the radius.
    #[test]
    fn nearest_record_in_the_index_block() {
        // A 5 × 3 block.
        #[rustfmt::skip]
        let texels = [
            0, 0, 0, 0, 8,
            0, 0, 0, 0, 0,
            4, 0, 0, 0, 0,
        ];
        assert_eq!(nearest_record(&texels, 5, [1, 1], 3), Some(3));
        assert_eq!(nearest_record(&texels, 5, [3, 0], 3), Some(7));
        assert_eq!(nearest_record(&texels, 5, [1, 1], 1), None);
        let mut hit = texels;
        hit[7] = 100;
        assert_eq!(nearest_record(&hit, 5, [2, 1], 0), Some(99));
        assert_eq!(nearest_record(&[0; 15], 5, [2, 1], 5), None);
    }
}
