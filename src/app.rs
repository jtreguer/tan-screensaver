//! winit event loop: one window, its surface and renderer, and the scene cycle.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

use crate::budget::Prepared;
use crate::config::Settings;
use crate::cycle::{Change, Cycle, Phase};
use crate::render::{Gpu, Renderer};
use crate::scene::{next_seed, Choices, Scene};
use crate::sim::{Sim, Splat};

/// The development window must not match Omarchy's rules for `org.omarchy.screensaver`
/// (fullscreen, and the idle service counting it as the screensaver).
const DEV_APP_ID: &str = "tan-screensaver-dev";
/// A stalled frame must not make sparks jump.
const MAX_DT_S: f64 = 0.05;
/// Scenes restart after a resize; waiting for resizes to settle avoids preparing a scene
/// per step of a drag, or for the size a tiling compositor replaces at once.
const RESIZE_SETTLE: Duration = Duration::from_millis(200);
/// Retry delay when the surface cannot give a frame (occluded, timed out).
const PRESENT_RETRY: Duration = Duration::from_millis(50);

pub struct Options {
    pub seed: u64,
    pub settings: Settings,
    pub choices: Choices,
}

pub fn run(options: Options) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("cannot open the display: {e}"))?;
    let mut app = App {
        options,
        gpu: None,
        screen: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop: {e}"))?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    options: Options,
    gpu: Option<Gpu>,
    screen: Option<Screen>,
    error: Option<String>,
}

struct Screen {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Non-sRGB view of the surface: the tone map writes raw values, as flow.js does.
    view_format: wgpu::TextureFormat,
    renderer: Renderer,
    /// Seed of the scene to (re)start when `show` is None.
    seed: u64,
    /// None until the first frame, and again after a resize.
    show: Option<Show>,
    /// No scene starts before this, while resizes settle.
    restart_after: Option<Instant>,
    /// The last frame could not be presented; draw again at this time.
    retry_at: Option<Instant>,
}

struct Show {
    sim: Sim,
    cycle: Cycle,
    next: Option<JoinHandle<Prepared>>,
    splats: Vec<Splat>,
    last_frame: Instant,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        eprintln!("tan-screensaver: {message}");
        self.error = Some(message);
        event_loop.exit();
    }

    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attributes = Window::default_attributes()
            .with_title("tan-screensaver")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_name(DEV_APP_ID, "");
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| format!("cannot open a window: {e}"))?,
        );

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
                Box::new(event_loop.owned_display_handle()),
            ));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("cannot create a surface: {e}"))?;
        let gpu = Gpu::new(instance, Some(&surface)).map_err(|e| e.to_string())?;

        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let mut config = surface
            .get_default_config(&gpu.adapter, w, h)
            .ok_or("the GPU cannot present to this window")?;
        let caps = surface.get_capabilities(&gpu.adapter);
        config.present_mode = wgpu::PresentMode::Fifo;
        if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        let view_format = config.format.remove_srgb_suffix();
        if view_format != config.format {
            config.view_formats = vec![view_format];
        }
        surface.configure(&gpu.device, &config);
        let renderer = Renderer::new(&gpu, w, h, view_format);

        self.screen = Some(Screen {
            window,
            surface,
            config,
            view_format,
            renderer,
            seed: self.options.seed,
            show: None,
            restart_after: Some(Instant::now() + RESIZE_SETTLE),
            retry_at: None,
        });
        self.gpu = Some(gpu);
        Ok(())
    }
}

fn log_scene(p: &Prepared, settings: &Settings) {
    eprintln!(
        "scene {} | {:.0} px of visible path, {:.0} px/s, about {:.0} s",
        p.scene,
        p.length_px,
        p.speed_px,
        p.expected_seconds(settings)
    );
}

