// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The surface texture preview in Prepare: objects (or their texture-painted areas) drawn again
//! with `surface_texture.wgsl`, which shades the texture's relief with the same projection the
//! slicer uses. Height maps live on the GPU as single-channel textures, shared by key.
use super::{DEPTH_FORMAT, FORMAT, SAMPLES, Uniform};
use crate::textures::SurfacePreview;
use egui_wgpu::wgpu::{self, util::DeviceExt};
use std::collections::HashMap;

pub(super) struct Surfaces {
    pipeline: wgpu::RenderPipeline,
    /// Painted areas: pulled towards the eye so they win over the object underneath.
    area_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    maps: HashMap<String, wgpu::TextureView>,
    areas: HashMap<u64, (wgpu::Buffer, u32)>,
    /// Per-draw uniform buffers and their bind groups (rebuilt when the map changes).
    slots: Vec<(wgpu::Buffer, Option<(String, wgpu::BindGroup)>)>,
    /// This frame's draws: slot index, painted-area key (None: the object's mesh), object id.
    pub(super) draws: Vec<(usize, Option<u64>, u64)>,
}

fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    area: bool,
) -> wgpu::RenderPipeline {
    let positions = [Some(wgpu::VertexBufferLayout {
        array_stride: 12,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0=>Float32x3],
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Mica surface texture"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &positions,
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(!area),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: if area {
                wgpu::DepthBiasState {
                    constant: -8,
                    slope_scale: -1.,
                    clamp: 0.,
                }
            } else {
                Default::default()
            },
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLES,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

impl Surfaces {
    pub(super) fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let source = format!(
            "{}\n{}",
            include_str!("common.wgsl"),
            include_str!("surface_texture.wgsl")
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mica surface texture shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mica surface texture layout"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        Self {
            pipeline: pipeline(device, &pl, &module, false),
            area_pipeline: pipeline(device, &pl, &module, true),
            sampler,
            maps: HashMap::new(),
            areas: HashMap::new(),
            slots: Vec::new(),
            draws: Vec::new(),
        }
    }

    fn map_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        p: &SurfacePreview,
    ) -> wgpu::TextureView {
        if let Some(v) = self.maps.get(&p.map_key) {
            return v.clone();
        }
        let (w, h) = (p.map.width() as u32, p.map.height() as u32);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Mica height map"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &p.map.to_gray8(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&Default::default());
        if self.maps.len() > 24 {
            self.maps.clear();
        }
        self.maps.insert(p.map_key.clone(), view.clone());
        view
    }

    /// Uploads what this frame's previews need and writes their uniforms; `uniform_of` gives
    /// each object's base uniform (model matrix, clip, flags), `None` hides its preview.
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        previews: &[SurfacePreview],
        uniform_of: impl Fn(u64) -> Option<Uniform>,
    ) {
        self.draws.clear();
        self.areas.retain(|k, _| {
            previews
                .iter()
                .any(|p| p.area.as_ref().is_some_and(|a| a.0 == *k))
        });
        for p in previews {
            let Some(mut u) = uniform_of(p.object) else {
                continue;
            };
            u.tools[..p.params.len()].copy_from_slice(&p.params);
            let view = self.map_view(device, queue, p);
            if let Some((key, triangles)) = &p.area {
                if triangles.is_empty() {
                    continue;
                }
                self.areas.entry(*key).or_insert_with(|| {
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Mica texture paint area"),
                        contents: bytemuck::cast_slice(triangles.as_slice()),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    (buffer, triangles.len() as u32)
                });
            }
            let n = self.draws.len();
            if self.slots.len() <= n {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Mica surface texture uniform"),
                    size: std::mem::size_of::<Uniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.slots.push((buffer, None));
            }
            let (buffer, group) = &mut self.slots[n];
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&u));
            if group.as_ref().is_none_or(|(k, _)| *k != p.map_key) {
                let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Mica surface texture group"),
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
                *group = Some((p.map_key.clone(), g));
            }
            self.draws.push((n, p.area.as_ref().map(|a| a.0), p.object));
        }
    }

    /// Draws preview `n` of this frame: the object's mesh (`mesh`) or its painted area.
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        n: usize,
        mesh: Option<(&wgpu::Buffer, &wgpu::Buffer, u32)>,
    ) {
        let Some(&(slot, area, _)) = self.draws.get(n) else {
            return;
        };
        let Some((_, Some((_, group)))) = self.slots.get(slot) else {
            return;
        };
        pass.set_bind_group(0, group, &[]);
        match area {
            Some(key) => {
                if let Some((buffer, count)) = self.areas.get(&key) {
                    pass.set_pipeline(&self.area_pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
            None => {
                if let Some((vertices, indices, count)) = mesh {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..count, 0, 0..1);
                }
            }
        }
    }
}
