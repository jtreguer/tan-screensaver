//! Per-scene speed from a target duration (SPEC §2, Scene length).

use crate::config::{Settings, REF_HEIGHT_PX};
use crate::scene::Scene;
use crate::sim::Drawing;

/// Start points traced for the length estimate. A coarser step would be cheaper per
/// trajectory but overshoots sinks and keeps circling them, where the real step stops.
pub const SAMPLE: u32 = 192;

/// On-screen path length of every half-trajectory of the drawing, in pixels, estimated
/// from an evenly spaced sample of start points traced at the real step. Off-screen
/// stretches are skipped live, so they do not count.
pub fn visible_length_px(drawing: &Drawing) -> f64 {
    let stride = drawing.trajectories.div_ceil(SAMPLE).max(1) as usize;
    let (mut traced, mut steps) = (0u64, 0u64);
    for (sx, sy) in drawing.start_points().step_by(stride) {
        traced += 1;
        for mut head in drawing.heads(sx, sy) {
            while let Ok(splat) = drawing.advance(&mut head) {
                steps += splat.is_some() as u64;
            }
        }
    }
    if traced == 0 {
        return 0.0;
    }
    steps as f64 * drawing.step_px * drawing.trajectories as f64 / traced as f64
}

/// Speed in px/s so the scene lasts about `scene_seconds`, within the configured range.
pub fn speed_px(length_px: f64, settings: &Settings, height_px: u32) -> f64 {
    let scale = height_px as f64 / REF_HEIGHT_PX;
    let heads_per_spark = if settings.trace_backward { 2.0 } else { 1.0 };
    let heads = settings.sparks.min(settings.trajectories) as f64 * heads_per_spark;
    let speed = length_px / (heads * settings.scene_seconds);
    speed.clamp(settings.min_speed * scale, settings.max_speed * scale)
}

/// A scene ready to run on one screen.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub scene: Scene,
    pub drawing: Drawing,
    pub length_px: f64,
    pub speed_px: f64,
}

impl Prepared {
    /// Traces a sample of the scene, so the live window runs it on a worker thread.
    pub fn new(scene: Scene, settings: &Settings, width_px: u32, height_px: u32) -> Prepared {
        let drawing = Drawing::new(&scene, settings, width_px, height_px);
        let length_px = visible_length_px(&drawing);
        let speed_px = speed_px(length_px, settings, height_px);
        Prepared {
            scene,
            drawing,
            length_px,
            speed_px,
        }
    }

    /// Expected duration at the chosen speed, for the log.
    pub fn expected_seconds(&self, settings: &Settings) -> f64 {
        let heads_per_spark = if settings.trace_backward { 2.0 } else { 1.0 };
        let heads = settings.sparks.min(settings.trajectories) as f64 * heads_per_spark;
        self.length_px / (heads * self.speed_px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Choices;

    #[test]
    fn sampled_length_is_close_to_the_full_one() {
        for seed in [1, 2, 3, 10, 11] {
            let scene = Scene::draw(seed, &Choices::default());
            let settings = Settings {
                trajectories: 600,
                ..Settings::default()
            };
            let d = Drawing::new(&scene, &settings, 480, 270);
            let mut full = 0;
            d.trace_all(|_| full += 1);
            let full_px = full as f64 * d.step_px;
            let ratio = visible_length_px(&d) / full_px;
            assert!((0.85..1.15).contains(&ratio), "seed {seed}: ratio {ratio}");
        }
    }

    #[test]
    fn speed_hits_the_target_unless_clamped() {
        let s = Settings::default();
        // 160 heads for 90 s at 300 px/s.
        let length = 160.0 * 90.0 * 300.0;
        assert!((speed_px(length, &s, 1440) - 300.0).abs() < 1e-9);
        assert_eq!(speed_px(length, &s, 720), 300.0);
        assert_eq!(speed_px(1.0, &s, 1440), 120.0);
        assert_eq!(speed_px(1e12, &s, 1080), 450.0);
    }
}
