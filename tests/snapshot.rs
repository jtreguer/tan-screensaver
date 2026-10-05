//! GPU snapshots against images rendered by flow.js (tools/dump-fixtures.js).

use serde::Deserialize;
use tan_screensaver::field::{Field, SystemKind};
use tan_screensaver::integrate::{max_steps, View};
use tan_screensaver::palette::{self, ColourMode, PALETTES};
use tan_screensaver::render::{snapshot, Gpu};
use tan_screensaver::sim::{Drawing, Kernel};

/// Per channel, out of 255. GPU exp() and f32 maths differ slightly from V8's f64.
const TOLERANCE: u8 = 2;

#[derive(Deserialize)]
struct Images {
    image: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    name: String,
    system: String,
    params: Vec<f64>,
    cx: f64,
    cy: f64,
    span: f64,
    width_px: u32,
    height_px: u32,
    seeds: u32,
    seed: u32,
    arc: f64,
    step_px: f64,
    both: bool,
    line_width_px: f64,
    exposure: f64,
    kappa0: f64,
    colour_mode: String,
    palette: String,
    reverse: bool,
    background: String,
}

impl Image {
    fn drawing(&self) -> Drawing {
        let kind = SystemKind::from_name(&self.system).expect("known system");
        let view = View::new(self.width_px, self.height_px, self.cx, self.cy, self.span);
        let h_world = view.h_world(self.step_px);
        let bg =
            u32::from_str_radix(self.background.trim_start_matches('#'), 16).expect("hex colour");
        Drawing {
            field: Field::new(kind, &self.params).expect("parameter count"),
            view,
            step_px: self.step_px,
            h_world,
            max_steps: max_steps(self.arc, h_world),
            start_seed: self.seed,
            trajectories: self.seeds,
            trace_backward: self.both,
            colour_mode: if self.colour_mode == "signed" {
                ColourMode::Signed
            } else {
                ColourMode::Magnitude
            },
            kappa0: self.kappa0,
            lut: PALETTES[palette::find(&self.palette).expect("known palette")].lut(self.reverse),
            background: palette::rgb(bg),
            kernel: Kernel::new(self.line_width_px),
            exposure_step: self.exposure * self.step_px,
        }
    }
}

fn read_png(path: &str) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect(path),
    ));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("PNG size")];
    let info = reader.next_frame(&mut buf).expect("PNG data");
    assert_eq!(info.color_type, png::ColorType::Rgb, "{path}");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn snapshots_match_flow_js() {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let images: Images = toml::from_str(
        &std::fs::read_to_string(format!("{dir}/images.toml")).expect("images.toml"),
    )
    .expect("images.toml");
    let gpu = match Gpu::headless() {
        Ok(gpu) => gpu,
        Err(e) => {
            // Machines without a GPU (CI) cannot run this; say so loudly rather than fail.
            eprintln!("SKIPPED snapshots_match_flow_js: {e}");
            return;
        }
    };
    for img in &images.image {
        let (w, h, expected) = read_png(&format!("{dir}/images/{}.png", img.name));
        assert_eq!((w, h), (img.width_px, img.height_px), "{}", img.name);
        let actual = snapshot(&gpu, &img.drawing()).expect("snapshot");
        let mut worst = 0;
        let mut off = 0;
        for (i, (e, a)) in expected.chunks(3).zip(actual.chunks(4)).enumerate() {
            let d = (0..3).map(|c| e[c].abs_diff(a[c])).max().unwrap_or(0);
            worst = worst.max(d);
            if d > TOLERANCE {
                off += 1;
                if off <= 5 {
                    eprintln!(
                        "{}: pixel ({}, {}) expected {:?} got {:?}",
                        img.name,
                        i as u32 % w,
                        i as u32 / w,
                        e,
                        &a[..3]
                    );
                }
            }
        }
        eprintln!(
            "{}: worst channel difference {worst}, {off} pixels over tolerance",
            img.name
        );
        assert_eq!(
            off, 0,
            "{}: {off} pixels differ by more than {TOLERANCE}",
            img.name
        );
    }
}
