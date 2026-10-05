//! wgpu pipelines: splat pass into a float accumulation texture, tone-map pass into the
//! output, glow pass for the spark heads on top (SPEC §3, Rendering).

mod snapshot;

pub use snapshot::{save_png, snapshot};

use std::fmt;
use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};

use crate::config::REF_HEIGHT_PX;
use crate::sim::{Drawing, Glow, Splat};

/// Splats per upload; 12 MB.
const CHUNK: usize = 1 << 20;

/// Spark head glow at `REF_HEIGHT_PX`: a small core pushed towards white and a wide soft
/// halo in the palette colour. Picked by eye.
const GLOW_CORE_SIGMA_PX: f32 = 1.5;
const GLOW_HALO_SIGMA_PX: f32 = 7.0;
const GLOW_CORE_GAIN: f32 = 1.0;
const GLOW_HALO_GAIN: f32 = 0.3;
const GLOW_WHITEN: f32 = 0.55;

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
    fault: Arc<Mutex<Option<String>>>,
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
        // wgpu's default handler panics; a screensaver that panics leaves a frozen frame.
        // Errors are recorded instead and the app exits cleanly.
        let fault = Arc::new(Mutex::new(None));
        let record = |fault: &Arc<Mutex<Option<String>>>| {
            let fault = fault.clone();
            move |message: String| {
                eprintln!("tan-screensaver: GPU error: {message}");
                if let Ok(mut f) = fault.lock() {
                    f.get_or_insert(message);
                }
            }
        };
        let on_error = record(&fault);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| on_error(e.to_string())));
        let on_lost = record(&fault);
        device.set_device_lost_callback(move |reason, message| {
            // Destroyed is the normal end of the device at exit.
            if reason != wgpu::DeviceLostReason::Destroyed {
                on_lost(format!("device lost ({reason:?}): {message}"))
            }
        });
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
            fault,
        })
    }

    /// The first GPU error or device loss since startup, if any.
    pub fn fault(&self) -> Option<String> {
        self.fault.lock().ok().and_then(|f| f.clone())
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlowParams {
    size: [f32; 2],
    core_sigma: f32,
    halo_sigma: f32,
    radius: f32,
    core_gain: f32,
    halo_gain: f32,
    whiten: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlowInstance {
    at: [f32; 2],
    /// 0..1.
    colour: [f32; 3],
    intensity: f32,
}

fn uniform_buffer(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

const ADDITIVE: wgpu::BlendComponent = wgpu::BlendComponent {
    src_factor: wgpu::BlendFactor::One,
    dst_factor: wgpu::BlendFactor::One,
    operation: wgpu::BlendOperation::Add,
};

struct PipelineSpec<'a> {
    label: &'a str,
    source: &'a str,
    layout: &'a wgpu::BindGroupLayout,
    instance: Option<wgpu::VertexBufferLayout<'a>>,
    topology: wgpu::PrimitiveTopology,
    target: wgpu::ColorTargetState,
}

fn pipeline(device: &wgpu::Device, spec: PipelineSpec<'_>) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(spec.label),
        source: wgpu::ShaderSource::Wgsl(spec.source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(spec.label),
        bind_group_layouts: &[Some(spec.layout)],
        immediate_size: 0,
    });
    let buffers: &[Option<wgpu::VertexBufferLayout<'_>>] = &[spec.instance];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: if buffers[0].is_some() { buffers } else { &[] },
        },
        primitive: wgpu::PrimitiveState {
            topology: spec.topology,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(spec.target)],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

/// Accumulation texture and the passes that fill and display it, for one output. The
/// pipelines are built once; `resize` only replaces what depends on the size.
pub struct Renderer {
    width_px: u32,
    height_px: u32,
    acc_format: wgpu::TextureFormat,
    acc_view: wgpu::TextureView,
    splat_pipeline: wgpu::RenderPipeline,
    splat_params: wgpu::Buffer,
    splat_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    tone_pipeline: wgpu::RenderPipeline,
    tone_layout: wgpu::BindGroupLayout,
    tone_params: wgpu::Buffer,
    tone_bind: wgpu::BindGroup,
    glow_pipeline: wgpu::RenderPipeline,
    glow_params: wgpu::Buffer,
    glow_bind: wgpu::BindGroup,
    glow_instances: wgpu::Buffer,
    glow_capacity: usize,
    glow_scratch: Vec<GlowInstance>,
    lut: [[f32; 3]; 256],
}

impl Renderer {
    pub fn new(
        gpu: &Gpu,
        width_px: u32,
        height_px: u32,
        target_format: wgpu::TextureFormat,
    ) -> Renderer {
        let device = &gpu.device;

        let splat_params = uniform_buffer(device, "splat params", size_of::<SplatParams>());
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
        let splat_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "splat",
                source: include_str!("splat.wgsl"),
                layout: &splat_layout,
                instance: Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Splat>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32],
                }),
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                target: wgpu::ColorTargetState {
                    format: gpu.acc_format,
                    blend: Some(wgpu::BlendState {
                        color: ADDITIVE,
                        alpha: ADDITIVE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                },
            },
        );
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splats"),
            size: (CHUNK * size_of::<Splat>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let tone_params = uniform_buffer(device, "tone params", size_of::<ToneParams>());
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
        let tone_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "tone map",
                source: include_str!("tonemap.wgsl"),
                layout: &tone_layout,
                instance: None,
                topology: wgpu::PrimitiveTopology::TriangleList,
                target: target_format.into(),
            },
        );

        let glow_params = uniform_buffer(device, "glow params", size_of::<GlowParams>());
        let glow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glow"),
            entries: &[uniform_entry(0)],
        });
        let glow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glow"),
            layout: &glow_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: glow_params.as_entire_binding(),
            }],
        });
        let glow_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "glow",
                source: include_str!("glow.wgsl"),
                layout: &glow_layout,
                instance: Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<GlowInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3, 2 => Float32],
                }),
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                target: wgpu::ColorTargetState {
                    format: target_format,
                    // Light added on top of the image; alpha stays as the tone map wrote it.
                    blend: Some(wgpu::BlendState {
                        color: ADDITIVE,
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                },
            },
        );
        let glow_capacity = 1024;
        let glow_instances = Renderer::glow_buffer(device, glow_capacity);

        let (acc_view, tone_bind) =
            Renderer::sized(gpu, width_px, height_px, &tone_layout, &tone_params);
        Renderer {
            width_px,
            height_px,
            acc_format: gpu.acc_format,
            acc_view,
            splat_pipeline,
            splat_params,
            splat_bind,
            instances,
            tone_pipeline,
            tone_layout,
            tone_params,
            tone_bind,
            glow_pipeline,
            glow_params,
            glow_bind,
            glow_instances,
            glow_capacity,
            glow_scratch: Vec::new(),
            lut: [[0.0; 3]; 256],
        }
    }

    fn glow_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glows"),
            size: (capacity * size_of::<GlowInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// The accumulation texture and the bind group that reads it.
    fn sized(
        gpu: &Gpu,
        width_px: u32,
        height_px: u32,
        tone_layout: &wgpu::BindGroupLayout,
        tone_params: &wgpu::Buffer,
    ) -> (wgpu::TextureView, wgpu::BindGroup) {
        let acc = gpu.device.create_texture(&wgpu::TextureDescriptor {
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
        let tone_bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tone map"),
            layout: tone_layout,
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
        (acc_view, tone_bind)
    }

    /// Replaces the accumulation texture; its contents are lost, so call `start` next.
    pub fn resize(&mut self, gpu: &Gpu, width_px: u32, height_px: u32) {
        debug_assert_eq!(gpu.acc_format, self.acc_format);
        let (acc_view, tone_bind) = Renderer::sized(
            gpu,
            width_px,
            height_px,
            &self.tone_layout,
            &self.tone_params,
        );
        (self.width_px, self.height_px) = (width_px, height_px);
        (self.acc_view, self.tone_bind) = (acc_view, tone_bind);
    }

    /// Sets the per-scene uniforms and clears the accumulation texture.
    pub fn start(&mut self, gpu: &Gpu, drawing: &Drawing) {
        // Accumulated in 0..1 so the Rgba16Float fallback (max 65504) does not overflow
        // in dense regions; the tone map scales back to 0..255.
        for (out, c) in self.lut.iter_mut().zip(&drawing.lut) {
            *out = c.map(|v| v / 255.0);
        }
        let mut lut = [[0.0; 4]; 256];
        for (out, c) in lut.iter_mut().zip(&self.lut) {
            *out = [c[0], c[1], c[2], 0.0];
        }
        let size = [self.width_px as f32, self.height_px as f32];
        let params = SplatParams {
            size,
            inv_2_sigma2: drawing.kernel.inv_2_sigma2() as f32,
            radius: drawing.kernel.radius as f32,
            lut,
        };
        gpu.queue
            .write_buffer(&self.splat_params, 0, bytemuck::bytes_of(&params));
        self.set_opacity(gpu, drawing, 1.0);

        let scale = self.height_px as f32 / REF_HEIGHT_PX as f32;
        let glow = GlowParams {
            size,
            core_sigma: GLOW_CORE_SIGMA_PX * scale,
            halo_sigma: GLOW_HALO_SIGMA_PX * scale,
            radius: (3.0 * GLOW_HALO_SIGMA_PX * scale).ceil(),
            core_gain: GLOW_CORE_GAIN,
            halo_gain: GLOW_HALO_GAIN,
            whiten: GLOW_WHITEN,
        };
        gpu.queue
            .write_buffer(&self.glow_params, 0, bytemuck::bytes_of(&glow));

        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        pass(
            &mut encoder,
            "clear",
            &self.acc_view,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        );
        gpu.queue.submit([encoder.finish()]);
    }

    /// Image opacity over the background: 1 normally, falling to 0 in the scene fade.
    pub fn set_opacity(&self, gpu: &Gpu, drawing: &Drawing, opacity: f32) {
        let [r, g, b] = drawing.background.map(f32::from);
        let params = ToneParams {
            background: [r, g, b, 0.0],
            exposure_step: drawing.exposure_step as f32,
            fade: opacity,
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
                let mut pass = pass(&mut encoder, "splat", &self.acc_view, wgpu::LoadOp::Load);
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
        let mut pass = pass(
            encoder,
            "tone map",
            target,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        );
        pass.set_pipeline(&self.tone_pipeline);
        pass.set_bind_group(0, &self.tone_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Fills `target` with the plain default background, before any scene exists.
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let [r, g, b] =
            crate::palette::rgb(crate::scene::PLAIN_BACKGROUND).map(|c| c as f64 / 255.0);
        pass(
            encoder,
            "blank",
            target,
            wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
        );
    }

    /// Adds the spark heads' glow on top of `target`. Not part of the accumulated image.
    pub fn glow(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        glows: impl Iterator<Item = Glow>,
    ) {
        let lut = &self.lut;
        self.glow_scratch.clear();
        self.glow_scratch.extend(glows.map(|g| GlowInstance {
            at: [g.x, g.y],
            colour: lut[g.colour as usize & 255],
            intensity: g.intensity,
        }));
        if self.glow_scratch.is_empty() {
            return;
        }
        if self.glow_scratch.len() > self.glow_capacity {
            self.glow_capacity = self.glow_scratch.len().next_power_of_two();
            self.glow_instances = Renderer::glow_buffer(&gpu.device, self.glow_capacity);
        }
        gpu.queue.write_buffer(
            &self.glow_instances,
            0,
            bytemuck::cast_slice(&self.glow_scratch),
        );
        let mut pass = pass(encoder, "glow", target, wgpu::LoadOp::Load);
        pass.set_pipeline(&self.glow_pipeline);
        pass.set_bind_group(0, &self.glow_bind, &[]);
        pass.set_vertex_buffer(0, self.glow_instances.slice(..));
        pass.draw(0..4, 0..self.glow_scratch.len() as u32);
    }
}