impl Screen {
    fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == (self.config.width, self.config.height) {
            return;
        }
        (self.config.width, self.config.height) = (width, height);
        self.surface.configure(&gpu.device, &self.config);
        self.renderer.resize(gpu, width, height);
        // The drawing depends on the size, so the scene starts over.
        self.show = None;
        self.restart_after = Some(Instant::now() + RESIZE_SETTLE);
    }

    fn start(&mut self, gpu: &Gpu, prepared: Prepared, options: &Options) {
        log_scene(&prepared, &options.settings);
        self.renderer.start(gpu, &prepared.drawing);
        let sim = Sim::new(
            prepared.drawing.clone(),
            options.settings.sparks,
            prepared.speed_px,
        );
        let s = &options.settings;
        self.show = Some(Show {
            sim,
            cycle: Cycle::new(s.hold_seconds, s.fade_seconds),
            next: None,
            splats: Vec::new(),
            last_frame: Instant::now(),
        });
    }

    fn prepare_next(&self, options: &Options) -> Option<JoinHandle<Prepared>> {
        let scene = Scene::draw(next_seed(self.seed), &options.choices);
        let settings = options.settings.clone();
        let (w, h) = (self.config.width, self.config.height);
        std::thread::Builder::new()
            .name("prepare scene".into())
            .spawn(move || Prepared::new(scene, &settings, w, h))
            .map_err(|e| eprintln!("tan-screensaver: preparing on this thread instead: {e}"))
            .ok()
    }

    fn frame(&mut self, gpu: &Gpu, options: &Options) {
        if self.restart_after.is_some_and(|t| Instant::now() < t) {
            // Wayland compositors map a window, and send its real size, only once it
            // has shown a buffer.
            self.present_blank(gpu);
            return;
        }
        self.restart_after = None;
        if self.show.is_none() {
            let scene = Scene::draw(self.seed, &options.choices);
            let prepared = Prepared::new(
                scene,
                &options.settings,
                self.config.width,
                self.config.height,
            );
            self.start(gpu, prepared, options);
        }
        let Some(show) = self.show.as_mut() else {
            return;
        };

        let now = Instant::now();
        let dt = now.duration_since(show.last_frame).as_secs_f64();
        show.last_frame = now;
        let change = match show.cycle.phase() {
            Phase::Drawing => {
                show.splats.clear();
                show.sim.step(dt.min(MAX_DT_S), &mut show.splats);
                self.renderer.splat(gpu, &show.splats);
                show.cycle.advance(dt, show.sim.finished())
            }
            Phase::Holding | Phase::Fading => show.cycle.advance(dt, true),
        };
        match change {
            Change::Hold => {
                let next = self.prepare_next(options);
                if let Some(show) = self.show.as_mut() {
                    show.next = next;
                }
            }
            Change::Next => {
                self.seed = next_seed(self.seed);
                let ready = self
                    .show
                    .as_mut()
                    .and_then(|s| s.next.take())
                    .and_then(|h| h.join().ok());
                let prepared = ready.unwrap_or_else(|| {
                    let scene = Scene::draw(self.seed, &options.choices);
                    Prepared::new(
                        scene,
                        &options.settings,
                        self.config.width,
                        self.config.height,
                    )
                });
                self.start(gpu, prepared, options);
            }
            Change::Fade | Change::None => {}
        }
        self.present(gpu);
    }

    fn acquire(&mut self, gpu: &Gpu) -> Option<(wgpu::SurfaceTexture, bool)> {
        let now = Instant::now();
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Some((f, false)),
            // Reconfiguring while the frame is alive is an error; present it first.
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some((f, true)),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&gpu.device, &self.config);
                self.retry_at = Some(now);
                None
            }
            // Validation errors also reach the GPU fault handler, which ends the app.
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                self.retry_at = Some(now + PRESENT_RETRY);
                None
            }
        }
    }

    fn finish(&mut self, gpu: &Gpu, frame: wgpu::SurfaceTexture, reconfigure: bool) {
        self.retry_at = None;
        self.window.pre_present_notify();
        gpu.queue.present(frame);
        if reconfigure {
            self.surface.configure(&gpu.device, &self.config);
        }
    }

    fn present_blank(&mut self, gpu: &Gpu) {
        let Some((frame, reconfigure)) = self.acquire(gpu) else {
            return;
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer.clear(&mut encoder, &view);
        gpu.queue.submit([encoder.finish()]);
        self.finish(gpu, frame, reconfigure);
    }

    fn present(&mut self, gpu: &Gpu) {
        if self.show.is_none() {
            return;
        }
        let Some((frame, reconfigure)) = self.acquire(gpu) else {
            return;
        };
        let Some(show) = self.show.as_ref() else {
            return;
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        self.renderer
            .set_opacity(gpu, show.sim.drawing(), show.cycle.opacity());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer.tone_map(&mut encoder, &view);
        if show.cycle.phase() == Phase::Drawing {
            self.renderer
                .glow(gpu, &mut encoder, &view, show.sim.glows());
        }
        gpu.queue.submit([encoder.finish()]);
        self.finish(gpu, frame, reconfigure);
    }

    /// When the next frame is due, or None to draw continuously.
    fn wake_at(&self) -> Option<Instant> {
        if let Some(t) = self.restart_after.or(self.retry_at) {
            return Some(t);
        }
        let show = self.show.as_ref()?;
        match show.cycle.phase() {
            // The finished image stays on screen without redrawing.
            Phase::Holding => {
                let left =
                    Duration::try_from_secs_f64(show.cycle.hold_left_s()).unwrap_or_default();
                Some(show.last_frame + left)
            }
            Phase::Drawing | Phase::Fading => None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.screen.is_some() {
            return;
        }
        if let Err(e) = self.open(event_loop) {
            self.fail(event_loop, e);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(gpu), Some(screen)) = (self.gpu.as_ref(), self.screen.as_mut()) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(size) => screen.resize(gpu, size.width, size.height),
            WindowEvent::RedrawRequested => screen.frame(gpu, &self.options),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(fault) = self.gpu.as_ref().and_then(Gpu::fault) {
            return self.fail(event_loop, format!("GPU error: {fault}"));
        }
        let Some(screen) = self.screen.as_ref() else {
            return;
        };
        match screen.wake_at() {
            Some(at) if Instant::now() < at => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(at))
            }
            _ => {
                event_loop.set_control_flow(ControlFlow::Wait);
                screen.window.request_redraw();
            }
        }
    }
}
