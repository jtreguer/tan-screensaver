//! winit event loop: one fullscreen window per monitor (or one development window), each
//! with its own surface, renderer and scene cycle, and the input that ends them all.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use crate::budget::Prepared;
use crate::config::Settings;
use crate::cycle::{Change, Cycle, Phase};
use crate::render::{Gpu, Renderer};
use crate::rng::SplitMix64;
use crate::scene::{next_seed, Choices, Scene};
use crate::sim::{Sim, Splat};

/// Omarchy's window rules, idle service and lock handoff all key on this.
const APP_ID: &str = "org.omarchy.screensaver";
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
/// How often to check whether a scene being prepared is ready to start.
const PREPARE_POLL: Duration = Duration::from_millis(20);
/// Wayland sends a motion event when the pointer enters a surface, and windows mapping
/// under a still pointer send more; none of that is the user moving the mouse.
const MOTION_GRACE: Duration = Duration::from_millis(500);
const MOTION_PX: f64 = 10.0;
/// Focus moving between our own windows arrives as a loss then a gain; only a loss that
/// lasts means another surface (the lock screen, say) took over.
const FOCUS_SETTLE: Duration = Duration::from_millis(300);

pub struct Options {
    pub seed: u64,
    pub settings: Settings,
    pub choices: Choices,
    /// A normal window that only Escape or closing ends, instead of fullscreen.
    pub windowed: bool,
    /// Only this monitor, by connector name.
    pub output: Option<String>,
}

/// A signal asked the screensaver to end.
struct Quit;

pub fn run(options: Options) -> Result<(), String> {
    let event_loop = EventLoop::<Quit>::with_user_event()
        .build()
        .map_err(|e| format!("cannot open the display: {e}"))?;
    let proxy = event_loop.create_proxy();
    crate::signals::install(move |_| {
        // Fails only once the loop is gone, when there is nothing left to end.
        let _ = proxy.send_event(Quit);
    })?;
    let mut app = App {
        options,
        gpu: None,
        screens: Vec::new(),
        watch: Watch::new(Instant::now()),
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
    screens: Vec<Screen>,
    watch: Watch<WindowId>,
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
    /// None until the first scene is prepared, after a resize, and between scenes.
    show: Option<Show>,
    /// The scene to start when `show` is None.
    preparing: Option<Pending>,
    /// No scene starts before this, while resizes settle.
    restart_after: Option<Instant>,
    /// The last frame could not be presented; draw again at this time.
    retry_at: Option<Instant>,
    /// Check again at this time whether `preparing` is ready.
    poll_at: Option<Instant>,
}

struct Show {
    sim: Sim,
    cycle: Cycle,
    next: Option<Pending>,
    splats: Vec<Splat>,
    last_frame: Instant,
}

/// A scene being prepared for one screen size. The pre-pass takes up to a second at
/// 1440p, so it runs on a worker thread and the event loop keeps handling input.
struct Pending {
    size: (u32, u32),
    work: Work,
}

enum Work {
    Thread(JoinHandle<Prepared>),
    Done(Box<Prepared>),
}

impl Pending {
    fn spawn(seed: u64, options: &Options, size: (u32, u32)) -> Pending {
        let scene = Scene::draw(seed, &options.choices);
        let settings = options.settings.clone();
        let work = std::thread::Builder::new()
            .name("prepare scene".into())
            .spawn(move || Prepared::new(scene, &settings, size.0, size.1))
            .map_or_else(
                |e| {
                    eprintln!("tan-screensaver: preparing on this thread instead: {e}");
                    let scene = Scene::draw(seed, &options.choices);
                    Work::Done(Box::new(Prepared::new(
                        scene,
                        &options.settings,
                        size.0,
                        size.1,
                    )))
                },
                Work::Thread,
            );
        Pending { size, work }
    }

    fn is_ready(&self) -> bool {
        match &self.work {
            Work::Thread(handle) => handle.is_finished(),
            Work::Done(_) => true,
        }
    }

    /// Waits for the result; None if the worker panicked.
    fn into_prepared(self) -> Option<Prepared> {
        match self.work {
            Work::Thread(handle) => handle.join().ok(),
            Work::Done(prepared) => Some(*prepared),
        }
    }
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        self.error = Some(message);
        event_loop.exit();
    }

    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let windows = window_attributes(event_loop, &self.options)?
            .into_iter()
            .map(|a| event_loop.create_window(a).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot open a window: {e}"))?;
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
                Box::new(event_loop.owned_display_handle()),
            ));
        let surfaces = windows
            .iter()
            .map(|w| instance.create_surface(w.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot create a surface: {e}"))?;
        let gpu = Gpu::new(instance, surfaces.first()).map_err(|e| e.to_string())?;
        for (index, (window, surface)) in windows.into_iter().zip(surfaces).enumerate() {
            if !self.options.windowed {
                window.set_cursor_visible(false);
            }
            let seed = screen_seed(self.options.seed, index as u64);
            self.screens.push(Screen::new(&gpu, window, surface, seed)?);
        }
        self.gpu = Some(gpu);
        self.watch = Watch::new(Instant::now());
        Ok(())
    }

    /// Input that ends the screensaver (SPEC §2, Exiting). The development window only
    /// ends on Escape or closing.
    fn dismisses(&mut self, id: WindowId, event: &WindowEvent) -> bool {
        let now = Instant::now();
        match event {
            WindowEvent::CloseRequested => true,
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => !self.options.windowed || *logical_key == Key::Named(NamedKey::Escape),
            _ if self.options.windowed => false,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            }
            | WindowEvent::MouseWheel { .. } => true,
            WindowEvent::CursorMoved {
                position: PhysicalPosition { x, y },
                ..
            } => self.watch.pointer(id, *x, *y, now),
            WindowEvent::Focused(focused) => {
                self.watch.focus(id, *focused, now);
                false
            }
            _ => false,
        }
    }
}

