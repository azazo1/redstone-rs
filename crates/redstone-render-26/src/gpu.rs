use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use glam::camera::rh::proj::directx::perspective;
use glam::camera::rh::proj::directx::orthographic;
use glam::camera::rh::view::look_to_mat4;
use glam::{Mat4, Quat, Vec3, Vec4};
use redstone_core::BlockStateId;
use wgpu::util::DeviceExt;

use crate::scene::section::SectionPos;
use crate::scene::{ModelBatch, SectionMeshUpdate, Vertex};
use crate::timeline::CameraPose;
use crate::FrameFormat;

pub(crate) struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    scene_bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    chunk_buffers: BTreeMap<SectionPos, Vec<GpuMesh>>,
    section_models: BTreeMap<SectionPos, Vec<ModelBatch>>,
    models: BTreeMap<(BlockStateId, u8), GpuModel>,
    zero_instance: wgpu::Buffer,
    dynamic_buffers: Vec<GpuMesh>,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    multisample_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
    shadow_view: wgpu::TextureView,
    readbacks: Vec<ReadbackSlot>,
    pending_readbacks: VecDeque<usize>,
    next_readback: usize,
    padded_bytes_per_row: u32,
    frame_size: usize,
    frame_format: FrameFormat,
    nv12: Option<Nv12Converter>,
    width: u32,
    height: u32,
    shadow_enabled: bool,
    timings: GpuTimings,
    static_generation: u64,
    dynamic_generation: u64,
    dynamic_hash: u64,
    last_shadow_key: Option<ShadowKey>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GpuTimings {
    pub submit: Duration,
    pub readback_wait: Duration,
    pub cpu_copy: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShadowKey {
    matrix: [u32; 16],
    static_generation: u64,
    dynamic_generation: u64,
}

struct GpuMesh {
    buffer: wgpu::Buffer,
    vertex_count: u32,
    capacity: usize,
}

struct GpuModel {
    template: GpuMesh,
    instances: wgpu::Buffer,
    instance_count: u32,
    instance_capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    position: [f32; 3],
}

impl Instance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![3 => Float32x3];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

struct ReadbackSlot {
    buffer: wgpu::Buffer,
    submission: Option<wgpu::SubmissionIndex>,
    receiver: Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

struct Nv12Converter {
    output: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
}

impl GpuRenderer {
    pub(crate) async fn new(
        width: u32,
        height: u32,
        samples: u32,
        shadow_size: u32,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
        );
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
        {
            Ok(adapter) => adapter,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                })
                .await
                .context("没有可用的 wgpu 图形适配器")?,
        };
        tracing::info!(adapter = ?adapter.get_info(), "初始化 replay 离屏渲染器");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("redstone replay renderer"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .context("创建 wgpu device 失败")?;

        let format = wgpu::TextureFormat::Bgra8Unorm;
        let color = create_texture(
            &device,
            "video color",
            width,
            height,
            1,
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let multisample = (samples > 1).then(|| {
            create_texture(
                &device,
                "video msaa",
                width,
                height,
                samples,
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth = create_texture(
            &device,
            "video depth",
            width,
            height,
            samples,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let shadow = create_texture(
            &device,
            "directional shadow map",
            shadow_size.max(1),
            shadow_size.max(1),
            1,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let multisample_view = multisample
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let nv12 = if cfg!(target_os = "macos")
            && width.is_multiple_of(2)
            && height.is_multiple_of(2)
        {
            create_nv12_converter(&device, &color_view, width, height).await
        } else {
            None
        };
        let frame_format = if nv12.is_some() {
            FrameFormat::Nv12
        } else {
            FrameFormat::Bgra
        };
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light uniform"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_view = shadow.create_view(&wgpu::TextureViewDescriptor::default());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..wgpu::SamplerDescriptor::default()
        });
        let scene_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene bind group"),
            layout: &camera_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow bind group"),
            layout: &shadow_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("redstone scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shadow pipeline layout"),
                bind_group_layouts: &[Some(&shadow_layout)],
                immediate_size: 0,
            });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let unpadded = width.checked_mul(4).context("视频行宽溢出")?;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded.div_ceil(alignment) * alignment;
        let bgra_readback_size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("视频 readback buffer 过大")?;
        let bgra_frame_size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|size| size.checked_mul(4))
            .context("BGRA 视频帧大小溢出")?;
        let nv12_size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|size| size.checked_mul(3))
            .map(|size| size / 2)
            .context("NV12 视频帧大小溢出")?;
        let readback_size = match frame_format {
            FrameFormat::Bgra => bgra_readback_size,
            FrameFormat::Nv12 => nv12_size,
        };
        let frame_size = match frame_format {
            FrameFormat::Bgra => bgra_frame_size,
            FrameFormat::Nv12 => nv12_size,
        };
        let readbacks = (0..3)
            .map(|slot| ReadbackSlot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("video readback {slot}")),
                    size: readback_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                submission: None,
                receiver: None,
            })
            .collect();
        let zero_instance = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("zero model instance"),
            contents: bytemuck::bytes_of(&Instance {
                position: [0.0; 3],
            }),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            shadow_pipeline,
            camera_buffer,
            light_buffer,
            scene_bind_group,
            shadow_bind_group,
            chunk_buffers: BTreeMap::new(),
            section_models: BTreeMap::new(),
            models: BTreeMap::new(),
            zero_instance,
            dynamic_buffers: Vec::new(),
            color,
            color_view,
            multisample_view,
            depth_view,
            shadow_view,
            readbacks,
            pending_readbacks: VecDeque::new(),
            next_readback: 0,
            padded_bytes_per_row,
            frame_size: usize::try_from(frame_size).context("视频帧大小超过 usize")?,
            frame_format,
            nv12,
            width,
            height,
            shadow_enabled: shadow_size > 0,
            timings: GpuTimings::default(),
            static_generation: 0,
            dynamic_generation: 0,
            dynamic_hash: 0,
            last_shadow_key: None,
        })
    }

    pub(crate) fn frame_format(&self) -> FrameFormat {
        self.frame_format
    }

    pub(crate) fn timings(&self) -> GpuTimings {
        self.timings
    }

    pub(crate) fn update_meshes(&mut self, updates: Vec<SectionMeshUpdate>) -> Result<()> {
        if !updates.is_empty() {
            self.static_generation = self.static_generation.wrapping_add(1);
        }
        for update in updates {
            if update.vertices.is_empty() {
                self.chunk_buffers.remove(&update.section);
            } else {
                let meshes = self.chunk_buffers.entry(update.section).or_default();
                update_mesh_buffers(
                    &self.device,
                    &self.queue,
                    meshes,
                    &update.vertices,
                    "section scene vertices",
                )?;
            }
            if update.models.is_empty() {
                self.section_models.remove(&update.section);
            } else {
                self.section_models.insert(update.section, update.models);
            }
        }
        self.rebuild_models()
    }

    fn rebuild_models(&mut self) -> Result<()> {
        let mut groups = BTreeMap::<(BlockStateId, u8), (Arc<[Vertex]>, Vec<[f32; 3]>)>::new();
        for batches in self.section_models.values() {
            for batch in batches {
                let group = groups
                    .entry(batch.key)
                    .or_insert_with(|| (Arc::clone(&batch.template), Vec::new()));
                group.1.extend_from_slice(&batch.positions);
            }
        }
        self.models.retain(|key, _| groups.contains_key(key));
        for (key, (template, positions)) in groups {
            let instance_count = u32::try_from(positions.len())
                .context("模型实例数量超过 u32")?;
            let instance_bytes = bytemuck::cast_slice(&positions);
            if let Some(model) = self.models.get_mut(&key)
                && model.instance_capacity >= instance_bytes.len()
            {
                self.queue
                    .write_buffer(&model.instances, 0, instance_bytes);
                model.instance_count = instance_count;
                continue;
            }
            let template_bytes = bytemuck::cast_slice(&template);
            let template_buffer = self.device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("device model template"),
                    contents: template_bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                },
            );
            let instances = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("device model instances"),
                contents: instance_bytes,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            self.models.insert(
                key,
                GpuModel {
                    template: GpuMesh {
                        buffer: template_buffer,
                        vertex_count: u32::try_from(template.len())
                            .context("模型模板顶点数量超过 u32")?,
                        capacity: template_bytes.len(),
                    },
                    instances,
                    instance_count,
                    instance_capacity: instance_bytes.len(),
                },
            );
        }
        Ok(())
    }

    pub(crate) fn update_dynamic_mesh(&mut self, vertices: &[Vertex]) -> Result<()> {
        let hash = vertex_hash(vertices);
        if hash == self.dynamic_hash {
            return Ok(());
        }
        self.dynamic_hash = hash;
        self.dynamic_generation = self.dynamic_generation.wrapping_add(1);
        update_mesh_buffers(
            &self.device,
            &self.queue,
            &mut self.dynamic_buffers,
            vertices,
            "dynamic scene vertices",
        )
    }

    pub(crate) fn render(
        &mut self,
        pose: CameraPose,
        fov_degrees: f32,
    ) -> Result<Option<Vec<u8>>> {
        let completed = if self.pending_readbacks.len() == self.readbacks.len() {
            self.read_oldest()?
        } else {
            None
        };
        let (view_projection, light_view_projection) =
            camera_matrices(pose, self.width, self.height, fov_degrees);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&view_projection.to_cols_array()),
        );
        let shadow_key = ShadowKey {
            matrix: light_view_projection.to_cols_array().map(f32::to_bits),
            static_generation: self.static_generation,
            dynamic_generation: self.dynamic_generation,
        };
        let redraw_shadow = self.shadow_enabled && self.last_shadow_key != Some(shadow_key);
        self.queue.write_buffer(
            &self.light_buffer,
            0,
            bytemuck::cast_slice(&light_view_projection.to_cols_array()),
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("video frame encoder"),
            });
        if redraw_shadow {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.shadow_bind_group, &[]);
            for (section, meshes) in &self.chunk_buffers {
                if section_visible(light_view_projection, *section) {
                    for mesh in meshes {
                        pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                        pass.set_vertex_buffer(1, self.zero_instance.slice(..));
                        pass.draw(0..mesh.vertex_count, 0..1);
                    }
                }
            }
            for mesh in &self.dynamic_buffers {
                pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                pass.set_vertex_buffer(1, self.zero_instance.slice(..));
                pass.draw(0..mesh.vertex_count, 0..1);
            }
            for model in self.models.values() {
                pass.set_vertex_buffer(0, model.template.buffer.slice(..));
                pass.set_vertex_buffer(1, model.instances.slice(..));
                pass.draw(0..model.template.vertex_count, 0..model.instance_count);
            }
            drop(pass);
            self.last_shadow_key = Some(shadow_key);
        }
        {
            let attachment_view = self
                .multisample_view
                .as_ref()
                .unwrap_or(&self.color_view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("video scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: attachment_view,
                    depth_slice: None,
                    resolve_target: self.multisample_view.as_ref().map(|_| &self.color_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.035,
                            g: 0.045,
                            b: 0.055,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.scene_bind_group, &[]);
            for (section, meshes) in &self.chunk_buffers {
                if !section_visible(view_projection, *section) {
                    continue;
                }
                for mesh in meshes {
                    pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                    pass.set_vertex_buffer(1, self.zero_instance.slice(..));
                    pass.draw(0..mesh.vertex_count, 0..1);
                }
            }
            for mesh in &self.dynamic_buffers {
                pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                pass.set_vertex_buffer(1, self.zero_instance.slice(..));
                pass.draw(0..mesh.vertex_count, 0..1);
            }
            for model in self.models.values() {
                pass.set_vertex_buffer(0, model.template.buffer.slice(..));
                pass.set_vertex_buffer(1, model.instances.slice(..));
                pass.draw(0..model.template.vertex_count, 0..model.instance_count);
            }
        }
        let slot_index = self.next_readback;
        let slot = &self.readbacks[slot_index];
        if let Some(nv12) = &self.nv12 {
            encoder.clear_buffer(&nv12.output, 0, None);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("BGRA to NV12"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&nv12.pipeline);
                pass.set_bind_group(0, &nv12.bind_group, &[]);
                pass.dispatch_workgroups(self.width.div_ceil(8), self.height.div_ceil(8), 1);
            }
            encoder.copy_buffer_to_buffer(
                &nv12.output,
                0,
                &slot.buffer,
                0,
                self.frame_size as u64,
            );
        } else {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.color,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &slot.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(self.padded_bytes_per_row),
                        rows_per_image: Some(self.height),
                    },
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let submit_started = Instant::now();
        let submission = self.queue.submit(Some(encoder.finish()));
        self.timings.submit += submit_started.elapsed();
        let slice = self.readbacks[slot_index].buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.readbacks[slot_index].submission = Some(submission);
        self.readbacks[slot_index].receiver = Some(receiver);
        self.pending_readbacks.push_back(slot_index);
        self.next_readback = (slot_index + 1) % self.readbacks.len();
        Ok(completed)
    }

    pub(crate) fn finish_frames(&mut self) -> Result<Vec<Vec<u8>>> {
        let mut frames = Vec::with_capacity(self.pending_readbacks.len());
        while !self.pending_readbacks.is_empty() {
            if let Some(frame) = self.read_oldest()? {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    fn read_oldest(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(slot_index) = self.pending_readbacks.pop_front() else {
            return Ok(None);
        };
        let slot = &mut self.readbacks[slot_index];
        let submission = slot.submission.take().context("GPU readback 缺少提交索引")?;
        let wait_started = Instant::now();
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .context("等待 GPU readback 失败")?;
        slot.receiver
            .take()
            .context("GPU readback 缺少回调")?
            .recv()
            .context("GPU readback callback 丢失")?
            .context("GPU readback 映射失败")?;
        self.timings.readback_wait += wait_started.elapsed();
        let slice = slot.buffer.slice(..);
        let mapped = slice.get_mapped_range()?;
        let copy_started = Instant::now();
        let mut output = Vec::with_capacity(self.frame_size);
        if self.frame_format == FrameFormat::Nv12 {
            output.extend_from_slice(&mapped[..self.frame_size]);
        } else {
            let row_bytes = usize::try_from(self.width * 4).unwrap();
            let padded = usize::try_from(self.padded_bytes_per_row).unwrap();
            for row in mapped.chunks(padded).take(self.height as usize) {
                output.extend_from_slice(&row[..row_bytes]);
            }
        }
        drop(mapped);
        slot.buffer.unmap();
        if output.len() != self.frame_size {
            bail!("GPU readback 帧长度无效");
        }
        self.timings.cpu_copy += copy_started.elapsed();
        Ok(Some(output))
    }
}

async fn create_nv12_converter(
    device: &wgpu::Device,
    color_view: &wgpu::TextureView,
    width: u32,
    height: u32,
) -> Option<Nv12Converter> {
    let logical_size = u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(3)?
        / 2;
    let buffer_size = logical_size.div_ceil(4) * 4;
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("NV12 conversion output"),
        size: buffer_size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("NV12 conversion layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("NV12 conversion bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(color_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("NV12 conversion shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("nv12.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("NV12 conversion pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("NV12 conversion pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("convert"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    if let Some(error) = error_scope.pop().await {
        tracing::warn!(%error, "GPU NV12 初始化失败, 回退 BGRA readback");
        return None;
    }
    tracing::info!(width, height, "启用 GPU NV12 转换");
    Some(Nv12Converter {
        output,
        bind_group,
        pipeline,
    })
}

fn update_mesh_buffers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffers: &mut Vec<GpuMesh>,
    vertices: &[Vertex],
    label: &'static str,
) -> Result<()> {
    let vertex_size = std::mem::size_of::<Vertex>();
    let limit = usize::try_from(device.limits().max_buffer_size).unwrap_or(usize::MAX) / vertex_size;
    let vertices_per_buffer = (limit / 3 * 3).max(3);
    let required = vertices.len().div_ceil(vertices_per_buffer);
    for (index, vertices) in vertices.chunks(vertices_per_buffer).enumerate() {
        let bytes = bytemuck::cast_slice(vertices);
        let vertex_count = u32::try_from(vertices.len())
            .context("单个渲染 buffer 顶点数量超过 u32")?;
        if let Some(mesh) = buffers.get_mut(index).filter(|mesh| mesh.capacity >= bytes.len()) {
            queue.write_buffer(&mesh.buffer, 0, bytes);
            mesh.vertex_count = vertex_count;
            continue;
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let mesh = GpuMesh {
            buffer,
            vertex_count,
            capacity: bytes.len(),
        };
        if index < buffers.len() {
            buffers[index] = mesh;
        } else {
            buffers.push(mesh);
        }
    }
    buffers.truncate(required);
    Ok(())
}

fn vertex_hash(vertices: &[Vertex]) -> u64 {
    bytemuck::cast_slice::<Vertex, u8>(vertices)
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn section_visible(view_projection: Mat4, section: SectionPos) -> bool {
    let min = Vec3::new(
        (section.x * 16) as f32,
        (section.y * 16) as f32,
        (section.z * 16) as f32,
    );
    let max = min + Vec3::splat(16.0);
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, max.y, max.z),
    ]
    .map(|corner| view_projection * corner.extend(1.0));
    !outside(&corners, |point| point.x < -point.w)
        && !outside(&corners, |point| point.x > point.w)
        && !outside(&corners, |point| point.y < -point.w)
        && !outside(&corners, |point| point.y > point.w)
        && !outside(&corners, |point| point.z < 0.0)
        && !outside(&corners, |point| point.z > point.w)
}

fn outside(corners: &[Vec4; 8], outside: impl Fn(Vec4) -> bool) -> bool {
    corners.iter().copied().all(outside)
}

fn create_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    sample_count: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn camera_matrices(
    pose: CameraPose,
    width: u32,
    height: u32,
    fov_degrees: f32,
) -> (Mat4, Mat4) {
    let yaw = pose.rotation[0].to_radians();
    let pitch = pose.rotation[1].to_radians();
    let roll = pose.rotation[2].to_radians();
    let forward = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    )
    .normalize_or_zero();
    let eye = Vec3::new(
        pose.position[0] as f32,
        pose.position[1] as f32 + 1.62,
        pose.position[2] as f32,
    );
    let base_up = Vec3::Y;
    let up = Quat::from_axis_angle(forward, roll) * base_up;
    let view = look_to_mat4(eye, forward, up);
    let projection = perspective(
        fov_degrees.to_radians(),
        width as f32 / height as f32,
        0.05,
        4096.0,
    );
    let light_direction = Vec3::new(0.42, 0.82, 0.38).normalize();
    let focus = eye + forward * 64.0;
    let light_view = look_to_mat4(focus + light_direction * 220.0, -light_direction, Vec3::Y);
    let light_projection = orthographic(-128.0, 128.0, -128.0, 128.0, 0.1, 500.0);
    (projection * view, light_projection * light_view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frustum_rejects_sections_outside_an_orthographic_view() {
        let projection = orthographic(-32.0, 32.0, -32.0, 32.0, 0.0, 64.0);
        assert!(section_visible(
            projection,
            SectionPos { x: 0, y: 0, z: 0 }
        ));
        assert!(!section_visible(
            projection,
            SectionPos { x: 10, y: 0, z: 0 }
        ));
    }

    #[test]
    fn bt709_limited_range_has_expected_neutral_endpoints() {
        let convert = |value: f32| {
            let y = 16.0 + 255.0 * value * (0.182586 + 0.614231 + 0.062007);
            let u = 128.0 + 255.0 * value * (-0.100644 - 0.338572 + 0.439216);
            let v = 128.0 + 255.0 * value * (0.439216 - 0.398942 - 0.040274);
            [y.round() as u8, u.round() as u8, v.round() as u8]
        };
        assert_eq!(convert(0.0), [16, 128, 128]);
        assert_eq!(convert(1.0), [235, 128, 128]);
    }
}
