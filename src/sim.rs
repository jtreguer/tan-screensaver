//! From a scene to splats: the whole scene at once for snapshots, or a pool of sparks
//! advancing frame by frame for the live window.

use bytemuck::{Pod, Zeroable};

use crate::config::{Settings, REF_HEIGHT_PX};
use crate::field::Field;
use crate::integrate::{max_steps, Head, Stop, View};
use crate::palette::{colour_index, ColourMode, Lut, PALETTES};
use crate::rng::Mulberry32;
use crate::scene::Scene;

/// One Gaussian deposit, as uploaded to the GPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Splat {
    /// Pixel coordinates, y down; pixel i is centred on i, as in flow.js.
    pub x: f32,
    pub y: f32,
    /// Index into the LUT.
    pub colour: u32,
}

/// Gaussian splat footprint, as flow.js `makeKernel`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kernel {
    pub sigma: f64,
    pub radius: u32,
}

impl Kernel {
    pub fn new(line_width_px: f64) -> Kernel {
        let sigma = line_width_px.max(0.5) * 0.45;
        let radius = ((2.2 * sigma).ceil() as u32).max(1);
        Kernel { sigma, radius }
    }

    pub fn inv_2_sigma2(&self) -> f64 {
        1.0 / (2.0 * self.sigma * self.sigma)
    }
}

/// Everything needed to draw one scene at one output size.
#[derive(Clone, Debug)]
pub struct Drawing {
    pub field: Field,
    pub view: View,
    pub step_px: f64,
    pub h_world: f64,
    pub max_steps: u32,
    pub start_seed: u32,
    pub trajectories: u32,
    pub trace_backward: bool,
    pub colour_mode: ColourMode,
    pub kappa0: f64,
    pub lut: Lut,
    pub background: [u8; 3],
    pub kernel: Kernel,
    /// Tone-mapping density factor: exposure × step, since density per pixel scales as
    /// 1/step.
    pub exposure_step: f64,
}

impl Drawing {
    pub fn new(scene: &Scene, settings: &Settings, width_px: u32, height_px: u32) -> Drawing {
        let view = View::new(width_px, height_px, scene.cx, scene.cy, scene.span);
        let h_world = view.h_world(settings.step_px);
        let line_width_px = settings.line_width * height_px as f64 / REF_HEIGHT_PX;
        Drawing {
            field: scene.field,
            view,
            step_px: settings.step_px,
            h_world,
            max_steps: max_steps(settings.arc_length, h_world),
            start_seed: scene.start_seed,
            trajectories: settings.trajectories,
            trace_backward: settings.trace_backward,
            colour_mode: scene.colour_mode,
            kappa0: scene.kappa0,
            lut: PALETTES[scene.palette].lut(scene.reversed),
            background: scene.background,
            kernel: Kernel::new(line_width_px),
            exposure_step: settings.exposure * settings.step_px,
        }
    }

    pub fn start_points(&self) -> StartPoints {
        StartPoints {
            rng: Mulberry32::new(self.start_seed),
            left: self.trajectories,
            x: (self.view.xmin, self.view.xmax),
            y: (self.view.ymin, self.view.ymax),
        }
    }

    pub fn heads(&self, sx: f64, sy: f64) -> impl Iterator<Item = Head> {
        let dirs: &[f64] = if self.trace_backward {
            &[1.0, -1.0]
        } else {
            &[1.0]
        };
        dirs.iter().map(move |&dir| Head::new(sx, sy, dir))
    }

    /// Advances a head one step; the splat is None when the step lands entirely off
    /// screen.
    pub fn advance(&self, head: &mut Head) -> Result<Option<Splat>, Stop> {
        let step = head.advance(&self.field, &self.view, self.h_world, self.max_steps)?;
        let (px, py) = self.view.to_px(step.x, step.y);
        if !self.touches_screen(px, py) {
            return Ok(None);
        }
        Ok(Some(Splat {
            x: px as f32,
            y: py as f32,
            colour: colour_index(step.kappa, self.colour_mode, self.kappa0) as u32,
        }))
    }