/// The development window, or one fullscreen window per monitor (sorted by name, so a
/// seed gives each monitor the same scenes every time).
fn window_attributes(
    event_loop: &ActiveEventLoop,
    options: &Options,
) -> Result<Vec<WindowAttributes>, String> {
    let base = Window::default_attributes().with_title("tan-screensaver");
    if options.windowed {
        return Ok(vec![base
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_name(DEV_APP_ID, "")]);
    }
    let mut monitors: Vec<(String, MonitorHandle)> = event_loop
        .available_monitors()
        .map(|m| (m.name().unwrap_or_default(), m))
        .collect();
    monitors.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(output) = &options.output {
        let names: Vec<_> = monitors.iter().map(|(n, _)| n.clone()).collect();
        monitors.retain(|(n, _)| n == output);
        if monitors.is_empty() {
            return Err(format!(
                "no monitor named {output:?}; available: {}",
                names.join(", ")
            ));
        }
    }
    if monitors.is_empty() {
        return Err("no monitors found".into());
    }
    Ok(monitors
        .into_iter()
        .map(|(_, m)| {
            base.clone()
                .with_fullscreen(Some(Fullscreen::Borderless(Some(m))))
                .with_name(APP_ID, "")
        })
        .collect())
}

/// The first screen runs the given seed, so `--seed` and `--snapshot` agree on it; the
/// others get unrelated sequences.
fn screen_seed(seed: u64, index: u64) -> u64 {
    match index {
        0 => seed,
        _ => SplitMix64::new(seed.rotate_left(32) ^ index).next_u64(),
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
    fn new(
        gpu: &Gpu,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        seed: u64,
    ) -> Result<Screen, String> {
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
        let renderer = Renderer::new(gpu, w, h, view_format);
        Ok(Screen {
            window,
            surface,
            config,
            view_format,
            renderer,
            seed,
            show: None,
            preparing: None,
            restart_after: Some(Instant::now() + RESIZE_SETTLE),
            retry_at: None,
            poll_at: None,
        })
    }

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

    /// Starts the scene for `seed` once it is prepared for the current size, and returns
    /// whether a scene is running.
    fn start_prepared(&mut self, gpu: &Gpu, options: &Options) -> bool {
        let size = (self.config.width, self.config.height);
        self.poll_at = None;
        match self.preparing.take() {
            // A scene prepared for an old size is dropped at once; its thread is detached.
            Some(p) if p.size != size => {
                self.preparing = Some(Pending::spawn(self.seed, options, size))
            }
            Some(p) if !p.is_ready() => self.preparing = Some(p),
            Some(p) => match p.into_prepared() {
                Some(prepared) => {
                    self.start(gpu, prepared, options);
                    return true;
                }
                None => {
                    eprintln!(
                        "tan-screensaver: preparing scene {} failed; skipping it",
                        self.seed
                    );
                    self.seed = next_seed(self.seed);
                }
            },
            None => self.preparing = Some(Pending::spawn(self.seed, options, size)),
        }
        self.poll_at = Some(Instant::now() + PREPARE_POLL);
        false
    }

    fn frame(&mut self, gpu: &Gpu, options: &Options) {
        if self.restart_after.is_some_and(|t| Instant::now() < t) {
            // Wayland compositors map a window, and send its real size, only once it
            // has shown a buffer.
            self.present_blank(gpu);
            return;
        }
        self.restart_after = None;
        if self.show.is_none() && !self.start_prepared(gpu, options) {
            return;
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
                let size = (self.config.width, self.config.height);
                let next = Pending::spawn(next_seed(self.seed), options, size);
                if let Some(show) = self.show.as_mut() {
                    show.next = Some(next);
                }
            }
            Change::Next => {
                self.seed = next_seed(self.seed);
                self.preparing = self.show.take().and_then(|s| s.next);
                if !self.start_prepared(gpu, options) {
                    return;
                }
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
        let Some(show) = self.show.as_ref() else {
            return self.poll_at;
        };
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

impl ApplicationHandler<Quit> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.screens.is_empty() {
            return;
        }
        if let Err(e) = self.open(event_loop) {
            self.fail(event_loop, e);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: Quit) {
        event_loop.exit();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.dismisses(id, &event) {
            return event_loop.exit();
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(screen) = self.screens.iter_mut().find(|s| s.window.id() == id) else {
            return;
        };
        match event {
            WindowEvent::Resized(size) => screen.resize(gpu, size.width, size.height),
            WindowEvent::RedrawRequested => screen.frame(gpu, &self.options),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(fault) = self.gpu.as_ref().and_then(Gpu::fault) {
            return self.fail(event_loop, format!("GPU error: {fault}"));
        }
        let now = Instant::now();
        if self.watch.focus_lost(now) {
            return event_loop.exit();
        }
        let mut wake = self.watch.focus_deadline();
        for screen in &self.screens {
            match screen.wake_at() {
                Some(at) if now < at => wake = Some(wake.map_or(at, |w| w.min(at))),
                _ => screen.window.request_redraw(),
            }
        }
        event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

/// Pointer motion and focus, the inputs whose meaning depends on what came before.
struct Watch<I> {
    started: Instant,
    /// Window and position the pointer was last seen at during the grace period, or
    /// first seen at after it.
    anchor: Option<(I, f64, f64)>,
    focused: Vec<I>,
    /// All windows lost focus; end at this time unless one gets it back.
    unfocused_until: Option<Instant>,
}

impl<I: Copy + PartialEq> Watch<I> {
    fn new(started: Instant) -> Watch<I> {
        Watch {
            started,
            anchor: None,
            focused: Vec::new(),
            unfocused_until: None,
        }
    }

    /// Whether the pointer moved far enough to end the screensaver. Coordinates are
    /// window-local, so reaching another window counts as moving.
    fn pointer(&mut self, id: I, x: f64, y: f64, now: Instant) -> bool {
        match self.anchor {
            Some((a, ax, ay)) if now >= self.started + MOTION_GRACE => {
                a != id || (x - ax).hypot(y - ay) > MOTION_PX
            }
            _ => {
                self.anchor = Some((id, x, y));
                false
            }
        }
    }

    /// Windows never focused do not count: the compositor may not give focus at all.
    fn focus(&mut self, id: I, focused: bool, now: Instant) {
        let had_focus = !self.focused.is_empty();
        self.focused.retain(|&f| f != id);
        if focused {
            self.focused.push(id);
            self.unfocused_until = None;
        } else if had_focus && self.focused.is_empty() {
            self.unfocused_until = Some(now + FOCUS_SETTLE);
        }
    }

    fn focus_deadline(&self) -> Option<Instant> {
        self.unfocused_until
    }

    fn focus_lost(&self, now: Instant) -> bool {
        self.unfocused_until.is_some_and(|t| now >= t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn motion_counts_only_after_the_grace_period_and_beyond_the_threshold() {
        let t0 = Instant::now();
        let mut w = Watch::new(t0);
        assert!(!w.pointer(1, 100.0, 100.0, ms(t0, 10)));
        assert!(!w.pointer(1, 400.0, 300.0, ms(t0, 400)));
        assert!(!w.pointer(1, 406.0, 306.0, ms(t0, 600)));
        assert!(w.pointer(1, 408.0, 308.0, ms(t0, 700)));
    }

    #[test]
    fn a_still_pointer_is_anchored_after_the_grace_period() {
        let t0 = Instant::now();
        let mut w = Watch::new(t0);
        assert!(!w.pointer(1, 50.0, 50.0, ms(t0, 900)));
        assert!(!w.pointer(1, 55.0, 50.0, ms(t0, 950)));
        assert!(w.pointer(1, 61.0, 50.0, ms(t0, 990)));
    }

    #[test]
    fn reaching_another_window_counts_as_motion() {
        let t0 = Instant::now();
        let mut w = Watch::new(t0);
        assert!(!w.pointer(1, 0.0, 500.0, ms(t0, 100)));
        assert!(!w.pointer(2, 1919.0, 500.0, ms(t0, 200)));
        assert!(w.pointer(1, 0.0, 500.0, ms(t0, 800)));
    }

    #[test]
    fn focus_moving_between_windows_is_not_a_loss() {
        let t0 = Instant::now();
        let mut w = Watch::new(t0);
        w.focus(1, false, t0);
        assert_eq!(w.focus_deadline(), None);
        w.focus(1, true, t0);
        w.focus(1, false, ms(t0, 50));
        w.focus(2, true, ms(t0, 50));
        assert!(!w.focus_lost(ms(t0, 1000)));
        w.focus(2, false, ms(t0, 2000));
        assert!(!w.focus_lost(ms(t0, 2100)));
        assert!(w.focus_lost(ms(t0, 2300)));
    }
}
