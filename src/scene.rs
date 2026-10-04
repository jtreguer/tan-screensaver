//! Random scene drawn from a 64-bit seed (SPEC §2, Scenes).

use crate::field::{Field, SystemKind};
use crate::palette::{self, ColourMode, PALETTES};
use crate::rng::SplitMix64;

const PRESET_CHANCE: f64 = 0.3;
const PRESET_JITTER: f64 = 0.1;
const SPAN_RANGE: (f64, f64) = (6.0, 30.0);
const REVERSE_CHANCE: f64 = 0.25;
const SIGNED_CHANCE: f64 = 0.3;
const KAPPA0_RANGE: (f64, f64) = (0.6, 3.0);
const PALETTE_BG_CHANCE: f64 = 0.5;
const PALETTE_BG_LUMA: f64 = 0.06;
const PLAIN_BG: u32 = 0x0b0c10;

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub seed: u64,
    pub field: Field,
    /// Name of the jittered preset, or None for a uniform draw.
    pub preset: Option<&'static str>,
    pub cx: f64,
    pub cy: f64,
    pub span: f64,
    /// Index into `PALETTES`.
    pub palette: usize,
    pub reversed: bool,
    pub colour_mode: ColourMode,
    pub kappa0: f64,
    pub background: [u8; 3],
    /// mulberry32 seed for the start points, as `seed` in the web app.
    pub start_seed: u32,
}

/// Systems and palettes a scene may use. Empty means all.
#[derive(Clone, Debug, Default)]
pub struct Choices {
    pub systems: Vec<SystemKind>,
    pub palettes: Vec<usize>,
}

impl Scene {
    /// The order of draws is part of the format: changing it changes every seed's scene.
    pub fn draw(seed: u64, choices: &Choices) -> Scene {
        let mut rng = SplitMix64::new(seed);

        let systems: &[SystemKind] = if choices.systems.is_empty() {
            &SystemKind::ALL
        } else {
            &choices.systems
        };
        let kind = systems[rng.below(systems.len())];
        let specs = kind.params();

        let (params, preset, cx, cy, span);
        if rng.chance(PRESET_CHANCE) {
            let p = &kind.presets()[rng.below(kind.presets().len())];
            params = p
                .params
                .iter()
                .zip(specs)
                .map(|(&v, s)| {
                    (v * rng.range(1.0 - PRESET_JITTER, 1.0 + PRESET_JITTER)).clamp(s.min, s.max)
                })
                .collect::<Vec<_>>();
            (preset, cx, cy, span) = (Some(p.name), p.cx, p.cy, p.span);
        } else {
            params = specs
                .iter()
                .map(|s| rng.range(s.scene_min, s.scene_max))
                .collect();
            preset = None;
            span = rng.log_range(SPAN_RANGE.0, SPAN_RANGE.1);
            cx = rng.range(-std::f64::consts::PI, std::f64::consts::PI);
            cy = rng.range(-std::f64::consts::PI, std::f64::consts::PI);
        }
        let field = Field::new(kind, &params).expect("one value per parameter spec");

        let all_palettes: Vec<usize> = (0..PALETTES.len()).collect();
        let palettes = if choices.palettes.is_empty() {
            &all_palettes
        } else {
            &choices.palettes
        };
        let palette = palettes[rng.below(palettes.len())];
        let reversed = rng.chance(REVERSE_CHANCE);
        let colour_mode = if rng.chance(SIGNED_CHANCE) {
            ColourMode::Signed
        } else {
            ColourMode::Magnitude
        };
        let kappa0 = rng.log_range(KAPPA0_RANGE.0, KAPPA0_RANGE.1);
        let background = if rng.chance(PALETTE_BG_CHANCE) {
            palette_background(palette)
        } else {
            palette::rgb(PLAIN_BG)
        };
        let start_seed = (rng.next_u64() >> 32) as u32;

        Scene {
            seed,
            field,
            preset,
            cx,
            cy,
            span,
            palette,
            reversed,
            colour_mode,
            kappa0,
            background,
            start_seed,
        }
    }
}

fn luma(c: [u8; 3]) -> f64 {
    (0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64) / 255.0
}

/// The palette's darkest stop, darkened to about 6% luma; never brightened.
fn palette_background(palette: usize) -> [u8; 3] {
    let darkest = PALETTES[palette]
        .stops
        .iter()
        .map(|&h| palette::rgb(h))
        .min_by(|a, b| luma(*a).total_cmp(&luma(*b)))
        .unwrap_or([0, 0, 0]);
    let y = luma(darkest);
    let scale = if y > PALETTE_BG_LUMA {
        PALETTE_BG_LUMA / y
    } else {
        1.0
    };
    darkest.map(|c| (c as f64 * scale).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_scene() {
        let c = Choices::default();
        for seed in [0, 1, 42, u64::MAX] {
            assert_eq!(Scene::draw(seed, &c), Scene::draw(seed, &c));
        }
        assert_ne!(Scene::draw(1, &c), Scene::draw(2, &c));
    }

    #[test]
    fn draws_stay_in_range_and_use_everything() {
        let c = Choices::default();
        let (mut presets, mut systems, mut palettes, mut signed) = (0, [0; 2], [0; 9], 0);
        for seed in 0..2000 {
            let s = Scene::draw(seed, &c);
            let kind = s.field.kind();
            systems[SystemKind::ALL.iter().position(|&k| k == kind).unwrap()] += 1;
            palettes[s.palette] += 1;
            signed += (s.colour_mode == ColourMode::Signed) as u32;
            assert!((0.6..=3.0).contains(&s.kappa0));
            assert!(luma(s.background) <= PALETTE_BG_LUMA + 0.01);
            for (v, spec) in s.field.params().iter().zip(kind.params()) {
                if s.preset.is_some() {
                    assert!((spec.min..=spec.max).contains(v));
                } else {
                    assert!((spec.scene_min..=spec.scene_max).contains(v));
                }
            }
            if s.preset.is_some() {
                presets += 1;
            } else {
                assert!((6.0..=30.0).contains(&s.span));
                assert!(s.cx.abs() <= std::f64::consts::PI && s.cy.abs() <= std::f64::consts::PI);
            }
        }
        assert!((500..700).contains(&presets), "{presets}");
        assert!((500..700).contains(&signed), "{signed}");
        assert!(systems.iter().all(|&n| n > 900));
        assert!(palettes.iter().all(|&n| n > 150));
    }

    #[test]
    fn jittered_presets_never_reach_the_tan_singularity() {
        let c = Choices {
            systems: vec![SystemKind::Whirlpools],
            palettes: vec![],
        };
        for seed in 0..2000 {
            if let Field::Whirlpools { k, .. } = Scene::draw(seed, &c).field {
                assert!(k < std::f64::consts::FRAC_PI_2);
            }
        }
    }

    #[test]
    fn choices_restrict_the_draw() {
        let c = Choices {
            systems: vec![SystemKind::Spirals],
            palettes: vec![3, 5],
        };
        for seed in 0..200 {
            let s = Scene::draw(seed, &c);
            assert_eq!(s.field.kind(), SystemKind::Spirals);
            assert!(s.palette == 3 || s.palette == 5);
        }
    }
}
