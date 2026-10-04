//! wgpu pipelines: splat pass into a float accumulation texture, then a tone-map pass into
//! the output (SPEC §3, Rendering).

mod snapshot;

pub use snapshot::{save_png, snapshot};

use std::fmt;

use bytemuck::{Pod, Zeroable};

use crate::sim::{Drawing, Splat};

/// Splats per upload; 12 MB.
const CHUNK: usize = 1 << 20;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(context: &str, e: impl fmt::Display) -> Error {
    Error(format!("{context}: {e}"))
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Rgba32Float when the adapter can blend it, else Rgba16Float.
    pub acc_format: wgpu::TextureFormat,
}

impl Gpu {
    /// Headless when `surface` is None.
    pub fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Gpu, Error> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: surface,
            apply_limit_buckets: false,
        }))
        .map_err(|e| err("no GPU adapter", e))?;
        let blend32 = adapter
            .features()
            .contains(wgpu::Features::FLOAT32_BLENDABLE);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("tan-screensaver"),
            required_features: if blend32 {
                wgpu::Features::FLOAT32_BLENDABLE
            } else {
                wgpu::Features::empty()
            },
            // Large snapshots need the adapter's real texture and buffer limits.
            required_limits: wgpu::Limits {
                max_buffer_size: adapter.limits().max_buffer_size,
                ..wgpu::Limits::default().using_resolution(adapter.limits())
            },
            ..Default::default()
        }))
        .map_err(|e| err("cannot open GPU device", e))?;
        let acc_format = if blend32 {
            wgpu::TextureFormat::Rgba32Float
        } else {
            wgpu::TextureFormat::Rgba16Float
        };
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
            acc_format,
        })
    }

    pub fn headless() -> Result<Gpu, Error> {
        Gpu::new(
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env()),
            None,
        )
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SplatParams {
    size: [f32; 2],
    inv_2_sigma2: f32,
    radius: f32,
    lut: [[f32; 4]; 256],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ToneParams {
    background: [f32; 4],
    exposure_step: f32,
    fade: f32,
    _pad: [f32; 2],
}

/// Accumulation texture and the passes that fill and display it, for one output.
pub struct Renderer {
    width_px: u32,
    height_px: u32,
    acc_view: wgpu::TextureView,
    splat_pipeline: wgpu::RenderPipeline,
    splat_params: wgpu::Buffer,
    splat_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    tone_pipeline: wgpu::RenderPipeline,
    tone_params: wgpu::Buffer,
    tone_bind: wgpu::BindGroup,
}

impl Renderer {
    pub fn new(
        gpu: &Gpu,
        width_px: u32,
        height_px: u32,
        target_format: wgpu::TextureFormat,
    ) -> Renderer {
        let device = &gpu.device;
        let acc = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("accumulation"),
            size: wgpu::Extent3d {
                width: width_px,
                height: height_px,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.acc_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let acc_view = acc.create_view(&Default::default());

        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let splat_params = uniform("splat params", size_of::<SplatParams>() as u64);
        let splat_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat"),
            entries: &[uniform_entry(0)],
        });
        let splat_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat"),
            layout: &splat_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: splat_params.as_entire_binding(),
            }],
        });
        let splat_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("splat"),
            source: wgpu::ShaderSource::Wgsl(include_str!("splat.wgsl").into()),
        });
        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let splat_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splat"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("splat"),
                    bind_group_layouts: &[Some(&splat_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &splat_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Splat>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &splat_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.acc_format,
                    blend: Some(wgpu::BlendState {
                        color: additive,
                        alpha: additive,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splats"),
            size: (CHUNK * size_of::<Splat>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let tone_params = uniform("tone params", size_of::<ToneParams>() as u64);
        let tone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tone map"),
            entries: &[
                uniform_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let tone_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tone map"),
            layout: &tone_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: tone_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&acc_view),
                },
            ],
        });
        let tone_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tone map"),
            source: wgpu::ShaderSource::Wgsl(include_str!("tonemap.wgsl").into()),
        });
        let tone_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tone map"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("tone map"),
                    bind_group_layouts: &[Some(&tone_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &tone_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &tone_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        Renderer {
            width_px,
            height_px,
            acc_view,
            splat_pipeline,
            splat_params,
            splat_bind,
            instances,
            tone_pipeline,
            tone_params,
            tone_bind,
        }
    }

    /// Sets the per-scene uniforms and clears the accumulation texture.
    pub fn start(&self, gpu: &Gpu, drawing: &Drawing) {
        // Accumulated in 0..1 so the Rgba16Float fallback (max 65504) does not overflow
        // in dense regions; the tone map scales back to 0..255.
        let mut lut = [[0.0; 4]; 256];
        for (out, c) in lut.iter_mut().zip(&drawing.lut) {
            *out = [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, 0.0];
        }
        let params = SplatParams {
            size: [self.width_px as f32, self.height_px as f32],
            inv_2_sigma2: drawing.kernel.inv_2_sigma2() as f32,
            radius: drawing.kernel.radius as f32,
            lut,
        };
        gpu.queue
            .write_buffer(&self.splat_params, 0, bytemuck::bytes_of(&params));
        self.set_tone(gpu, drawing, 1.0);

        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.acc_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        gpu.queue.submit([encoder.finish()]);
    }

    pub fn set_tone(&self, gpu: &Gpu, drawing: &Drawing, fade: f32) {
        let [r, g, b] = drawing.background.map(f32::from);
        let params = ToneParams {
            background: [r, g, b, 0.0],
            exposure_step: drawing.exposure_step as f32,
            fade,
            _pad: [0.0; 2],
        };
        gpu.queue
            .write_buffer(&self.tone_params, 0, bytemuck::bytes_of(&params));
    }

    /// Adds splats to the accumulation texture, one submission per chunk.
    pub fn splat(&self, gpu: &Gpu, splats: &[Splat]) {
        for chunk in splats.chunks(CHUNK) {
            gpu.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(chunk));
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("splat"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.acc_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&self.splat_pipeline);
                pass.set_bind_group(0, &self.splat_bind, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..4, 0..chunk.len() as u32);
            }
            gpu.queue.submit([encoder.finish()]);
        }
    }

    /// Draws the tone-mapped image into `target`.
    pub fn tone_map(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tone map"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.tone_pipeline);
        pass.set_bind_group(0, &self.tone_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