    /// Whether any pixel of the splat footprint is on screen. flow.js covers the
    /// pixels round(p) ± radius on each axis.
    fn touches_screen(&self, px: f64, py: f64) -> bool {
        let r = self.kernel.radius as f64;
        let (ix, iy) = ((px + 0.5).floor(), (py + 0.5).floor());
        ix + r >= 0.0
            && ix - r < self.view.width_px
            && iy + r >= 0.0
            && iy - r < self.view.height_px
    }

    /// Traces the whole scene, as flow.js `render` does.
    pub fn trace_all(&self, mut on_splat: impl FnMut(Splat)) {
        for (sx, sy) in self.start_points() {
            for mut head in self.heads(sx, sy) {
                while let Ok(splat) = self.advance(&mut head) {
                    if let Some(s) = splat {
                        on_splat(s);
                    }
                }
            }
        }
    }
}

/// Start points in flow.js order: uniform over the view plus margin, x drawn first.
#[derive(Clone, Debug)]
pub struct StartPoints {
    rng: Mulberry32,
    left: u32,
    x: (f64, f64),
    y: (f64, f64),
}

impl Iterator for StartPoints {
    type Item = (f64, f64);

    fn next(&mut self) -> Option<(f64, f64)> {
        self.left = self.left.checked_sub(1)?;
        let sx = self.x.0 + self.rng.next_f64() * (self.x.1 - self.x.0);
        let sy = self.y.0 + self.rng.next_f64() * (self.y.1 - self.y.0);
        Some((sx, sy))
    }
}

/// Fade-in and fade-out time of a head's glow, so sparks do not pop.
pub const GLOW_FADE_S: f64 = 0.3;
/// Off-screen steps cost no animation time, but each takes CPU; this caps them at a
/// millisecond or two per frame for all heads together. Shared out per head, so one long
/// off-screen stretch cannot hold up the heads after it.
const OFF_SCREEN_STEPS_PER_FRAME: usize = 16384;
const MIN_OFF_SCREEN_STEPS_PER_HEAD: usize = 64;

/// A spark head for the glow pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    pub x: f32,
    pub y: f32,
    pub colour: u32,
    pub intensity: f32,
}

#[derive(Clone, Debug)]
struct LiveHead {
    /// Order of creation; heads keep it across frames. Only the tests read it.
    #[cfg_attr(not(test), expect(dead_code))]
    id: u64,
    head: Head,
    /// Pixels the head may still travel this frame.
    budget_px: f64,
    last: Option<Splat>,
    on_screen: bool,
    visible_s: f64,
}

#[derive(Clone, Debug)]
struct Ghost {
    at: Splat,
    age_s: f64,
}

/// The sparks of one scene. Every start point is traced to the end, so once finished
/// the splats add up to the same image as `Drawing::trace_all`.
pub struct Sim {
    drawing: Drawing,
    starts: StartPoints,
    heads: Vec<LiveHead>,
    /// Heads in flight: sparks × heads per spark. A new start point comes in as soon as
    /// both its heads fit, rather than when the whole previous spark is done, so heads
    /// whose partner is still running do not leave a slot idle.
    capacity: usize,
    ghosts: Vec<Ghost>,
    speed_px: f64,
    next_id: u64,
}

impl Sim {
    pub fn new(drawing: Drawing, sparks: u32, speed_px: f64) -> Sim {
        let per_spark = drawing.heads(0.0, 0.0).count();
        let mut sim = Sim {
            starts: drawing.start_points(),
            drawing,
            heads: Vec::new(),
            capacity: sparks as usize * per_spark,
            ghosts: Vec::new(),
            speed_px,
            next_id: 0,
        };
        sim.refill();
        sim
    }

    fn refill(&mut self) {
        let per_spark = self.drawing.heads(0.0, 0.0).count();
        while self.heads.len() + per_spark <= self.capacity {
            let Some((sx, sy)) = self.starts.next() else {
                break;
            };
            let first = self.next_id;
            self.next_id += per_spark as u64;
            self.heads.extend(
                self.drawing
                    .heads(sx, sy)
                    .zip(first..)
                    .map(|(head, id)| LiveHead {
                        id,
                        head,
                        budget_px: 0.0,
                        last: None,
                        on_screen: false,
                        visible_s: 0.0,
                    }),
            );
        }
    }

