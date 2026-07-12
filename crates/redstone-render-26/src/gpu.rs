use std::collections::BTreeMap;
use std::sync::mpsc;

use anyhow::{Context, Result, bail};
use glam::camera::rh::proj::directx::perspective;
use glam::camera::rh::proj::directx::orthographic;
use glam::camera::rh::view::look_to_mat4;
use glam::{Mat4, Quat, Vec3};
use wgpu::util::DeviceExt;

use crate::scene::{ChunkMeshUpdate, Vertex};
use crate::timeline::CameraPose;

pub(crate) struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    scene_bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    chunk_buffers: BTreeMap<(i32, i32), Vec<GpuMesh>>,
    dynamic_buffers: Vec<GpuMesh>,
    color: wgpu::Texture,
    multisample: Option<wgpu::Texture>,
    depth: wgpu::Texture,
    shadow: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_bytes_per_row: u32,
    width: u32,
    height: u32,
    fov_degrees: f32,
}

struct GpuMesh {
    buffer: wgpu::Buffer,
    vertex_count: u32,
}

impl GpuRenderer {
    pub(crate) async fn new(
        width: u32,
        height: u32,
        samples: u32,
        fov_degrees: f32,
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

        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let color = create_texture(
            &device,
            "video color",
            width,
            height,
            1,
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
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
            2048,
            2048,
            1,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
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
                buffers: &[Some(Vertex::layout())],
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
                buffers: &[Some(Vertex::layout())],
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
        let readback_size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("视频 readback buffer 过大")?;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
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
            dynamic_buffers: Vec::new(),
            color,
            multisample,
            depth,
            shadow,
            readback,
            padded_bytes_per_row,
            width,
            height,
            fov_degrees,
        })
    }

    pub(crate) fn update_meshes(&mut self, updates: Vec<ChunkMeshUpdate>) -> Result<()> {
        for update in updates {
            if update.vertices.is_empty() {
                self.chunk_buffers.remove(&update.chunk);
                continue;
            }
            let meshes = self.create_meshes(&update.vertices, "chunk scene vertices")?;
            self.chunk_buffers.insert(update.chunk, meshes);
        }
        Ok(())
    }

    pub(crate) fn update_dynamic_mesh(&mut self, vertices: &[Vertex]) -> Result<()> {
        self.dynamic_buffers = self.create_meshes(vertices, "dynamic scene vertices")?;
        Ok(())
    }

    fn create_meshes(&self, vertices: &[Vertex], label: &'static str) -> Result<Vec<GpuMesh>> {
        let vertex_size = std::mem::size_of::<Vertex>();
        let limit = usize::try_from(self.device.limits().max_buffer_size)
            .unwrap_or(usize::MAX)
            / vertex_size;
        let vertices_per_buffer = (limit / 3 * 3).max(3);
        let mut meshes = Vec::new();
        for vertices in vertices.chunks(vertices_per_buffer) {
            let vertex_count = u32::try_from(vertices.len())
                .context("单个渲染 buffer 顶点数量超过 u32")?;
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            meshes.push(GpuMesh {
                buffer,
                vertex_count,
            });
        }
        Ok(meshes)
    }

    pub(crate) fn render(&self, pose: CameraPose) -> Result<Vec<u8>> {
        let (view_projection, light_view_projection) =
            camera_matrices(pose, self.width, self.height, self.fov_degrees);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&view_projection.to_cols_array()),
        );
        self.queue.write_buffer(
            &self.light_buffer,
            0,
            bytemuck::cast_slice(&light_view_projection.to_cols_array()),
        );
        let color_view = self.color.create_view(&wgpu::TextureViewDescriptor::default());
        let multisample_view = self
            .multisample
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let depth_view = self.depth.create_view(&wgpu::TextureViewDescriptor::default());
        let shadow_view = self.shadow.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("video frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_view,
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
            for meshes in self.chunk_buffers.values() {
                for mesh in meshes {
                    pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                    pass.draw(0..mesh.vertex_count, 0..1);
                }
            }
            for mesh in &self.dynamic_buffers {
                pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                pass.draw(0..mesh.vertex_count, 0..1);
            }
        }
        {
            let attachment_view = multisample_view.as_ref().unwrap_or(&color_view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("video scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: attachment_view,
                    depth_slice: None,
                    resolve_target: multisample_view.as_ref().map(|_| &color_view),
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
                    view: &depth_view,
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
            for meshes in self.chunk_buffers.values() {
                for mesh in meshes {
                    pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                    pass.draw(0..mesh.vertex_count, 0..1);
                }
            }
            for mesh in &self.dynamic_buffers {
                pass.set_vertex_buffer(0, mesh.buffer.slice(..));
                pass.draw(0..mesh.vertex_count, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
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
        self.queue.submit(Some(encoder.finish()));
        let slice = self.readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .context("等待 GPU readback 失败")?;
        receiver
            .recv()
            .context("GPU readback callback 丢失")?
            .context("GPU readback 映射失败")?;
        let mapped = slice.get_mapped_range()?;
        let row_bytes = usize::try_from(self.width * 4).unwrap();
        let padded = usize::try_from(self.padded_bytes_per_row).unwrap();
        let output_len = row_bytes
            .checked_mul(self.height as usize)
            .context("视频帧大小溢出")?;
        let mut output = Vec::with_capacity(output_len);
        for row in mapped.chunks(padded).take(self.height as usize) {
            output.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        self.readback.unmap();
        if output.len() != output_len {
            bail!("GPU readback 帧长度无效");
        }
        Ok(output)
    }
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
