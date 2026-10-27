// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! wgpu rendering of the 3D view: 4× MSAA colour + depth, a single-sample selection mask,
//! and a composite pass that blits into egui and draws outlines. Pipelines share one uniform
//! layout; per-draw uniforms come from a reusable slot pool. The bed is either the printer's
//! real sheet (model + texture from `scene::bed_assets`) or a drawn plate with a grid. The
//! G-code preview's toolpaths live on the GPU with levels of detail and an index-buffer hover
//! pick (`toolpaths`). Nothing here waits for the GPU: a frame is recorded and submitted once.

pub mod bench;
#[cfg(test)]
mod gpu_tests;
pub mod screenshot;
mod surface;
mod tags;
pub mod toolpaths;
use crate::{
    preview::{ColorMode, Preview, View},
    scene::{Bed, Object, Placement, bed, bed_assets, camera::Camera, plates::Plate},
    theme::{self, ViewportColors},
};
use bytemuck::{Pod, Zeroable};
use eframe::egui;
use egui_wgpu::{
    CallbackResources, CallbackTrait, ScreenDescriptor,
    wgpu::{self, util::DeviceExt},
};
use glam::{Mat4, Vec3};
use std::{collections::HashMap, sync::Arc};
pub use tags::{TagDraw, TagImage};

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const SAMPLES: u32 = 4;
/// Red: selection (1.0) and hover (0.5), drawn through other objects; green: the visible
/// part of an object outside the printable volume (depth-tested).
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The hover pick's index buffer: record + 1 per pixel, 0 = no move.
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
// Reserved mesh ids; object ids count up from 1 and never reach these.
const BED_ID: u64 = u64::MAX;
const EXCLUSION_ID: u64 = u64::MAX - 1;
const OUTLINE_ID: u64 = u64::MAX - 2;
const OUTLINE_CURRENT_ID: u64 = u64::MAX - 3;
const BED_MODEL_ID: u64 = u64::MAX - 4;
const TEXTURE_ID: u64 = u64::MAX - 5;
/// A unit cube (−0.5..0.5), scaled per draw: height-range slabs and the cut plane.
const UNIT_CUBE_ID: u64 = u64::MAX - 6;
const RESERVED_FROM: u64 = UNIT_CUBE_ID;

/// A translucent volume to draw over the objects: a modifier, negative volume, support
/// volume, height-range slab or the cut plane.
#[derive(Clone, Debug)]
pub struct VolumeDraw {
    /// GPU mesh key (the volume's id); `None` draws the unit cube under `transform`.
    pub mesh: Option<(u64, Arc<mica_core::mesh::TriangleMesh>)>,
    /// The key is a scene volume's id, whose mesh can be replaced under it (a text rebuilt
    /// while it is edited): the GPU copy follows the mesh itself. Untracked keys name their
    /// content (connector shapes, tower footprints) and may come with a fresh `Arc` each frame.
    pub tracked: bool,
    pub transform: Mat4,
    /// RGBA; the alpha is the translucency.
    pub color: [f32; 4],
    /// Draw the grey cut-out stripes (negative volumes).
    pub cutout: bool,
    pub selected: bool,
    pub hovered: bool,
}
impl VolumeDraw {
    /// What identifies the drawn mesh for the redraw check: its key, and for a tracked key
    /// the mesh itself (an untracked one is content-named and may be a fresh copy).
    fn mesh_identity(&self) -> Option<(u64, Option<usize>)> {
        self.mesh
            .as_ref()
            .map(|(id, m)| (*id, self.tracked.then_some(Arc::as_ptr(m) as usize)))
    }
}

/// Whether the GPU copy under `id` was made from this very `mesh`.
fn source_is_current<V>(
    sources: &HashMap<u64, Arc<mica_core::mesh::TriangleMesh>>,
    meshes: &HashMap<u64, V>,
    id: u64,
    mesh: &Arc<mica_core::mesh::TriangleMesh>,
) -> bool {
    meshes.contains_key(&id) && sources.get(&id).is_some_and(|m| Arc::ptr_eq(m, mesh))
}

/// Volume colours by kind (RGBA): amber modifiers, grey negatives, the support paint
/// colours for enforcers and blockers, a cool blue for height ranges, the violet accent for the cut plane.
pub fn volume_color(kind: &mica_core::threemf::PartKind, selected: bool) -> ([f32; 4], bool) {
    use mica_core::threemf::PartKind;
    let a = if selected { 0.55 } else { 0.38 };
    match kind {
        PartKind::Modifier => ([0.95, 0.66, 0.2, a], false),
        PartKind::NegativeVolume => ([0.55, 0.57, 0.6, a + 0.1], true),
        PartKind::SupportEnforcer => {
            let c = crate::scene::paint::SUPPORT_ENFORCER;
            ([c[0], c[1], c[2], a], false)
        }
        PartKind::SupportBlocker => {
            let c = crate::scene::paint::SUPPORT_BLOCKER;
            ([c[0], c[1], c[2], a], false)
        }
        // An added part (a raised text) reads as part of the model: nearly opaque.
        PartKind::Model => ([0.78, 0.78, 0.82, if selected { 0.95 } else { 0.9 }], false),
        _ => ([0.7, 0.7, 0.7, a], false),
    }
}
pub const RANGE_COLOR: [f32; 4] = [0.35, 0.55, 0.95, 0.22];
pub const CUT_PLANE_COLOR: [f32; 4] = [0.55, 0.49, 0.96, 0.4];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Uniform {
    pub vp: [[f32; 4]; 4],
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
    pub params: [u32; 4],
    pub extra: [f32; 4],
    pub light: [f32; 4],
    pub eye: [f32; 4],
    pub sky: [f32; 4],
    pub ground: [f32; 4],
    pub warning: [f32; 4],
    pub tools: [[f32; 4]; 16],
    /// Section plane `[nx, ny, nz, d]`: fragments with `n · p > d` are cut away.
    pub clip: [f32; 4],
    /// The printable volume for the outside-part test: plate origin (world XY), maximum
    /// print height, tolerance (mm).
    pub volume: [f32; 4],
    /// Point counts: bed outline, number of exclusion zones, then each zone's (see
    /// `common.wgsl`).
    pub volume_counts: [[u32; 4]; 2],
    /// The bed outline then each exclusion zone (bed mm), two points per entry.
    pub volume_points: [[f32; 4]; VOLUME_SLOTS],
    /// Colour of the part outside the volume (stripes, outline).
    pub danger: [f32; 4],
}
/// How far (mm) a fragment may lie past the volume before it counts as outside, as the
/// placement check's tolerance (`Bed::placement`).
const OUTSIDE_TOLERANCE: f32 = 0.01;
/// `Uniform::volume_points` entries (two points each).
const VOLUME_SLOTS: usize = 32;
/// Exclusion zones the shader tests (`common.wgsl`).
const VOLUME_ZONES: usize = 6;
/// The bed outline and exclusion zones packed for the shader's outside-part test: counts and
/// points. An outline with more points than fit is thinned evenly; zones that no longer fit
/// are tested by their bounding box, or left out when even that does not fit.
pub fn volume_shape(bed: &Bed) -> ([[u32; 4]; 2], [[f32; 4]; VOLUME_SLOTS]) {
    let capacity = VOLUME_SLOTS * 2;
    let mut points: Vec<[f32; 2]> = Vec::with_capacity(capacity);
    let outline_cap = capacity - 16;
    let outline: Vec<[f32; 2]> = if bed.polygon.len() > outline_cap {
        let step = bed.polygon.len() as f32 / outline_cap as f32;
        (0..outline_cap)
            .map(|k| bed.polygon[(k as f32 * step) as usize])
            .collect()
    } else {
        bed.polygon.clone()
    };
    points.extend(&outline);
    let mut counts = vec![outline.len() as u32, 0];
    for zone in bed.exclusions.iter().take(VOLUME_ZONES) {
        let fitted: Vec<[f32; 2]> = if points.len() + zone.len() <= capacity {
            zone.clone()
        } else {
            let (lo, hi) = zone.iter().fold(
                ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
                |(lo, hi), p| {
                    (
                        [lo[0].min(p[0]), lo[1].min(p[1])],
                        [hi[0].max(p[0]), hi[1].max(p[1])],
                    )
                },
            );
            vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
        };
        if points.len() + fitted.len() > capacity {
            break;
        }
        counts.push(fitted.len() as u32);
        counts[1] += 1;
        points.extend(fitted);
    }
    let mut c = [[0u32; 4]; 2];
    for (i, n) in counts.iter().enumerate() {
        c[i / 4][i % 4] = *n;
    }
    let mut out = [[0f32; 4]; VOLUME_SLOTS];
    for (i, p) in points.iter().enumerate() {
        out[i / 2][(i % 2) * 2] = p[0];
        out[i / 2][(i % 2) * 2 + 1] = p[1];
    }
    (c, out)
}
/// A clip plane that keeps everything.
pub const NO_CLIP: [f32; 4] = [0., 0., 0., 1.];
impl Default for Uniform {
    fn default() -> Self {
        Self {
            vp: Mat4::IDENTITY.to_cols_array_2d(),
            model: Mat4::IDENTITY.to_cols_array_2d(),
            color: [1.; 4],
            params: [0; 4],
            extra: [0.; 4],
            light: [0., 0., 1., 1.],
            eye: [0., 0., 1., 0.],
            sky: [1.; 4],
            ground: [0.3, 0.3, 0.3, 1.],
            warning: [1., 0.4, 0.3, 1.],
            tools: [[1.; 4]; 16],
            clip: NO_CLIP,
            volume: [0., 0., f32::MAX, 0.01],
            volume_counts: [[0; 4]; 2],
            volume_points: [[0.; 4]; VOLUME_SLOTS],
            danger: [0.9, 0.33, 0.29, 1.],
        }
    }
}
/// The filled cross-section of an object cut by the section plane (world coordinates).
#[derive(Clone, Debug)]
pub struct CapDraw {
    /// Content key (object, transform and plane); the GPU mesh is cached under it.
    pub key: u64,
    pub mesh: Arc<mica_core::mesh::TriangleMesh>,
    pub color: [f32; 3],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 4],
}
/// Bed outline, exclusion polygons, asset files and theme the cached bed meshes were built from.
type BedKey = (
    Vec<[f32; 2]>,
    Vec<Vec<[f32; 2]>>,
    bed_assets::AssetFiles,
    bool,
);
struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

