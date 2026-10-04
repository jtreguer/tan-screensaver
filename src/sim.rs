//! From a scene to splats. The spark pool for the live window will live here too.

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

    /// Start points in flow.js order: uniform over the view plus margin.
    pub fn start_points(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        let mut rng = Mulberry32::new(self.start_seed);
        let v = &self.view;
        (0..self.trajectories).map(move |_| {
            let sx = v.xmin + rng.next_f64() * (v.xmax - v.xmin);
            let sy = v.ymin + rng.next_f64() * (v.ymax - v.ymin);
            (sx, sy)
        })
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
