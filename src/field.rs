//! Equation systems, parameter ranges and presets, ported from flow.js.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemKind {
    Whirlpools,
    Spirals,
}

pub struct ParamSpec {
    pub name: &'static str,
    /// Web app slider range. Jittered presets are clamped to it: past k or a ≈ π/2 the
    /// tan() argument reaches its singularity.
    pub min: f64,
    pub max: f64,
    /// Range for uniform scene draws, narrower than the sliders to avoid empty or
    /// degenerate pictures (SPEC §2). First guess, to be tuned from snapshots.
    pub scene_min: f64,
    pub scene_max: f64,
}

pub struct Preset {
    pub name: &'static str,
    pub field: Field,
    pub cx: f64,
    pub cy: f64,
    pub span: f64,
}

#[rustfmt::skip]
const WHIRLPOOL_PARAMS: [ParamSpec; 3] = [
    ParamSpec { name: "k", min: 0.2, max: 1.55, scene_min: 0.9, scene_max: 1.5 },
    ParamSpec { name: "w", min: 0.2, max: 3.0, scene_min: 0.6, scene_max: 2.2 },
    ParamSpec { name: "d", min: 0.0, max: 0.6, scene_min: 0.0, scene_max: 0.3 },
];

#[rustfmt::skip]
const SPIRAL_PARAMS: [ParamSpec; 2] = [
    ParamSpec { name: "a", min: 0.05, max: 1.55, scene_min: 0.2, scene_max: 1.35 },
    ParamSpec { name: "b", min: 0.0, max: 2.0, scene_min: 0.4, scene_max: 1.6 },
];

#[rustfmt::skip]
const WHIRLPOOL_PRESETS: [Preset; 6] = [
    Preset { name: "Whirlpools (original)", field: Field::Whirlpools { k: 1.3, w: 1.0, d: 0.15 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Slow drift", field: Field::Whirlpools { k: 1.45, w: 1.0, d: 0.05 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Tight vortices", field: Field::Whirlpools { k: 1.0, w: 1.0, d: 0.3 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Wide field", field: Field::Whirlpools { k: 1.3, w: 1.0, d: 0.08 }, cx: 0.0, cy: 0.0, span: 30.0 },
    Preset { name: "High frequency", field: Field::Whirlpools { k: 1.4, w: 2.2, d: 0.12 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Off-centre close-up", field: Field::Whirlpools { k: 1.35, w: 1.0, d: 0.1 }, cx: 3.2, cy: 3.0, span: 7.0 },
];

#[rustfmt::skip]
const SPIRAL_PRESETS: [Preset; 6] = [
    Preset { name: "Spiral garden (original)", field: Field::Spirals { a: 0.6, b: 1.0 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Gentle", field: Field::Spirals { a: 0.3, b: 1.0 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Strong shear", field: Field::Spirals { a: 1.0, b: 1.0 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Shear-dominated", field: Field::Spirals { a: 1.3, b: 0.5 }, cx: 0.0, cy: 0.0, span: 12.6 },
    Preset { name: "Wide field", field: Field::Spirals { a: 0.6, b: 1.0 }, cx: 0.0, cy: 0.0, span: 30.0 },
    Preset { name: "Close-up", field: Field::Spirals { a: 0.7, b: 1.0 }, cx: 0.8, cy: -0.8, span: 6.0 },
];

impl SystemKind {
    pub const ALL: [SystemKind; 2] = [SystemKind::Whirlpools, SystemKind::Spirals];

    pub fn name(self) -> &'static str {
        match self {
            SystemKind::Whirlpools => "whirlpools",
            SystemKind::Spirals => "spirals",
        }
    }

    pub fn from_name(name: &str) -> Option<SystemKind> {
        SystemKind::ALL.into_iter().find(|s| s.name() == name)
    }

    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            SystemKind::Whirlpools => &WHIRLPOOL_PARAMS,
            SystemKind::Spirals => &SPIRAL_PARAMS,
        }
    }

    pub fn presets(self) -> &'static [Preset] {
        match self {
            SystemKind::Whirlpools => &WHIRLPOOL_PRESETS,
            SystemKind::Spirals => &SPIRAL_PRESETS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Whirlpools { k: f64, w: f64, d: f64 },
    Spirals { a: f64, b: f64 },
}

impl Field {
    /// Builds a field by asking `value` for each parameter, in the order of
    /// `kind.params()`.
    pub fn from_fn(kind: SystemKind, mut value: impl FnMut(&ParamSpec) -> f64) -> Field {
        match kind {
            SystemKind::Whirlpools => {
                let [k, w, d] = &WHIRLPOOL_PARAMS;
                Field::Whirlpools {
                    k: value(k),
                    w: value(w),
                    d: value(d),
                }
            }
            SystemKind::Spirals => {
                let [a, b] = &SPIRAL_PARAMS;
                Field::Spirals {
                    a: value(a),
                    b: value(b),
                }
            }
        }
    }

    /// Applies `f` to each parameter, in the order of `kind.params()`.
    pub fn map(&self, mut f: impl FnMut(&ParamSpec, f64) -> f64) -> Field {
        let mut values = self.params().into_iter();
        Field::from_fn(self.kind(), |spec| {
            f(spec, values.next().unwrap_or(spec.min))
        })
    }

    /// `params` in the order of `kind.params()`; returns None on a wrong count.
    pub fn new(kind: SystemKind, params: &[f64]) -> Option<Field> {
        match (kind, params) {
            (SystemKind::Whirlpools, &[k, w, d]) => Some(Field::Whirlpools { k, w, d }),
            (SystemKind::Spirals, &[a, b]) => Some(Field::Spirals { a, b }),
            _ => None,
        }
    }

    pub fn kind(&self) -> SystemKind {
        match self {
            Field::Whirlpools { .. } => SystemKind::Whirlpools,
            Field::Spirals { .. } => SystemKind::Spirals,
        }
    }

    pub fn params(&self) -> Vec<f64> {
        match *self {
            Field::Whirlpools { k, w, d } => vec![k, w, d],
            Field::Spirals { a, b } => vec![a, b],
        }
    }

    /// Same operation order as flow.js, so results match it bit for bit where the
    /// platform's sin and tan agree with V8's.
    #[inline]
    pub fn eval(&self, x: f64, y: f64) -> (f64, f64) {
        match *self {
            Field::Whirlpools { k, w, d } => (
                (k * (w * y).sin()).tan() - d * x,
                -(k * (w * x).sin()).tan() - d * y,
            ),
            Field::Spirals { a, b } => (
                b * y.sin() + (a * (x + y).cos()).tan(),
                -b * x.sin() + (a * (x - y).sin()).tan(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_their_system_and_slider_ranges() {
        for kind in SystemKind::ALL {
            for p in kind.presets() {
                assert_eq!(p.field.kind(), kind, "{}", p.name);
                for (v, spec) in p.field.params().iter().zip(kind.params()) {
                    assert!(
                        (spec.min..=spec.max).contains(v),
                        "{} {}",
                        p.name,
                        spec.name
                    );
                }
            }
        }
    }

    #[test]
    fn scene_ranges_lie_within_slider_ranges() {
        for kind in SystemKind::ALL {
            for s in kind.params() {
                assert!(s.min <= s.scene_min && s.scene_min < s.scene_max && s.scene_max <= s.max);
            }
        }
    }

    #[test]
    fn names_round_trip() {
        for kind in SystemKind::ALL {
            assert_eq!(SystemKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(SystemKind::from_name("lorenz"), None);
    }
}