/// The rasterised bed texture on the GPU plus one uniform slot per plate (each plate draws
/// the quad with its own offset, and all writes land before the command buffer runs).
struct BedTexture {
    view: wgpu::TextureView,
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// Everything the renderer needs for one frame; hashed to skip redundant redraws.
pub struct Frame {
    pub camera: Camera,
    pub bed: Bed,
    pub plates: Vec<Plate>,
    pub objects: Vec<Object>,
    /// The plate each object belongs to (indexes `plates`); empty means plate 0 for all.
    pub plate_of: Vec<usize>,
    /// Translucent volumes (modifiers, negative and support volumes, range slabs, cut plane).
    pub volumes: Vec<VolumeDraw>,
    /// The primary selected object (gizmo), and every selected object (halo).
    pub selected: Option<usize>,
    pub selection: Vec<usize>,
    pub hovered: Option<usize>,
    pub tool_colors: Vec<[f32; 3]>,
    /// Each tool's alpha (missing = 1): objects whose own tool is translucent are drawn after
    /// the opaque scene, back to front, showing their nearest surface blended over it.
    pub tool_alphas: Vec<f32>,
    /// Painted facets to draw over the objects (Prepare only).
    pub overlays: Vec<crate::scene::paint::PaintOverlay>,
    pub preview: Option<Arc<Preview>>,
    pub view: View,
    pub size: [u32; 2],
    pub dark: bool,
    /// Section plane `[nx, ny, nz, d]` cutting objects (Prepare) or toolpaths (Preview).
    pub clip: Option<[f32; 4]>,
    /// Filled cut faces of the objects the plane crosses (Prepare).
    pub caps: Vec<CapDraw>,
    /// Sequential printing: clearance outlines on the bed and whether each collides.
    pub clearance: Vec<(Vec<[f32; 2]>, bool)>,
    /// Objects whose skirt or brim does not fit on the bed: tinted like objects outside it.
    pub warned: Vec<usize>,
    /// Surface texture previews (Prepare): objects or painted areas shaded with their texture.
    pub surface_textures: Vec<crate::textures::SurfacePreview>,
    /// Preview: the camera or a slider moved within `toolpaths::REFINE_DELAY`, so distant
    /// layers draw as ribbons (the current layer and near layers stay full tubes).
    pub moving: bool,
    /// Preview: the hover pick shared with the viewport (not part of the redraw check).
    pub pick: Option<toolpaths::SharedPick>,
    /// Plate tags lying on the ground in front of their plates (`viewport::plate_tags`).
    pub tags: Vec<TagDraw>,
}
/// The model matrix of the preview's toolpaths: their records are in bed (G-code)
/// coordinates and the preview shows the current plate, so they move to its world origin.
/// An opened G-code file shows on a single plate at the origin (`gcode_doc::single_plate`).
pub fn preview_model(plates: &[Plate]) -> Mat4 {
    Mat4::from_translation(crate::scene::plates::current_origin(plates).extend(0.))
}
impl Frame {
    pub fn aspect(&self) -> f32 {
        self.size[0] as f32 / self.size[1].max(1) as f32
    }
    /// Whether the camera looks at the bed from underneath (PrusaSlicer's `bottom`).
    pub fn looks_from_below(&self) -> bool {
        self.camera.direction().z < 0.
    }
    fn hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.size.hash(&mut h);
        self.moving.hash(&mut h);
        self.dark.hash(&mut h);
        self.selected.hash(&mut h);
        self.hovered.hash(&mut h);
        for v in self.camera.matrix(self.aspect()).to_cols_array() {
            v.to_bits().hash(&mut h);
        }
        self.camera.perspective.hash(&mut h);
        for p in &self.bed.polygon {
            p.map(f32::to_bits).hash(&mut h);
        }
        for z in &self.bed.exclusions {
            for p in z {
                p.map(f32::to_bits).hash(&mut h);
            }
        }
        self.bed.height.to_bits().hash(&mut h);
        self.bed.assets.hash(&mut h);
        for p in &self.plates {
            p.index.hash(&mut h);
            p.current.hash(&mut h);
            p.origin.to_array().map(f32::to_bits).hash(&mut h);
        }
        for c in &self.tool_colors {
            c.map(f32::to_bits).hash(&mut h);
        }
        for a in &self.tool_alphas {
            a.to_bits().hash(&mut h);
        }
        for o in &self.objects {
            o.id.hash(&mut h);
            // The mesh itself: a rebuilt text keeps its object's id.
            (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
            o.tools.0.hash(&mut h);
            for v in o.transform().to_cols_array() {
                v.to_bits().hash(&mut h);
            }
        }
        self.selection.hash(&mut h);
        for v in &self.volumes {
            v.mesh_identity().hash(&mut h);
            for x in v.transform.to_cols_array() {
                x.to_bits().hash(&mut h);
            }
            v.color.map(f32::to_bits).hash(&mut h);
            (v.cutout, v.selected, v.hovered).hash(&mut h);
        }
        for o in &self.overlays {
            o.object.hash(&mut h);
            o.key.hash(&mut h);
        }
        for s in &self.surface_textures {
            (s.object, s.key, s.area.as_ref().map(|a| a.0)).hash(&mut h);
        }
        if let Some(p) = &self.preview {
            p.job.path.hash(&mut h);
            self.view.range(p).hash(&mut h);
            self.view.gradient_range(p).map(f32::to_bits).hash(&mut h);
        }
        self.warned.hash(&mut h);
        for (outline, colliding) in &self.clearance {
            colliding.hash(&mut h);
            for p in outline {
                p.map(f32::to_bits).hash(&mut h);
            }
        }
        self.view.mask.hash(&mut h);
        self.view.tool_mask.hash(&mut h);
        (self.view.color as u32).hash(&mut h);
        self.clip.map(|c| c.map(f32::to_bits)).hash(&mut h);
        for t in &self.tags {
            t.hash_into(&mut h);
        }
        for c in &self.caps {
            c.key.hash(&mut h);
            c.color.map(f32::to_bits).hash(&mut h);
        }
        h.finish()
    }
}

struct Pool {
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
}
impl Pool {
    fn ensure(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, n: usize) {
        while self.slots.len() < n {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mica uniform slot"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mica uniform group"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.slots.push((buffer, group));
        }
    }
}

struct Targets {
    msaa: wgpu::TextureView,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    mask: wgpu::TextureView,
    /// Single-sample depth for the mask pass's outside-part channel.
    mask_depth: wgpu::TextureView,
    /// The hover pick's index buffer and its depth (single-sample).
    id: wgpu::Texture,
    id_view: wgpu::TextureView,
    id_depth: wgpu::TextureView,
    blit: wgpu::BindGroup,
    size: [u32; 2],
}

enum Kind {
    Background,
    /// Painted facets over an object (`scene::paint`), keyed by the overlay's content hash.
    Paint(u64),
    /// Drawn bed geometry (surface, exclusions, outline strips); `true` = no depth write.
    Bed(u64, bool),
    Lines(u32),
    /// A lit mesh: an object or the bed sheet model.
    Mesh(u64),
    /// A translucent lit mesh drawn after the objects (no depth write).
    Volume(u64),
    /// A translucent object (its filament has an alpha): the depth of its nearest surface
    /// first, then its colour painting (depth written), then its own colour blended where
    /// the paint is not, then its patterned paint. See `Renderer::draw`.
    Depth(u64),
    TranslucentPaint(u64),
    Translucent(u64),
    TranslucentMarks(u64),
    /// The bed texture quad for plate `n`; `true` = see-through (no depth write).
    Texture(usize, bool),
    /// A section cap (`Renderer::caps` key).
    Cap(u64),
    /// A surface texture preview (index into `Surfaces::draws`).
    Surface(usize),
    /// Every plate tag (`tags::Tags`), after everything that writes depth.
    Tags,
}
struct Item {
    kind: Kind,
    uniform: Uniform,
}

pub struct Renderer {
    uniform_layout: wgpu::BindGroupLayout,
    segment_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    background_pipeline: wgpu::RenderPipeline,
    bed_pipeline: wgpu::RenderPipeline,
    bed_see_through_pipeline: wgpu::RenderPipeline,
    texture_pipeline: wgpu::RenderPipeline,
    texture_see_through_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    volume_pipeline: wgpu::RenderPipeline,
    paint_pipeline: wgpu::RenderPipeline,
    /// Translucent objects: depth prepass, blended colour, colour paint (depth written) and
    /// patterned paint.
    depth_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
    paint_solid_pipeline: wgpu::RenderPipeline,
    paint_marks_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    outside_mask_pipeline: wgpu::RenderPipeline,
    lines_pipeline: wgpu::RenderPipeline,
    /// Toolpaths without and with the section plane, by `toolpaths::Detail` (full tube,
    /// tube, ribbon); then the index-buffer pipelines for tubes and ribbons.
    toolpath_pipelines: [[wgpu::RenderPipeline; 3]; 2],
    id_pipelines: [[wgpu::RenderPipeline; 2]; 2],
    chunk_layout: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
    surfaces: surface::Surfaces,
    tags: tags::Tags,
    pool: Pool,
    meshes: HashMap<u64, GpuMesh>,
    /// The scene meshes (objects, tracked volumes) the GPU copies under the same keys in
    /// `meshes` were made from: a mesh replaced under its id (a text edited, a simplify
    /// preview) is uploaded again. Holding them keeps their addresses from being reused.
    sources: HashMap<u64, Arc<mica_core::mesh::TriangleMesh>>,
    /// Paint overlays by content key: vertex buffer and vertex count.
    overlays: HashMap<u64, (wgpu::Buffer, u32)>,
    bed_key: Option<BedKey>,
    bed_texture: Option<BedTexture>,
    lines: Option<(wgpu::Buffer, u32)>,
    /// The shown preview's records on the GPU.
    resident: Option<toolpaths::Resident>,
    /// The index-buffer draws of the last still frame (its hash; whether the section plane
    /// cuts) not drawn yet: drawn once a hover asks for them.
    id_pending: Option<(u64, bool, Vec<toolpaths::Draw>)>,
    /// The frame whose moves the index buffer holds.
    id_key: Option<u64>,
    /// The pick read-back in flight, the last request read back (with its frame) and the
    /// answers so far.
    readback: Option<toolpaths::PickReadback>,
    picked: Option<(toolpaths::PickRequest, u64)>,
    pick_serial: u64,
    /// Diagnostics (`--bench … --legacy`): every move a full tube with tube shading, as before
    /// the levels of detail.
    legacy: bool,
    segment_uniform: wgpu::Buffer,
    segment_group: wgpu::BindGroup,
    layer_starts: wgpu::Buffer,
    layer_values: wgpu::Buffer,
    layer_key: Option<(std::path::PathBuf, bool)>,
    /// Section caps by content key.
    caps: HashMap<u64, GpuMesh>,
    blit_uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    texture_sampler: wgpu::Sampler,
    targets: Targets,
    pub error: Option<String>,
    /// Why the real bed sheet could not be shown (falls back to the drawn plate).
    pub bed_asset_error: Option<String>,
    last_frame: u64,
}

struct PipelineDesc<'a> {
    source: &'a str,
    vertex: &'a str,
    /// Pipeline-overridable constants of the shader.
    constants: &'a [(&'a str, f64)],
    layout: &'a wgpu::PipelineLayout,
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    format: wgpu::TextureFormat,
    samples: u32,
    depth: Option<(bool, wgpu::CompareFunction)>,
    blend: Option<wgpu::BlendState>,
    topology: wgpu::PrimitiveTopology,
    fragment: &'a str,
    writes: wgpu::ColorWrites,
}
fn pipeline(device: &wgpu::Device, d: PipelineDesc<'_>) -> wgpu::RenderPipeline {
    let source = format!("{}\n{}", include_str!("common.wgsl"), d.source);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Mica shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Mica pipeline"),
        layout: Some(d.layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(d.vertex),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: d.constants,
                ..Default::default()
            },
            buffers: d.buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(d.fragment),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: d.constants,
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: d.format,
                blend: d.blend,
                write_mask: d.writes,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: d.topology,
            ..Default::default()
        },
        depth_stencil: d.depth.map(|(write, compare)| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: d.samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}
