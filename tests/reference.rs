//! The port against flow.js: fixtures come from tools/dump-fixtures.js.

use serde::Deserialize;
use tan_screensaver::field::{Field, SystemKind};
use tan_screensaver::integrate::{max_steps, Head, View};
use tan_screensaver::palette::{colour_index, ColourMode, PALETTES};
use tan_screensaver::rng::Mulberry32;

const TOLERANCE: f64 = 1e-9;

#[derive(Deserialize)]
struct Trajectories {
    case: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    system: String,
    params: Vec<f64>,
    cx: f64,
    cy: f64,
    span: f64,
    width_px: u32,
    height_px: u32,
    step_px: f64,
    arc: f64,
    seeds: u32,
    seed: u32,
    both: bool,
    colour_mode: String,
    kappa0: f64,
    trace: Vec<Trace>,
}

#[derive(Deserialize)]
struct Trace {
    sx: f64,
    sy: f64,
    dir: f64,
    x: Vec<f64>,
    y: Vec<f64>,
    kappa: Vec<f64>,
    ci: Vec<u8>,
}

#[derive(Deserialize)]
struct PalettesRng {
    rng: Vec<RngCase>,
    palette: Vec<PaletteCase>,
}

#[derive(Deserialize)]
struct RngCase {
    seed: u32,
    values: Vec<f64>,
}

#[derive(Deserialize)]
struct PaletteCase {
    name: String,
    reverse: bool,
    index: Vec<usize>,
    rgb: Vec<f32>,
}

fn load<T: for<'de> Deserialize<'de>>(file: &str) -> T {
    let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE * a.abs().max(1.0)
}

#[test]
fn trajectories_match_flow_js() {
    let fixtures: Trajectories = load("trajectories.toml");
    assert!(!fixtures.case.is_empty());
    for c in &fixtures.case {
        let kind = SystemKind::from_name(&c.system).expect("known system");
        let field = Field::new(kind, &c.params).expect("parameter count");
        let view = View::new(c.width_px, c.height_px, c.cx, c.cy, c.span);
        let h = view.h_world(c.step_px);
        let n = max_steps(c.arc, h);
        let mode = if c.colour_mode == "signed" {
            ColourMode::Signed
        } else {
            ColourMode::Magnitude
        };

        let mut rng = Mulberry32::new(c.seed);
        let dirs: &[f64] = if c.both { &[1.0, -1.0] } else { &[1.0] };
        let mut expected = c.trace.iter();
        for _ in 0..c.seeds {
            let sx = view.xmin + rng.next_f64() * (view.xmax - view.xmin);
            let sy = view.ymin + rng.next_f64() * (view.ymax - view.ymin);
            for &dir in dirs {
                let t = expected
                    .next()
                    .unwrap_or_else(|| panic!("{}: missing trace", c.name));
                assert!(
                    sx == t.sx && sy == t.sy && dir == t.dir,
                    "{}: start point",
                    c.name
                );
                let mut head = Head::new(sx, sy, dir);
                let mut i = 0;
                while let Ok(s) = head.advance(&field, &view, h, n) {
                    let at = format!("{}: start ({sx}, {sy}) dir {dir} step {i}", c.name);
                    assert!(
                        i < t.x.len(),
                        "{at}: Rust trace is longer than {}",
                        t.x.len()
                    );
                    assert!(close(s.x, t.x[i]) && close(s.y, t.y[i]), "{at}: position");
                    assert!(
                        close(s.kappa, t.kappa[i]),
                        "{at}: kappa {} vs {}",
                        s.kappa,
                        t.kappa[i]
                    );
                    // tanh may differ by an ulp between libm and V8, which can move a .5 tie.
                    let ci = colour_index(s.kappa, mode, c.kappa0);
                    assert!(
                        ci.abs_diff(t.ci[i]) <= 1,
                        "{at}: colour {ci} vs {}",
                        t.ci[i]
                    );
                    i += 1;
                }
                assert_eq!(
                    i,
                    t.x.len(),
                    "{}: trace length, stopped by {:?}",
                    c.name,
                    head.stopped
                );
            }
        }
        assert!(
            expected.next().is_none(),
            "{}: extra traces in fixture",
            c.name
        );
    }
}

#[test]
fn mulberry32_matches_flow_js() {
    let fixtures: PalettesRng = load("palettes_rng.toml");
    for c in &fixtures.rng {
        let mut rng = Mulberry32::new(c.seed);
        for (i, &v) in c.values.iter().enumerate() {
            assert_eq!(rng.next_f64(), v, "seed {} value {i}", c.seed);
        }
    }
}

#[test]
fn palette_luts_match_flow_js() {
    let fixtures: PalettesRng = load("palettes_rng.toml");
    assert_eq!(fixtures.palette.len(), 2 * PALETTES.len());
    for c in &fixtures.palette {
        let p = PALETTES
            .iter()
            .find(|p| p.name == c.name)
            .expect("known palette");
        let lut = p.lut(c.reverse);
        for (k, &i) in c.index.iter().enumerate() {
            assert_eq!(
                lut[i],
                [c.rgb[3 * k], c.rgb[3 * k + 1], c.rgb[3 * k + 2]],
                "{} {} [{i}]",
                c.name,
                c.reverse
            );
        }
    }
}
