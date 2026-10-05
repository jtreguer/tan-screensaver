use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use tan_screensaver::app;
use tan_screensaver::config::Settings;
use tan_screensaver::field::SystemKind;
use tan_screensaver::render::{save_png, snapshot, Gpu};
use tan_screensaver::rng::SplitMix64;
use tan_screensaver::scene::{Choices, Scene};
use tan_screensaver::sim::Drawing;

/// Glowing sparks tracing the trajectories of tan() flow fields.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Run in a normal window instead of fullscreen; only Escape or closing it exits.
    #[arg(long)]
    windowed: bool,
    /// Render one scene offscreen and write it to this PNG, then exit.
    #[arg(long, value_name = "PATH")]
    snapshot: Option<PathBuf>,
    /// Snapshot size in pixels.
    #[arg(long, value_name = "WxH", default_value = "2560x1440", value_parser = parse_size)]
    size: (u32, u32),
    /// First scene seed; random when omitted.
    #[arg(long)]
    seed: Option<u64>,
    /// Use only this system: whirlpools or spirals.
    #[arg(long, value_parser = parse_system)]
    system: Option<SystemKind>,
    /// Start points per scene.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    trajectories: Option<u32>,
    /// Start points in flight per screen.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    sparks: Option<u32>,
    /// Only open on this monitor (connector name, as in `hyprctl monitors`).
    #[arg(long, value_name = "NAME")]
    output: Option<String>,
    /// Config file instead of ~/.config/tan-screensaver/config.toml.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or("expected WxH, e.g. 2560x1440")?;
    let parse = |v: &str| v.parse::<u32>().ok().filter(|&n| (1..=16384).contains(&n));
    match (parse(w), parse(h)) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err("width and height must be between 1 and 16384".into()),
    }
}

fn parse_system(s: &str) -> Result<SystemKind, String> {
    SystemKind::from_name(s).ok_or_else(|| {
        let names: Vec<_> = SystemKind::ALL.iter().map(|k| k.name()).collect();
        format!("unknown system, expected one of {}", names.join(", "))
    })
}

/// Scene seeds may come from the clock; nothing inside a scene does.
fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    SplitMix64::new(nanos ^ (std::process::id() as u64) << 32).next_u64()
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (mut settings, warning) = Settings::load(cli.config.as_deref());
    if let Some(w) = warning {
        eprintln!("tan-screensaver: config ignored, using defaults: {w}");
    }
    if let Some(n) = cli.trajectories {
        settings.trajectories = n;
    }
    if let Some(n) = cli.sparks {
        settings.sparks = n;
    }
    let (mut choices, warnings) = settings.choices();
    for w in warnings {
        eprintln!("tan-screensaver: config: {w}");
    }
    if let Some(kind) = cli.system {
        choices.systems = vec![kind];
    }
    let seed = cli.seed.unwrap_or_else(random_seed);

    let result = match &cli.snapshot {
        Some(path) => run_snapshot(path, seed, &settings, &choices, cli.size),
        None => app::run(app::Options {
            seed,
            settings,
            choices,
            windowed: cli.windowed,
            output: cli.output,
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tan-screensaver: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_snapshot(
    path: &Path,
    seed: u64,
    settings: &Settings,
    choices: &Choices,
    (w, h): (u32, u32),
) -> Result<(), String> {
    let scene = Scene::draw(seed, choices);
    eprintln!("scene {scene}");
    let started = Instant::now();
    Gpu::headless()
        .and_then(|gpu| snapshot(&gpu, &Drawing::new(&scene, settings, w, h)))
        .and_then(|rgba| save_png(path, w, h, &rgba))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "wrote {} in {:.1} s",
        path.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