fn uniform_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn storage_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn sampler_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}
fn targets(
    device: &wgpu::Device,
    blit_layout: &wgpu::BindGroupLayout,
    blit_uniform: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    size: [u32; 2],
) -> Targets {
    let desc = wgpu::TextureDescriptor {
        label: Some("Mica viewport"),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let view = device
        .create_texture(&desc)
        .create_view(&Default::default());
    let msaa = device
        .create_texture(&wgpu::TextureDescriptor {
            sample_count: SAMPLES,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            ..desc
        })
        .create_view(&Default::default());
    let depth = device
        .create_texture(&wgpu::TextureDescriptor {
            format: DEPTH_FORMAT,
            sample_count: SAMPLES,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            ..desc
        })
        .create_view(&Default::default());
    let mask = device
        .create_texture(&wgpu::TextureDescriptor {
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ..desc
        })
        .create_view(&Default::default());
    let mask_depth = device
        .create_texture(&wgpu::TextureDescriptor {
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            ..desc
        })
        .create_view(&Default::default());
    let id = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Mica pick index"),
        format: ID_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        ..desc
    });
    let id_view = id.create_view(&Default::default());
    let id_depth = device
        .create_texture(&wgpu::TextureDescriptor {
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            ..desc
        })
        .create_view(&Default::default());
    let blit = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Mica blit"),
        layout: blit_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: blit_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&mask),
            },
        ],
    });
    Targets {
        msaa,
        view,
        depth,
        mask,
        mask_depth,
        id,
        id_view,
        id_depth,
        blit,
        size,
    }
}
fn to4(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

/// Mip chain of an RGBA8 image by 2×2 box filtering (straight alpha).
pub fn mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(width, height, rgba.to_vec())];
    while let Some((w, h, data)) = levels.last()
        && (*w > 1 || *h > 1)
    {
        let (nw, nh) = ((*w).max(2) / 2, (*h).max(2) / 2);
        let mut next = Vec::with_capacity((nw * nh * 4) as usize);
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0u32; 4];
                let mut n = 0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = (x * 2 + dx).min(*w - 1);
                        let sy = (y * 2 + dy).min(*h - 1);
                        let i = ((sy * *w + sx) * 4) as usize;
                        for c in 0..4 {
                            sum[c] += data[i + c] as u32;
                        }
                        n += 1;
                    }
                }
                next.extend(sum.map(|s| (s / n) as u8));
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mica uniforms"),
            entries: &[uniform_layout_entry(0)],
        });
        let segment_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mica segment uniforms"),
            entries: &[
                uniform_layout_entry(0),
                storage_layout_entry(1),
                storage_layout_entry(2),
            ],
        });
        // A resident chunk of toolpaths: records, attributes, visible list, chunk uniform.
        let chunk_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mica preview chunk"),
            entries: &[
                storage_layout_entry(0),
                storage_layout_entry(1),
                storage_layout_entry(2),
                uniform_layout_entry(3),
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mica bed texture"),
            entries: &[
                uniform_layout_entry(0),
                texture_layout_entry(1),
                sampler_layout_entry(2),
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mica blit"),
            entries: &[
                uniform_layout_entry(0),
                texture_layout_entry(1),
                sampler_layout_entry(2),
                texture_layout_entry(3),
            ],
        });
        let layout = |l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Mica layout"),
                bind_group_layouts: &[Some(l)],
                immediate_size: 0,
            })
        };
        let uniform_pl = layout(&uniform_layout);
        let segment_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mica toolpath layout"),
            bind_group_layouts: &[Some(&segment_layout), Some(&chunk_layout)],
            immediate_size: 0,
        });
        let texture_pl = layout(&texture_layout);
        let blit_pl = layout(&blit_layout);
        let positions = [Some(wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3],
        })];
        let textured_attrs = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2];
        let textured = [Some(wgpu::VertexBufferLayout {
            array_stride: 20,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &textured_attrs,
        })];
        let line_attrs = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x4];
        let lines = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &line_attrs,
        })];
        let base = |source, buffers, depth, blend, topology, fragment| PipelineDesc {
            source,
            vertex: "vs",
            constants: &[],
            layout: &uniform_pl,
            buffers,
            format: FORMAT,
            samples: SAMPLES,
            depth,
            blend,
            topology,
            fragment,
            writes: wgpu::ColorWrites::ALL,
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let background_pipeline = pipeline(
            device,
            base(
                include_str!("background.wgsl"),
                &[],
                Some((false, wgpu::CompareFunction::Always)),
                None,
                tri,
                "fs",
            ),
        );
        let bed_pipeline = pipeline(
            device,
            base(
                include_str!("bed.wgsl"),
                &positions,
                Some((true, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs",
            ),
        );
        // Seen from below the plate is translucent and must not hide the objects behind it.
        let bed_see_through_pipeline = pipeline(
            device,
            base(
                include_str!("bed.wgsl"),
                &positions,
                Some((false, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs",
            ),
        );
        let texture_pipeline = pipeline(
            device,
            PipelineDesc {
                layout: &texture_pl,
                ..base(
                    include_str!("texture.wgsl"),
                    &textured,
                    Some((true, wgpu::CompareFunction::LessEqual)),
                    Some(wgpu::BlendState::ALPHA_BLENDING),
                    tri,
                    "fs",
                )
            },
        );
        let texture_see_through_pipeline = pipeline(
            device,
            PipelineDesc {
                layout: &texture_pl,
                ..base(
                    include_str!("texture.wgsl"),
                    &textured,
                    Some((false, wgpu::CompareFunction::LessEqual)),
                    Some(wgpu::BlendState::ALPHA_BLENDING),
                    tri,
                    "fs",
                )
            },
        );
        let mesh_pipeline = pipeline(
            device,
            base(
                include_str!("mesh.wgsl"),
                &positions,
                Some((true, wgpu::CompareFunction::LessEqual)),
                None,
                tri,
                "fs",
            ),
        );
        // Translucent volumes: blended over the objects without a depth test (x-ray), so a
        // modifier or a negative volume inside its object stays visible, as do overlapping
        // volumes; nothing is written to the depth buffer.
        let volume_pipeline = pipeline(
            device,
            base(
                include_str!("mesh.wgsl"),
                &positions,
                Some((false, wgpu::CompareFunction::Always)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs_volume",
            ),
        );
        let paint_attrs = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x4,2=>Uint32];
        let paint_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::scene::paint::PaintVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &paint_attrs,
        })];
        // Patterned paint blends over the model at its strength (alpha).
        let paint_pipeline = pipeline(
            device,
            base(
                include_str!("paint.wgsl"),
                &paint_layout,
                Some((false, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs",
            ),
        );
        // Translucent objects (a filament colour with an alpha, as OrcaSlicer draws them after
        // the opaque volumes with blending): each, back to front, first writes the depth of its
        // nearest surface only, so the colour pass blends exactly one layer of it over what is
        // behind (no inner or back faces, whatever the triangle order), and the opaque objects
        // behind it stay visible through it.
        let depth_pipeline = pipeline(
            device,
            PipelineDesc {
                writes: wgpu::ColorWrites::empty(),
                ..base(
                    include_str!("mesh.wgsl"),
                    &positions,
                    Some((true, wgpu::CompareFunction::LessEqual)),
                    None,
                    tri,
                    "fs_depth",
                )
            },
        );
        let translucent_pipeline = pipeline(
            device,
            base(
                include_str!("mesh.wgsl"),
                &positions,
                Some((false, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs",
            ),
        );
        // A translucent object's colour painting writes its (lifted) depth, so the object's
        // own colour is not blended in under it; patterned paint goes over both.
        let paint_solid_pipeline = pipeline(
            device,
            base(
                include_str!("paint.wgsl"),
                &paint_layout,
                Some((true, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs_solid",
            ),
        );
        let paint_marks_pipeline = pipeline(
            device,
            base(
                include_str!("paint.wgsl"),
                &paint_layout,
                Some((false, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                tri,
                "fs_marks",
            ),
        );
        let mask_pipeline = pipeline(
            device,
            PipelineDesc {
                format: MASK_FORMAT,
                samples: 1,
                writes: wgpu::ColorWrites::RED,
                ..base(
                    include_str!("mesh.wgsl"),
                    &positions,
                    Some((false, wgpu::CompareFunction::Always)),
                    None,
                    tri,
                    "fs_mask",
                )
            },
        );
        // The outside part: every object depth-tested, so only what is visible counts.
        let outside_mask_pipeline = pipeline(
            device,
            PipelineDesc {
                format: MASK_FORMAT,
                samples: 1,
                writes: wgpu::ColorWrites::GREEN,
                ..base(
                    include_str!("mesh.wgsl"),
                    &positions,
                    Some((true, wgpu::CompareFunction::LessEqual)),
                    None,
                    tri,
                    "fs_mask_outside",
                )
            },
        );
        let lines_pipeline = pipeline(
            device,
            base(
                include_str!("lines.wgsl"),
                &lines,
                Some((false, wgpu::CompareFunction::LessEqual)),
                Some(wgpu::BlendState::ALPHA_BLENDING),
                wgpu::PrimitiveTopology::LineList,
                "fs",
            ),
        );
        // Toolpaths: no vertex buffers, the shader fetches records through the visible list.
        let toolpath = |vertex, fragment, id: bool, sectioned: bool| {
            pipeline(
                device,
                PipelineDesc {
                    layout: &segment_pl,
                    vertex,
                    constants: &[("sectioned", if sectioned { 1. } else { 0. })],
                    format: if id { ID_FORMAT } else { FORMAT },
                    samples: if id { 1 } else { SAMPLES },
                    ..base(
                        include_str!("segments.wgsl"),
                        &[],
                        Some((true, wgpu::CompareFunction::LessEqual)),
                        None,
                        tri,
                        fragment,
                    )
                },
            )
        };
        let toolpath_pipelines = [false, true].map(|sectioned| {
            [
                toolpath("vs_full", "fs_full", false, sectioned),
                toolpath("vs", "fs", false, sectioned),
                toolpath("vs_ribbon", "fs_ribbon", false, sectioned),
            ]
        });
        let id_pipelines = [false, true].map(|sectioned| {
            [
                toolpath("vs_id", "fs_id", true, sectioned),
                toolpath("vs_ribbon_id", "fs_id", true, sectioned),
            ]
        });
        let blit_pipeline = pipeline(
            device,
            PipelineDesc {
                layout: &blit_pl,
                format,
                samples: 1,
                // The window keeps its own (opaque) alpha: the view's alpha is below 1 where
                // the bed grid blends, and Linux compositors show such pixels see-through.
                writes: wgpu::ColorWrites::COLOR,
                ..base(include_str!("blit.wgsl"), &[], None, None, tri, "fs")
            },
        );
        let uniform_buffer = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let storage = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let segment_uniform = uniform_buffer("Mica segment uniform");
        let layer_starts = storage("Mica layer starts", 16);
        let layer_values = storage("Mica layer values", 16);
        let segment_group = Self::segment_group(
            device,
            &segment_layout,
            &segment_uniform,
            [&layer_starts, &layer_values],
        );
        let blit_uniform = uniform_buffer("Mica blit uniform");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        let targets = targets(device, &blit_layout, &blit_uniform, &sampler, [1, 1]);
        let surfaces = surface::Surfaces::new(device, &texture_layout);
        let tags = tags::Tags::new(device, &texture_layout);
        Self {
            surfaces,
            tags,
            uniform_layout,
            segment_layout,
            texture_layout,
            blit_layout,
            background_pipeline,
            bed_pipeline,
            bed_see_through_pipeline,
            texture_pipeline,
            texture_see_through_pipeline,
            mesh_pipeline,
            volume_pipeline,
            paint_pipeline,
            depth_pipeline,
            translucent_pipeline,
            paint_solid_pipeline,
            paint_marks_pipeline,
            mask_pipeline,
            outside_mask_pipeline,
            lines_pipeline,
            toolpath_pipelines,
            id_pipelines,
            chunk_layout,
            blit_pipeline,
            pool: Pool { slots: vec![] },
            meshes: HashMap::new(),
            sources: HashMap::new(),
            overlays: HashMap::new(),
            bed_key: None,
            bed_texture: None,
            lines: None,
            resident: None,
            id_pending: None,
            id_key: None,
            readback: None,
            picked: None,
            pick_serial: 0,
            legacy: false,
            segment_uniform,
            segment_group,
            layer_starts,
            layer_values,
            layer_key: None,
            caps: HashMap::new(),
            blit_uniform,
            sampler,
            texture_sampler,
            targets,
            error: None,
            bed_asset_error: None,
            last_frame: 0,
        }
    }
    /// The segment pass's bindings: uniform, then the per-layer starts and values.
    fn segment_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        layers: [&wgpu::Buffer; 2],
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Mica segment group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: layers[0].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: layers[1].as_entire_binding(),
                },
            ],
        })
    }
    /// Uploads a scene mesh under `id` unless the GPU already holds this very mesh there.
    fn ensure_source(
        &mut self,
        device: &wgpu::Device,
        id: u64,
        mesh: &Arc<mica_core::mesh::TriangleMesh>,
    ) {
        if source_is_current(&self.sources, &self.meshes, id, mesh) {
            return;
        }
        let gpu = self.upload(device, &mesh.vertices, &mesh.faces);
        self.meshes.insert(id, gpu);
        self.sources.insert(id, mesh.clone());
    }

    fn upload_raw(&self, device: &wgpu::Device, vertices: &[u8], indices: &[[u32; 3]]) -> GpuMesh {
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mica vertices"),
            contents: vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let count = indices.len() as u32 * 3;
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mica triangles"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        GpuMesh {
            vertices,
            indices,
            count,
        }
    }
    fn upload(
        &self,
        device: &wgpu::Device,
        vertices: &[[f32; 3]],
        indices: &[[u32; 3]],
    ) -> GpuMesh {
        self.upload_raw(device, bytemuck::cast_slice(vertices), indices)
    }
    /// Uploads the bed texture with a full mip chain and one uniform slot per plate.
    fn upload_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &bed_assets::Texture,
    ) {
        let levels = mip_chain(texture.width, texture.height, &texture.rgba);
        let gpu = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Mica bed texture"),
            size: wgpu::Extent3d {
                width: texture.width,
                height: texture.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, h, data)) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &gpu,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(*h),
                },
                wgpu::Extent3d {
                    width: *w,
                    height: *h,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.bed_texture = Some(BedTexture {
            view: gpu.create_view(&Default::default()),
            slots: vec![],
        });
    }
    fn ensure_texture_slots(&mut self, device: &wgpu::Device, n: usize) {
        let Some(t) = &mut self.bed_texture else {
            return;
        };
        while t.slots.len() < n {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mica texture uniform"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mica texture group"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&t.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                    },
                ],
            });
            t.slots.push((buffer, group));
        }
    }
    /// Rebuilds the bed geometry (and loads the sheet model and texture) when the bed changed.
    fn ensure_bed(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame) {
        let key = (
            frame.bed.polygon.clone(),
            frame.bed.exclusions.clone(),
            frame.bed.assets.clone(),
            frame.dark,
        );
        if self.bed_key.as_ref() == Some(&key) {
            return;
        }
        let surface: Vec<_> = frame
            .bed
            .polygon
            .iter()
            .map(|p| [p[0], p[1], bed_assets::GROUND_Z])
            .collect();
        let mesh = self.upload(device, &surface, &bed::triangulate(&frame.bed.polygon));
        self.meshes.insert(BED_ID, mesh);
        let mut zv = vec![];
        let mut zi = vec![];
        for zone in &frame.bed.exclusions {
            let base = zv.len() as u32;
            zv.extend(zone.iter().map(|p| [p[0], p[1], 0.0]));
            zi.extend(
                bed::triangulate(zone)
                    .into_iter()
                    .map(|t| t.map(|i| i + base)),
            );
        }
        if zi.is_empty() {
            self.meshes.remove(&EXCLUSION_ID);
        } else {
            let mesh = self.upload(device, &zv, &zi);
            self.meshes.insert(EXCLUSION_ID, mesh);
        }
        let (v, f) = bed::outline_strip(&frame.bed.polygon, 0.7, 0.01);
        let mesh = self.upload(device, &v, &f);
        self.meshes.insert(OUTLINE_ID, mesh);
        let (v, f) = bed::outline_strip(&frame.bed.polygon, 1.2, 0.012);
        let mesh = self.upload(device, &v, &f);
        self.meshes.insert(OUTLINE_CURRENT_ID, mesh);
        // The printer's real bed: sheet model centred on the bed, texture over its bounds.
        self.meshes.remove(&BED_MODEL_ID);
        self.meshes.remove(&TEXTURE_ID);
        self.bed_texture = None;
        self.bed_asset_error = None;
        let (bmin, bmax) = (frame.bed.min.truncate(), frame.bed.max.truncate());
        if let Some(path) = &frame.bed.assets.model {
            match bed_assets::load_model(path, bmin, bmax) {
                Ok(mesh) => {
                    let gpu = self.upload(device, &mesh.vertices, &mesh.faces);
                    self.meshes.insert(BED_MODEL_ID, gpu);
                }
                Err(e) => self.bed_asset_error = Some(e),
            }
        }
        if let Some(path) = &frame.bed.assets.texture {
            match bed_assets::rasterize_svg(path, bed_assets::TEXTURE_SIDE) {
                Ok(texture) => {
                    // OrcaSlicer's textures are dark marks for a light plate: invert for dark.
                    let texture = texture.for_theme(frame.dark);
                    self.upload_texture(device, queue, &texture);
                    let (v, f) = bed_assets::texture_quad(&frame.bed.polygon, bmin, bmax);
                    let gpu = self.upload_raw(device, bytemuck::cast_slice(&v), &f);
                    self.meshes.insert(TEXTURE_ID, gpu);
                }
                Err(e) => self.bed_asset_error = Some(e),
            }
        }
        self.bed_key = Some(key);
    }
    fn build_lines(
        &mut self,
        device: &wgpu::Device,
        frame: &Frame,
        colors: &ViewportColors,
        textured: bool,
        from_below: bool,
    ) {
        let mut v: Vec<LineVertex> = vec![];
        let grid = frame.bed.grid();
        for plate in &frame.plates {
            let o = plate.origin.extend(0.);
            let mut c = colors.volume_outline;
            if !plate.current {
                c[3] *= 0.5;
            }
            // Major grid lines are real geometry (crisp under MSAA); the shader draws the minor
            // grid. The real bed sheet brings its own markings.
            if !textured {
                let alpha =
                    bed_assets::surface_alpha(from_below) * if plate.current { 1. } else { 0.6 };
                let gc = to4(colors.grid_major, alpha);
                for [a, b] in &grid.major {
                    for p in [a, b] {
                        v.push(LineVertex {
                            position: [p[0] + o.x, p[1] + o.y, 0.005],
                            color: gc,
                        });
                    }
                }
            }
            for [a, b] in frame.bed.volume_edges() {
                for p in [a + o, b + o] {
                    v.push(LineVertex {
                        position: p.to_array(),
                        color: c,
                    });
                }
            }
        }
        // Sequential printing: PrusaSlicer's / Orca's clearance contours, red when they collide.
        for (outline, colliding) in &frame.clearance {
            let color = if *colliding {
                to4(theme::c3_of(theme::DANGER), 1.)
            } else {
                colors.volume_outline
            };
            for i in 0..outline.len() {
                let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                for p in [a, b] {
                    v.push(LineVertex {
                        position: [p[0], p[1], 0.02],
                        color,
                    });
                }
            }
        }
        self.lines = (!v.is_empty()).then(|| {
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Mica lines"),
                    contents: bytemuck::cast_slice(&v),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                v.len() as u32,
            )
        });
    }
    fn ensure_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        preview: &Preview,
        color_print: bool,
    ) {
        let color_print = color_print && preview.color_print.is_some();
        let key = (preview.job.path.clone(), color_print);
        if self.layer_key.as_ref() == Some(&key) {
            return;
        }
        let starts: Vec<u32> = preview
            .layers
            .iter()
            .map(|l| l.start.min(u32::MAX as u64) as u32)
            .collect();
        // Colour print reads the packed colour segments, layer time the layer times.
        let values: Vec<f32> = match (&preview.color_print, color_print) {
            (Some(cp), true) => preview
                .layers
                .iter()
                .enumerate()
                .map(|(i, _)| cp.packed.get(i).copied().unwrap_or(0.))
                .collect(),
            _ => preview.layers.iter().map(|l| l.time_s).collect(),
        };
        let bytes = |n: usize| ((n * 4).max(16)) as u64;
        if self.layer_starts.size() < bytes(starts.len()) {
            self.layer_starts = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mica layer starts"),
                size: bytes(starts.len()),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.layer_values = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mica layer values"),
                size: bytes(values.len()),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.segment_group = Self::segment_group(
                device,
                &self.segment_layout,
                &self.segment_uniform,
                [&self.layer_starts, &self.layer_values],
            );
        }
        if !starts.is_empty() {
            queue.write_buffer(&self.layer_starts, 0, bytemuck::cast_slice(&starts));
            queue.write_buffer(&self.layer_values, 0, bytemuck::cast_slice(&values));
        }
        self.layer_key = Some(key);
    }
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame) {
        self.collect_pick(device, frame);
        let key = frame.hash();
        if key != self.last_frame {
            self.last_frame = key;
            self.draw(device, queue, frame, key);
        }
        self.serve_pick(device, queue, frame, key);
    }
    /// Diagnostics: draw every move as a full tube with tube shading (the look before the
    /// levels of detail), or go back to the levels of detail.
    pub fn set_legacy(&mut self, legacy: bool) {
        self.legacy = legacy;
        self.last_frame = 0;
    }
    fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame, key: u64) {
        // The index buffer no longer shows this frame.
        self.id_key = None;
        self.id_pending = None;
        if frame.size != self.targets.size {
            self.targets = targets(
                device,
                &self.blit_layout,
                &self.blit_uniform,
                &self.sampler,
                frame.size,
            );
        }
        let colors = theme::viewport_colors(frame.dark);
        self.ensure_bed(device, queue, frame);
        let textured = self.bed_texture.is_some() && self.meshes.contains_key(&TEXTURE_ID);
        let has_model = self.meshes.contains_key(&BED_MODEL_ID);
        let from_below = frame.looks_from_below();
        self.build_lines(device, frame, &colors, textured, from_below);
        self.ensure_texture_slots(device, frame.plates.len());
        self.meshes.retain(|id, _| {
            *id >= RESERVED_FROM
                || frame.objects.iter().any(|o| o.id == *id)
                || frame
                    .volumes
                    .iter()
                    .any(|v| v.mesh.as_ref().is_some_and(|(k, _)| k == id))
        });
        let meshes = &self.meshes;
        self.sources.retain(|id, _| meshes.contains_key(id));
        if !self.meshes.contains_key(&UNIT_CUBE_ID) {
            let cube = mica_core::mesh_edit::cuboid([1., 1., 1.]);
            let mesh = self.upload(device, &cube.vertices, &cube.faces);
            self.meshes.insert(UNIT_CUBE_ID, mesh);
        }
        self.overlays
            .retain(|key, _| frame.overlays.iter().any(|o| o.key == *key));
        self.caps
            .retain(|key, _| frame.caps.iter().any(|c| c.key == *key));
        let clip = frame.clip.unwrap_or(NO_CLIP);
        let camera = &frame.camera;
        let vp = camera.matrix(frame.aspect());
        // Key light: high, from the viewer's upper left, following the camera so the model never
        // turns into an unlit silhouette.
        let (right, up) = camera.basis();
        let light =
            (camera.direction() * 0.55 - right * 0.7 + up * 0.5 + Vec3::Z * 0.45).normalize();
        let eye = camera.eye();
        let mut tools = [[1.; 4]; 16];
        for (i, c) in frame.tool_colors.iter().take(16).enumerate() {
            tools[i] = to4(*c, 1.);
        }
        let (volume_counts, volume_points) = volume_shape(&frame.bed);
        let base = Uniform {
            vp: vp.to_cols_array_2d(),
            light: to4(light.to_array(), 1.),
            eye: to4(eye.to_array(), if camera.perspective { 1. } else { 0. }),
            sky: to4(colors.sky, 1.),
            ground: to4(colors.ground, 1.),
            warning: to4(colors.warning, 1.),
            tools,
            volume: [0., 0., frame.bed.height, OUTSIDE_TOLERANCE],
            volume_counts,
            volume_points,
            danger: to4(theme::c3_of(theme::DANGER), 1.),
            ..Default::default()
        };
        let mut items = vec![Item {
            kind: Kind::Background,
            uniform: Uniform {
                color: to4(colors.background, 1.),
                ..base
            },
        }];
        let mut texture_uniforms: Vec<(usize, Uniform)> = vec![];
        for (n, plate) in frame.plates.iter().enumerate() {
            let model = Mat4::from_translation(plate.origin.extend(0.)).to_cols_array_2d();
            let see_through = from_below || !plate.current;
            if has_model && !from_below {
                items.push(Item {
                    kind: Kind::Mesh(BED_MODEL_ID),
                    uniform: Uniform {
                        model,
                        color: to4(colors.bed_model, 1.),
                        ..base
                    },
                });
            }
            if textured {
                texture_uniforms.push((
                    n,
                    Uniform {
                        model,
                        color: to4(colors.texture_back_light, 1.),
                        warning: to4(colors.texture_back_dark, 1.),
                        params: [see_through as u32, 0, 4, 0],
                        extra: [plate.origin.x, plate.origin.y, 0., 0.],
                        ..base
                    },
                ));
                items.push(Item {
                    kind: Kind::Texture(n, see_through),
                    uniform: base,
                });
            } else {
                let bed_color = if plate.current {
                    colors.bed
                } else {
                    colors.bed_inactive
                };
                items.push(Item {
                    kind: Kind::Bed(BED_ID, from_below),
                    uniform: Uniform {
                        model,
                        color: to4(bed_color, bed_assets::surface_alpha(from_below)),
                        warning: to4(colors.grid_minor, 1.),
                        params: [0, 0, 1, 0],
                        extra: [plate.origin.x, plate.origin.y, 0., bed::MINOR_STEP],
                        ..base
                    },
                });
            }
            if self.meshes.contains_key(&EXCLUSION_ID) {
                items.push(Item {
                    kind: Kind::Bed(EXCLUSION_ID, from_below),
                    uniform: Uniform {
                        model,
                        color: colors.exclusion,
                        params: [0, 0, 2, 0],
                        extra: [plate.origin.x, plate.origin.y, 0., 0.],
                        ..base
                    },
                });
            }
            items.push(Item {
                kind: Kind::Bed(
                    if plate.current {
                        OUTLINE_CURRENT_ID
                    } else {
                        OUTLINE_ID
                    },
                    from_below,
                ),
                uniform: Uniform {
                    model,
                    color: to4(
                        if plate.current {
                            colors.bed_outline_current
                        } else {
                            colors.bed_outline
                        },
                        1.,
                    ),
                    params: [0, 0, 3, 0],
                    ..base
                },
            });
        }
        if let Some((_, count)) = &self.lines {
            items.push(Item {
                kind: Kind::Lines(*count),
                uniform: base,
            });
        }
        // Selection / hover mask entries: (GPU mesh, model matrix, value).
        let mut mask_items: Vec<(u64, Mat4, f32)> = vec![];
        // Outside-part mask entries, every object: (GPU mesh, its uniform).
        let mut outside_items: Vec<(u64, Uniform)> = vec![];
        let mut any_outside = false;
        // Translucent objects, drawn after everything opaque: (distance from the eye, object
        // id, its uniform).
        let mut translucent: Vec<(f32, u64, Uniform)> = vec![];
        if frame.preview.is_none() {
            for (i, o) in frame.objects.iter().enumerate() {
                self.ensure_source(device, o.id, &o.mesh);
                let transform = o.transform();
                let (bmin, bmax) = o.bounds();
                // The object against the printable volume of its own plate.
                let plate = frame.plate_of.get(i).copied().unwrap_or(0);
                // Preview draws the current plate alone: objects of the others stay hidden.
                if !frame.plates.is_empty() && !frame.plates.iter().any(|p| p.index == plate) {
                    continue;
                }
                let origin = frame
                    .plates
                    .iter()
                    .find(|p| p.index == plate)
                    .map_or(glam::Vec2::ZERO, |p| p.origin);
                let placement = crate::scene::plates::placement_on(
                    &frame.bed,
                    &frame.plates,
                    plate,
                    bmin,
                    bmax,
                );
                let tool = o.tools.0[0].unwrap_or(0);
                let color = frame
                    .tool_colors
                    .get(tool)
                    .copied()
                    .unwrap_or_else(|| crate::preview::legend::tool_color(tool));
                let mut flags = 0;
                if frame.hovered == Some(i) {
                    flags |= 1;
                }
                if placement != Placement::Inside {
                    // Only the fragments outside the volume are marked (`outside_volume`).
                    flags |= 32;
                    any_outside = true;
                } else if frame.warned.contains(&i) {
                    // Its skirt or brim does not fit: warning stripes over the whole object.
                    flags |= 2;
                }
                let alpha = frame.tool_alphas.get(tool).copied().unwrap_or(1.);
                let uniform = Uniform {
                    model: transform.to_cols_array_2d(),
                    color: to4(color, alpha.min(1.)),
                    params: [flags, 0, 0, 0],
                    clip,
                    volume: [origin.x, origin.y, frame.bed.height, OUTSIDE_TOLERANCE],
                    ..base
                };
                if alpha < 1. {
                    translucent.push(((((bmin + bmax) * 0.5) - eye).length(), o.id, uniform));
                } else {
                    items.push(Item {
                        kind: Kind::Mesh(o.id),
                        uniform,
                    });
                }
                outside_items.push((o.id, uniform));
                if frame.selected == Some(i) || frame.selection.contains(&i) {
                    mask_items.push((o.id, transform, 1.));
                } else if frame.hovered == Some(i) {
                    mask_items.push((o.id, transform, 0.5));
                }
            }
            // Texture previews right after the objects they shade (before anything
            // translucent); objects marked outside the volume keep their warning look.
            let object_uniforms: HashMap<u64, Uniform> = items
                .iter()
                .filter_map(|it| match it.kind {
                    Kind::Mesh(id) if it.uniform.params[0] & (2 | 32) == 0 => {
                        Some((id, it.uniform))
                    }
                    _ => None,
                })
                .collect();
            self.surfaces.prepare(
                device,
                queue,
                &self.texture_layout,
                &frame.surface_textures,
                |id| object_uniforms.get(&id).copied(),
            );
            for n in 0..self.surfaces.draws.len() {
                items.push(Item {
                    kind: Kind::Surface(n),
                    uniform: base,
                });
            }
            // Translucent volumes after the objects, selected ones into the mask too.
            for v in &frame.volumes {
                let id = match &v.mesh {
                    Some((id, mesh)) => {
                        if v.tracked {
                            self.ensure_source(device, *id, mesh);
                        } else if !self.meshes.contains_key(id) {
                            let gpu = self.upload(device, &mesh.vertices, &mesh.faces);
                            self.meshes.insert(*id, gpu);
                        }
                        *id
                    }
                    None => UNIT_CUBE_ID,
                };
                let mut flags = 0;
                if v.hovered {
                    flags |= 1;
                }
                if v.cutout {
                    flags |= 4;
                }
                if v.selected {
                    flags |= 8;
                }
                items.push(Item {
                    kind: Kind::Volume(id),
                    uniform: Uniform {
                        model: v.transform.to_cols_array_2d(),
                        color: v.color,
                        params: [flags, 0, 0, 0],
                        clip,
                        ..base
                    },
                });
                if v.selected {
                    mask_items.push((id, v.transform, 1.));
                } else if v.hovered {
                    // A hovered part of the selected object: the selection outline, lighter,
                    // so a modifier hidden inside the body can be found.
                    mask_items.push((id, v.transform, 0.5));
                }
            }
            // Painted facets after every object, lifted a little towards the eye (a
            // translucent object's with the object itself, below).
            let mut translucent_paint: HashMap<u64, Vec<(u64, Uniform)>> = HashMap::new();
            for overlay in &frame.overlays {
                let Some(o) = frame.objects.iter().find(|o| o.id == overlay.object) else {
                    continue;
                };
                if overlay.vertices.is_empty() {
                    continue;
                }
                self.overlays.entry(overlay.key).or_insert_with(|| {
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Mica paint overlay"),
                        contents: bytemuck::cast_slice(&overlay.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    (buffer, overlay.vertices.len() as u32)
                });
                let lift = ((eye - o.pivot_world()).length() * 0.0015).clamp(0.05, 0.6);
                let uniform = Uniform {
                    model: o.transform().to_cols_array_2d(),
                    extra: [lift, 0., 0., 0.],
                    clip,
                    ..base
                };
                if translucent.iter().any(|(_, id, _)| *id == o.id) {
                    translucent_paint
                        .entry(o.id)
                        .or_default()
                        .push((overlay.key, uniform));
                } else {
                    items.push(Item {
                        kind: Kind::Paint(overlay.key),
                        uniform,
                    });
                }
            }
            // Section caps: the cut faces, flat in the object's colour (never clipped).
            for cap in &frame.caps {
                if cap.mesh.faces.is_empty() {
                    continue;
                }
                if !self.caps.contains_key(&cap.key) {
                    let gpu = self.upload(device, &cap.mesh.vertices, &cap.mesh.faces);
                    self.caps.insert(cap.key, gpu);
                }
                items.push(Item {
                    kind: Kind::Cap(cap.key),
                    uniform: Uniform {
                        color: to4(cap.color, 1.),
                        params: [16, 0, 0, 0],
                        ..base
                    },
                });
            }
            // Translucent objects last, farthest first (as OrcaSlicer sorts its transparent
            // volumes), each over everything drawn before it.
            translucent.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, id, uniform) in translucent {
                let paint = translucent_paint.remove(&id).unwrap_or_default();
                items.push(Item {
                    kind: Kind::Depth(id),
                    uniform,
                });
                for (key, u) in &paint {
                    items.push(Item {
                        kind: Kind::TranslucentPaint(*key),
                        uniform: *u,
                    });
                }
                items.push(Item {
                    kind: Kind::Translucent(id),
                    uniform,
                });
                for (key, u) in paint {
                    items.push(Item {
                        kind: Kind::TranslucentMarks(key),
                        uniform: u,
                    });
                }
            }
        }
        // Plate tags last: depth-tested against everything above, writing none themselves.
        // From below the bed they would read mirrored through the plates: none.
        if !frame.tags.is_empty() && !from_below {
            self.tags
                .prepare(device, queue, &self.texture_layout, &frame.tags, &base);
            items.push(Item {
                kind: Kind::Tags,
                uniform: base,
            });
        }
        if !any_outside {
            outside_items.clear();
        }
        let outside_from = items.len() + mask_items.len();
        let total = outside_from + outside_items.len();
        self.pool.ensure(device, &self.uniform_layout, total);
        for (i, item) in items.iter().enumerate() {
            queue.write_buffer(&self.pool.slots[i].0, 0, bytemuck::bytes_of(&item.uniform));
        }
        if let Some(t) = &self.bed_texture {
            for (n, u) in &texture_uniforms {
                if let Some((buffer, _)) = t.slots.get(*n) {
                    queue.write_buffer(buffer, 0, bytemuck::bytes_of(u));
                }
            }
        }
        for (j, (_, model, value)) in mask_items.iter().enumerate() {
            let u = Uniform {
                model: model.to_cols_array_2d(),
                color: [*value, 0., 0., 1.],
                clip,
                ..base
            };
            queue.write_buffer(
                &self.pool.slots[items.len() + j].0,
                0,
                bytemuck::bytes_of(&u),
            );
        }
        for (j, (_, u)) in outside_items.iter().enumerate() {
            queue.write_buffer(
                &self.pool.slots[outside_from + j].0,
                0,
                bytemuck::bytes_of(u),
            );
        }
        queue.write_buffer(
            &self.blit_uniform,
            0,
            bytemuck::bytes_of(&Uniform {
                color: to4(colors.selection, 1.),
                warning: to4(colors.hover, 1.),
                extra: [
                    1. / frame.size[0].max(1) as f32,
                    1. / frame.size[1].max(1) as f32,
                    0.,
                    0.,
                ],
                ..base
            }),
        );
        let toolpaths = match &frame.preview {
            Some(preview) => self.prepare_toolpaths(device, queue, frame, preview, &base, key),
            None => {
                self.resident = None;
                vec![]
            }
        };
        if frame.preview.is_some() && !frame.moving && self.id_pending.is_none() {
            // Nothing to draw (or the records could not be read): a hover is answered with
            // "no move" rather than left waiting.
            self.id_pending = Some((key, frame.clip.is_some(), vec![]));
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = self.pass(&mut encoder, true);
            for (i, item) in items.iter().enumerate() {
                let group = &self.pool.slots[i].1;
                match item.kind {
                    Kind::Background => {
                        pass.set_pipeline(&self.background_pipeline);
                        pass.set_bind_group(0, group, &[]);
                        pass.draw(0..3, 0..1);
                    }
                    Kind::Bed(id, see_through) => {
                        pass.set_pipeline(if see_through {
                            &self.bed_see_through_pipeline
                        } else {
                            &self.bed_pipeline
                        });
                        Self::draw_mesh(&mut pass, group, &self.meshes[&id]);
                    }
                    Kind::Texture(n, see_through) => {
                        if let (Some(t), Some(mesh)) =
                            (&self.bed_texture, self.meshes.get(&TEXTURE_ID))
                            && let Some((_, group)) = t.slots.get(n)
                        {
                            pass.set_pipeline(if see_through {
                                &self.texture_see_through_pipeline
                            } else {
                                &self.texture_pipeline
                            });
                            Self::draw_mesh(&mut pass, group, mesh);
                        }
                    }
                    Kind::Lines(count) => {
                        if let Some((buffer, _)) = &self.lines {
                            pass.set_pipeline(&self.lines_pipeline);
                            pass.set_bind_group(0, group, &[]);
                            pass.set_vertex_buffer(0, buffer.slice(..));
                            pass.draw(0..count, 0..1);
                        }
                    }
                    Kind::Mesh(id) => {
                        pass.set_pipeline(&self.mesh_pipeline);
                        Self::draw_mesh(&mut pass, group, &self.meshes[&id]);
                    }
                    Kind::Volume(id) => {
                        if let Some(mesh) = self.meshes.get(&id) {
                            pass.set_pipeline(&self.volume_pipeline);
                            Self::draw_mesh(&mut pass, group, mesh);
                        }
                    }
                    Kind::Paint(key)
                    | Kind::TranslucentPaint(key)
                    | Kind::TranslucentMarks(key) => {
                        if let Some((buffer, count)) = self.overlays.get(&key) {
                            pass.set_pipeline(match item.kind {
                                Kind::TranslucentPaint(_) => &self.paint_solid_pipeline,
                                Kind::TranslucentMarks(_) => &self.paint_marks_pipeline,
                                _ => &self.paint_pipeline,
                            });
                            pass.set_bind_group(0, group, &[]);
                            pass.set_vertex_buffer(0, buffer.slice(..));
                            pass.draw(0..*count, 0..1);
                        }
                    }
                    Kind::Depth(id) | Kind::Translucent(id) => {
                        pass.set_pipeline(if matches!(item.kind, Kind::Depth(_)) {
                            &self.depth_pipeline
                        } else {
                            &self.translucent_pipeline
                        });
                        Self::draw_mesh(&mut pass, group, &self.meshes[&id]);
                    }
                    Kind::Cap(key) => {
                        if let Some(mesh) = self.caps.get(&key) {
                            pass.set_pipeline(&self.mesh_pipeline);
                            Self::draw_mesh(&mut pass, group, mesh);
                        }
                    }
                    Kind::Surface(n) => {
                        let object = self.surfaces.draws.get(n).map(|d| d.2);
                        let mesh = object
                            .and_then(|id| self.meshes.get(&id))
                            .map(|m| (&m.vertices, &m.indices, m.count));
                        self.surfaces.draw(&mut pass, n, mesh);
                    }
                    Kind::Tags => self.tags.draw(&mut pass),
                }
            }
            self.draw_toolpaths(&mut pass, &toolpaths, false, frame.clip.is_some());
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Mica mask pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.mask,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.mask_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.mask_pipeline);
            for (j, (id, _, _)) in mask_items.iter().enumerate() {
                if let Some(mesh) = self.meshes.get(id) {
                    Self::draw_mesh(&mut pass, &self.pool.slots[items.len() + j].1, mesh);
                }
            }
            // The outside part's channel: only while some object reaches outside.
            pass.set_pipeline(&self.outside_mask_pipeline);
            for (j, (id, _)) in outside_items.iter().enumerate() {
                if let Some(mesh) = self.meshes.get(id) {
                    Self::draw_mesh(&mut pass, &self.pool.slots[outside_from + j].1, mesh);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }
    /// Uploads the preview's records when they are new, rebuilds the visible lists when the
    /// legend changed, writes the toolpath uniform and returns this frame's draws; a still
    /// frame also leaves its index-buffer draws for the hover pick.
    fn prepare_toolpaths(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
        preview: &Preview,
        base: &Uniform,
        key: u64,
    ) -> Vec<toolpaths::Draw> {
        use toolpaths::{Detail, NEAR_LAYER_PX, Run};
        self.ensure_layers(
            device,
            queue,
            preview,
            frame.view.color == ColorMode::ColorPrint,
        );
        let held = (preview.job.path.clone(), preview.count);
        if self.resident.as_ref().is_none_or(|r| r.key != held) {
            // Free the previous preview first.
            self.resident = None;
            match toolpaths::Resident::load(device, &self.chunk_layout, preview) {
                Ok(r) => {
                    self.resident = Some(r);
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                    return vec![];
                }
            }
        }
        let Some(resident) = self.resident.as_mut() else {
            return vec![];
        };
        resident.ensure_visible(queue, frame.view.mask, frame.view.tool_mask);
        let resident = &*resident;
        let range = frame.view.range(preview);
        let [lo, hi] = frame.view.shader_range(preview);
        let model = preview_model(&frame.plates);
        let scale =
            toolpaths::Scale::new(&frame.camera, model.w_axis.truncate(), frame.size[1] as f32);
        let mut uniform = Uniform {
            params: [
                frame.view.mask | (frame.view.tool_mask << 16),
                frame.view.color as u32,
                0,
                0,
            ],
            extra: [lo, hi, scale.pixels_per_mm(), 0.],
            clip: frame.clip.unwrap_or(NO_CLIP),
            // Toolpaths are in bed coordinates: move them onto the shown plate's bed.
            model: model.to_cols_array_2d(),
            ..*base
        };
        if frame.view.color == ColorMode::ColorPrint
            && let Some(cp) = &preview.color_print
        {
            uniform.tools = cp.palette(&frame.tool_colors);
        } else if frame.view.color == ColorMode::ColorPrint {
            // No colour changes: the filament colours.
            uniform.params[1] = ColorMode::Tool as u32;
        }
        queue.write_buffer(&self.segment_uniform, 0, bytemuck::bytes_of(&uniform));
        let near = |i: usize| {
            resident
                .extents
                .get(i)
                .is_some_and(|e| scale.layer_px(e) >= NEAR_LAYER_PX)
        };
        let top = frame.view.high.min(preview.layers.len().saturating_sub(1));
        // The section view shows every layer at full detail.
        let full = frame.clip.is_some();
        let legacy = self.legacy;
        let plan = |moving| {
            if legacy {
                vec![Run {
                    records: range.clone(),
                    detail: Detail::Full,
                }]
            } else {
                toolpaths::plan(&preview.layers, range.clone(), top, near, moving, full)
            }
        };
        let draws = resident.draws(&plan(frame.moving));
        // The pick's index buffer draws distant layers as ribbons, which cover the same
        // pixels as their tubes, at a fraction of the cost.
        let pending = (!frame.moving).then(|| (key, full, resident.draws(&plan(true))));
        self.id_pending = pending;
        draws
    }
    fn draw_toolpaths(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        draws: &[toolpaths::Draw],
        id: bool,
        sectioned: bool,
    ) {
        use toolpaths::Detail;
        let Some(resident) = &self.resident else {
            return;
        };
        for d in draws {
            let pipeline = if id {
                &self.id_pipelines[sectioned as usize][(d.detail == Detail::Ribbon) as usize]
            } else {
                &self.toolpath_pipelines[sectioned as usize][match d.detail {
                    Detail::Full => 0,
                    Detail::Tube => 1,
                    Detail::Ribbon => 2,
                }]
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.segment_group, &[]);
            pass.set_bind_group(1, resident.group(d.chunk), &[]);
            pass.draw(0..d.detail.vertices(), d.instances.clone());
        }
    }
    /// Hands a finished pick read-back's answer to the viewport (when the index buffer it
    /// read still shows the frame).
    fn collect_pick(&mut self, device: &wgpu::Device, frame: &Frame) {
        let Some(readback) = &self.readback else {
            return;
        };
        if readback.state.load(std::sync::atomic::Ordering::Acquire) == 0 {
            // Never waits: runs the mapping callback if the copy is done.
            let _ = device.poll(wgpu::PollType::Poll);
        }
        let Some(answer) = readback.finish() else {
            return;
        };
        let request = readback.request;
        let current = self.id_key == Some(readback.key);
        self.readback = None;
        if let (Some(record), Some(shared), true) = (answer, &frame.pick, current)
            && let Ok(mut pick) = shared.lock()
        {
            self.pick_serial += 1;
            pick.result = Some(toolpaths::PickResult {
                request,
                record,
                serial: self.pick_serial,
            });
        }
    }
    /// Answers the viewport's hover request: draws the index buffer of a still frame when it
    /// is first needed, then reads the texels around the pointer back (asynchronously).
    fn serve_pick(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame, key: u64) {
        let Some(shared) = &frame.pick else {
            return;
        };
        let Some(request) = shared.lock().ok().and_then(|p| p.request) else {
            return;
        };
        if self.id_key != Some(key) {
            match self.id_pending.take() {
                Some((k, sectioned, draws)) if k == key => {
                    self.draw_ids(device, queue, &draws, sectioned);
                    self.id_key = Some(key);
                }
                // Moving: no index buffer until the view is still.
                other => {
                    self.id_pending = other;
                    return;
                }
            }
        }
        if self.readback.is_some() || self.picked == Some((request, key)) {
            return;
        }
        self.picked = Some((request, key));
        self.readback = toolpaths::PickReadback::start(
            device,
            queue,
            &self.targets.id,
            self.targets.size,
            request,
            key,
        );
        if self.readback.is_none()
            && let Ok(mut pick) = shared.lock()
        {
            // Outside the view.
            self.pick_serial += 1;
            pick.result = Some(toolpaths::PickResult {
                request,
                record: None,
                serial: self.pick_serial,
            });
        }
    }
    /// The hover pick's index buffer: the visible moves' record indices, depth-tested.
    fn draw_ids(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draws: &[toolpaths::Draw],
        sectioned: bool,
    ) {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Mica pick pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.id_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.id_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            self.draw_toolpaths(&mut pass, draws, true, sectioned);
        }
        queue.submit([encoder.finish()]);
    }
    fn draw_mesh(pass: &mut wgpu::RenderPass<'_>, group: &wgpu::BindGroup, mesh: &GpuMesh) {
        pass.set_bind_group(0, group, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.count, 0, 0..1);
    }
    fn pass<'a>(
        &'a self,
        encoder: &'a mut wgpu::CommandEncoder,
        clear: bool,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Mica 3D pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.targets.msaa,
                depth_slice: None,
                resolve_target: Some(&self.targets.view),
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(1.)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        })
    }
}
impl CallbackTrait for Frame {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &ScreenDescriptor,
        _: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // Installed at start-up; a frame without it draws nothing rather than panicking.
        if let Some(r) = resources.get_mut::<Renderer>() {
            r.render(device, queue, self);
        }
        vec![]
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(r) = resources.get::<Renderer>() else {
            return;
        };
        pass.set_pipeline(&r.blit_pipeline);
        pass.set_bind_group(0, &r.targets.blit, &[]);
        pass.draw(0..3, 0..1);
    }
}
/// Default gradient-range hints per colour mode, used when the preview has no statistics.
pub fn default_range(mode: ColorMode) -> [f32; 2] {
    match mode {
        ColorMode::Speed => [0., 300.],
        ColorMode::Fan => [0., 100.],
        ColorMode::Width => [0.2, 0.8],
        ColorMode::Height => [0.05, 0.4],
        ColorMode::LayerTime | ColorMode::LayerTimeLog => [0., 60.],
        ColorMode::ActualSpeed => [0., 300.],
        ColorMode::VolumetricFlow => [0., 20.],
        ColorMode::Temperature => [180., 280.],
        ColorMode::Acceleration => [0., 10000.],
        _ => [0., 1.],
    }
}
#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    /// A mesh replaced under the same id (a text rebuilt as it is typed) is uploaded again
    /// and redraws the view; the same mesh is not uploaded twice.
    #[test]
    fn a_mesh_replaced_under_its_id_is_uploaded_again() {
        let a = Arc::new(mica_core::mesh_edit::cuboid([1., 1., 1.]));
        let b = Arc::new(mica_core::mesh_edit::cuboid([2., 1., 1.]));
        let mut sources = HashMap::new();
        let mut meshes: HashMap<u64, ()> = HashMap::new();
        assert!(!super::source_is_current(&sources, &meshes, 7, &a));
        sources.insert(7, a.clone());
        meshes.insert(7, ());
        assert!(super::source_is_current(&sources, &meshes, 7, &a));
        assert!(!super::source_is_current(&sources, &meshes, 7, &b));
        // A GPU copy dropped (the volume left the view a while) is made again.
        meshes.clear();
        assert!(!super::source_is_current(&sources, &meshes, 7, &a));
        let draw = |mesh: &Arc<mica_core::mesh::TriangleMesh>, tracked| super::VolumeDraw {
            mesh: Some((7, mesh.clone())),
            tracked,
            transform: glam::Mat4::IDENTITY,
            color: [1.; 4],
            cutout: false,
            selected: false,
            hovered: false,
        };
        assert_ne!(
            draw(&a, true).mesh_identity(),
            draw(&b, true).mesh_identity()
        );
        // Content-named keys redraw only when the key changes.
        assert_eq!(
            draw(&a, false).mesh_identity(),
            draw(&b, false).mesh_identity()
        );
    }

    #[test]
    fn shaders_validate_without_adapter() {
        for source in [
            include_str!("mesh.wgsl"),
            include_str!("bed.wgsl"),
            include_str!("texture.wgsl"),
            include_str!("background.wgsl"),
            include_str!("lines.wgsl"),
            include_str!("segments.wgsl"),
            include_str!("blit.wgsl"),
            include_str!("paint.wgsl"),
            include_str!("surface_texture.wgsl"),
        ] {
            let source = format!("{}\n{}", include_str!("common.wgsl"), source);
            let module = egui_wgpu::wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            egui_wgpu::wgpu::naga::valid::Validator::new(
                egui_wgpu::wgpu::naga::valid::ValidationFlags::all(),
                egui_wgpu::wgpu::naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap();
        }
    }
    #[test]
    fn uniform_layout_is_std140_sized() {
        assert_eq!(std::mem::size_of::<super::Uniform>() % 16, 0);
        assert_eq!(
            std::mem::size_of::<super::Uniform>(),
            64 * 2 + 16 * 8 + 256 + 16 + 16 + 32 + 16 * super::VOLUME_SLOTS + 16
        );
    }
    #[test]
    fn mip_chain_halves_down_to_one_pixel() {
        let rgba: Vec<u8> = (0..4 * 2 * 4).map(|i| (i % 7) as u8 * 30).collect();
        let levels = super::mip_chain(4, 2, &rgba);
        assert_eq!(
            levels.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>(),
            [(4, 2), (2, 1), (1, 1)]
        );
        assert_eq!(levels[1].2.len(), 2 * 4);
        // Constant images stay constant.
        let flat = super::mip_chain(8, 8, &[200u8; 8 * 8 * 4]);
        assert!(flat.iter().all(|(_, _, d)| d.iter().all(|&v| v == 200)));
    }
}
