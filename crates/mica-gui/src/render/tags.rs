// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Plate tags on the GPU: each tag is a textured quad lying on the ground next to its plate
//! (laid out by `viewport::plate_tags`), so it foreshortens with the plates. It is drawn after
//! the objects, depth-tested without writing depth: an object between the camera and a tag
//! hides it, a tag never covers an object. The texture holds coverage masks (`tag.wgsl`) and
//! is shared by key, so only a changed text uploads a new one.
use super::{DEPTH_FORMAT, FORMAT, SAMPLES, Uniform, mip_chain};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::{Mat4, Vec3};
use std::{collections::HashMap, sync::Arc};

/// Height of the tag quads above the bed's ground plane (mm): over the bed texture
/// (`bed_assets::GROUND_Z`) and the sheet model's rim, under any object.
const TAG_Z: f32 = 0.05;

/// A tag's coverage masks, top row first: r = chip, g = text, b = rim, a unused.
#[derive(Debug)]
pub struct TagImage {
    /// Content key (text, size): equal keys are equal images.
    pub key: u64,
    pub width: u32,
    pub height: u32,
    pub texels: Vec<u8>,
}

/// One tag to draw: where it lies on the ground (world mm, `[x0, y0, x1, y1]`, the image's
/// top row at `y1`, the far side from the front) and its colours (linear RGBA).
#[derive(Clone, Debug)]
pub struct TagDraw {
    pub rect: [f32; 4],
    pub image: Arc<TagImage>,
    pub fill: [f32; 4],
    pub text: [f32; 4],
    pub rim: [f32; 4],
}
impl TagDraw {
    pub(super) fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.image.key.hash(h);
        for v in self
            .rect
            .iter()
            .chain(&self.fill)
            .chain(&self.text)
            .chain(&self.rim)
        {
            v.to_bits().hash(h);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

pub(super) struct Tags {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// The unit quad (0..1 in x and y), scaled onto each tag by its model matrix.
    quad: wgpu::Buffer,
    textures: HashMap<u64, wgpu::TextureView>,
    /// Per-draw uniform buffers and their bind groups (rebuilt when the image changes).
    slots: Vec<(wgpu::Buffer, Option<(u64, wgpu::BindGroup)>)>,
    /// This frame's tag count (slots `0..count`).
    count: usize,
}

impl Tags {
    pub(super) fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let source = format!(
            "{}\n{}",
            include_str!("common.wgsl"),
            include_str!("tag.wgsl")
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mica plate tag shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mica plate tag layout"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3, 1=>Float32x2];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Mica plate tags"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        // Two triangles, counter-clockwise from above; v = 0 is the image's top row (far side).
        let corner = |x: f32, y: f32| Vertex {
            position: [x, y, 0.],
            uv: [x, 1. - y],
        };
        let vertices = [
            corner(0., 0.),
            corner(1., 0.),
            corner(1., 1.),
            corner(0., 0.),
            corner(1., 1.),
            corner(0., 1.),
        ];
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mica plate tag quad"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            pipeline,
            sampler,
            quad,
            textures: HashMap::new(),
            slots: Vec::new(),
            count: 0,
        }
    }

    fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &TagImage,
    ) -> wgpu::TextureView {
        if let Some(v) = self.textures.get(&image.key) {
            return v.clone();
        }
        let levels = mip_chain(image.width, image.height, &image.texels);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Mica plate tag"),
            size: wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (w, h, data)) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
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
        let view = texture.create_view(&Default::default());
        self.textures.insert(image.key, view.clone());
        view
    }

    /// Uploads the textures this frame's tags need (dropping the others) and writes their
    /// uniforms on top of `base` (camera, lights).
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        tags: &[TagDraw],
        base: &Uniform,
    ) {
        self.textures
            .retain(|k, _| tags.iter().any(|t| t.image.key == *k));
        self.count = 0;
        for (n, t) in tags.iter().enumerate() {
            let view = self.texture(device, queue, &t.image);
            let [x0, y0, x1, y1] = t.rect;
            let model = Mat4::from_translation(Vec3::new(x0, y0, TAG_Z))
                * Mat4::from_scale(Vec3::new(x1 - x0, y1 - y0, 1.));
            let u = Uniform {
                model: model.to_cols_array_2d(),
                color: t.fill,
                warning: t.text,
                extra: t.rim,
                ..*base
            };
            if self.slots.len() <= n {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Mica plate tag uniform"),
                    size: std::mem::size_of::<Uniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.slots.push((buffer, None));
            }
            let (buffer, group) = &mut self.slots[n];
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&u));
            if group.as_ref().is_none_or(|(k, _)| *k != t.image.key) {
                let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Mica plate tag group"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                *group = Some((t.image.key, g));
            }
        }
        self.count = tags.len();
    }

    /// Draws this frame's tags (after the objects: their depth hides what lies behind them).
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        for (_, group) in &self.slots[..self.count] {
            if let Some((_, group)) = group {
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }
}