    pub fn drawing(&self) -> &Drawing {
        &self.drawing
    }

    /// Done when every start point has been traced and the last glows have faded.
    pub fn finished(&self) -> bool {
        self.heads.is_empty() && self.ghosts.is_empty()
    }

    /// Advances every head by `speed · dt` pixels of visible path and appends the
    /// splats laid down.
    pub fn step(&mut self, dt: f64, out: &mut Vec<Splat>) {
        let d = &self.drawing;
        let off_screen_cap = (OFF_SCREEN_STEPS_PER_FRAME / self.heads.len().max(1))
            .max(MIN_OFF_SCREEN_STEPS_PER_HEAD);
        for lh in &mut self.heads {
            lh.budget_px += self.speed_px * dt;
            if lh.on_screen {
                lh.visible_s += dt;
            }
            let mut off_screen = 0;
            while lh.budget_px >= d.step_px {
                match d.advance(&mut lh.head) {
                    Ok(Some(splat)) => {
                        out.push(splat);
                        lh.last = Some(splat);
                        lh.on_screen = true;
                        lh.budget_px -= d.step_px;
                    }
                    Ok(None) => {
                        if lh.on_screen {
                            // Leaving the screen: fade out at the edge rather than vanish,
                            // and fade in again if it comes back.
                            if let Some(at) = lh.last {
                                self.ghosts.push(Ghost { at, age_s: 0.0 });
                            }
                            lh.on_screen = false;
                            lh.visible_s = 0.0;
                        }
                        off_screen += 1;
                        if off_screen == off_screen_cap {
                            // Held back by the cap: do not save up travel and then jump
                            // when it comes back.
                            lh.budget_px = lh.budget_px.min(self.speed_px * dt);
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        for lh in self.heads.extract_if(.., |lh| lh.head.stopped.is_some()) {
            if let (Some(at), true) = (lh.last, lh.on_screen) {
                self.ghosts.push(Ghost { at, age_s: 0.0 });
            }
        }
        self.refill();
        for g in &mut self.ghosts {
            g.age_s += dt;
        }
        self.ghosts.retain(|g| g.age_s < GLOW_FADE_S);
    }

    pub fn glows(&self) -> impl Iterator<Item = Glow> + '_ {
        let live = self
            .heads
            .iter()
            .filter(|lh| lh.on_screen)
            .filter_map(|lh| {
                let at = lh.last?;
                Some((at, (lh.visible_s / GLOW_FADE_S).min(1.0)))
            });
        let ghosts = self
            .ghosts
            .iter()
            .map(|g| (g.at, 1.0 - g.age_s / GLOW_FADE_S));
        live.chain(ghosts).map(|(at, intensity)| Glow {
            x: at.x,
            y: at.y,
            colour: at.colour,
            intensity: intensity as f32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Choices, Scene};

    #[test]
    fn kernel_matches_flow_js() {
        assert_eq!(Kernel::new(1.2).radius, 2);
        assert_eq!(Kernel::new(0.1), Kernel::new(0.5));
        assert_eq!(Kernel::new(0.5).radius, 1);
        assert_eq!(Kernel::new(2.5).radius, 3);
    }

    #[test]
    fn line_width_scales_with_height() {
        let scene = Scene::draw(1, &Choices::default());
        let s = Settings::default();
        let big = Drawing::new(&scene, &s, 2560, 1440);
        let small = Drawing::new(&scene, &s, 1920, 1080);
        assert!((big.kernel.sigma - 1.2 * 0.45).abs() < 1e-12);
        assert!((small.kernel.sigma - 0.9 * 0.45).abs() < 1e-12);
    }

    #[test]
    fn splats_stay_near_the_screen() {
        let scene = Scene::draw(3, &Choices::default());
        let settings = Settings {
            trajectories: 50,
            ..Settings::default()
        };
        let d = Drawing::new(&scene, &settings, 320, 180);
        let r = d.kernel.radius as f32;
        let mut n = 0;
        d.trace_all(|s| {
            n += 1;
            assert!(s.x > -r - 1.0 && s.x < 320.0 + r && s.y > -r - 1.0 && s.y < 180.0 + r);
            assert!(s.colour < 256);
        });
        assert!(n > 0);
    }

    fn sorted(mut v: Vec<Splat>) -> Vec<(u32, u32, u32)> {
        let mut k: Vec<_> = v
            .drain(..)
            .map(|s| (s.x.to_bits(), s.y.to_bits(), s.colour))
            .collect();
        k.sort_unstable();
        k
    }

    #[test]
    fn live_sim_lays_down_the_snapshot_splats() {
        for seed in [3, 4, 11] {
            let scene = Scene::draw(seed, &Choices::default());
            let settings = Settings {
                trajectories: 40,
                ..Settings::default()
            };
            let d = Drawing::new(&scene, &settings, 320, 180);
            let mut whole = Vec::new();
            d.trace_all(|s| whole.push(s));

            let mut sim = Sim::new(d, 7, 300.0);
            let mut live = Vec::new();
            let mut frames = 0;
            while !sim.finished() {
                sim.step(1.0 / 60.0, &mut live);
                frames += 1;
                assert!(frames < 1_000_000, "seed {seed}: sim never finishes");
            }
            assert_eq!(sorted(live), sorted(whole), "seed {seed}");
        }
    }

    #[test]
    fn heads_move_at_the_set_speed() {
        let scene = Scene::draw(5, &Choices::default());
        let d = Drawing::new(&scene, &Settings::default(), 320, 180);
        let mut sim = Sim::new(d, 20, 120.0);
        let mut out = Vec::new();
        for _ in 0..30 {
            sim.step(1.0 / 60.0, &mut out);
        }
        // 20 sparks × 2 heads × 60 px / 0.5 px at most; heads that stop or leave do less.
        assert!(out.len() <= 20 * 2 * 120 + 40, "{}", out.len());
        assert!(out.len() > 20 * 120 / 2, "{}", out.len());
    }

    #[test]
    fn glows_fade_in() {
        let scene = Scene::draw(5, &Choices::default());
        let d = Drawing::new(&scene, &Settings::default(), 320, 180);
        let mut sim = Sim::new(d, 20, 120.0);
        let mut out = Vec::new();
        sim.step(1.0 / 60.0, &mut out);
        sim.step(1.0 / 60.0, &mut out);
        let g: Vec<_> = sim.glows().collect();
        assert!(!g.is_empty());
        assert!(g.iter().all(|g| (0.0..0.2).contains(&g.intensity)), "{g:?}");
        assert!(g.iter().any(|g| g.intensity > 0.0), "{g:?}");
    }

    #[test]
    fn each_exit_from_the_screen_leaves_one_ghost() {
        use std::collections::HashMap;
        // A small view and a high speed make heads leave the screen and stop often; ten
        // frames of 10 ms stay under the ghost lifetime, so no ghost expires mid-test.
        let scene = Scene::draw(8, &Choices::default());
        let d = Drawing::new(&scene, &Settings::default(), 64, 36);
        let mut sim = Sim::new(d, 30, 3000.0);
        let mut out = Vec::new();
        let mut exits = 0;
        for _ in 0..10 {
            let before: HashMap<u64, bool> =
                sim.heads.iter().map(|h| (h.id, h.on_screen)).collect();
            let ghosts_before = sim.ghosts.len();
            sim.step(0.01, &mut out);
            let after: HashMap<u64, bool> = sim.heads.iter().map(|h| (h.id, h.on_screen)).collect();
            let left = before
                .iter()
                .filter(|&(id, &was_on)| was_on && !after.get(id).copied().unwrap_or(false))
                .count();
            // A head can also enter and leave within one frame; at least every visible
            // head that is no longer visible must have left a ghost.
            assert!(sim.ghosts.len() - ghosts_before >= left);
            exits += left;
        }
        assert!(
            exits > 0,
            "the test scene never had a head leave the screen"
        );
        assert!(sim.glows().all(|g| (0.0..=1.0).contains(&g.intensity)));
    }

    #[test]
    fn forward_only_traces_one_head() {
        let scene = Scene::draw(3, &Choices::default());
        let settings = Settings {
            trace_backward: false,
            ..Settings::default()
        };
        let d = Drawing::new(&scene, &settings, 320, 180);
        assert_eq!(d.heads(0.0, 0.0).count(), 1);
    }
}
