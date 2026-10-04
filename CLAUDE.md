# CLAUDE.md: tan-screensaver

A native screensaver for Omarchy (Hyprland, Arch Linux). Glowing sparks trace the
trajectories of the `tan()` flow fields from the sibling project tan-trajectories; each
scene picks a random system, parameters and palette, runs until the screen is filled
enough, holds, fades and starts over.

**`SPEC.md` is the specification.** Read it before any non-trivial change. When a
decision changes behaviour described there, update `SPEC.md` in the same change.

## Source of the maths

The equations, integrator, curvature colouring, palettes and tone mapping come from
`~/Work/web_dev/tan-trajectories/flow.js` (also on GitHub at jtreguer/tan-trajectories).
Treat that file as the reference implementation: port it faithfully and keep the
reference-trajectory tests passing. Do not edit the sibling repository from here.

## Stack

- Rust (stable, edition 2021), `winit` for windows and input, `wgpu` for rendering,
  WGSL shaders.
- Keep dependencies few. Expected: `winit`, `wgpu`, `pollster`, `bytemuck`, `png` (or
  `image` with only the PNG feature), `serde` + `toml` for config, `clap` for flags, a
  small seedable PRNG (`rand_pcg` or a hand-written PCG/SplitMix). Ask before adding
  anything outside this list.
- Rust is not installed on this machine yet. Install with `rustup` (or
  `mise use -g rust`), not the Arch `rust` package.

## Commands

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

cargo run -- --windowed                      # development window, Escape quits
cargo run --release -- --snapshot out.png --seed 42 --size 2560x1440
```

All four checks (build, test, clippy, fmt) must pass before a commit.

## Layout

```
src/
  main.rs        CLI parsing, dispatch to app or snapshot
  lib.rs         the pure modules, shared by the binary and the tests
  field.rs       equation systems, ranges, presets (pure)
  integrate.rs   RK4, curvature, stop conditions (pure)
  palette.rs     palette stops, LUTs, curvature colouring (pure)
  rng.rs         SplitMix64 for scenes, mulberry32 for start points (pure)
  scene.rs       random scene from a seed (pure)
  sim.rs         spark pool and per-frame stepping (pure)
  budget.rs      path-length pre-pass, per-scene speed (pure)
  render/        wgpu pipelines and WGSL shaders
  app.rs         winit event loop, windows, input, exit
  config.rs      config file and flags
scripts/         launcher, install, Omarchy hook install
tests/fixtures/  reference trajectories, LUTs and RNG output dumped from flow.js
tools/           dump-fixtures.js: `node tools/dump-fixtures.js` regenerates the fixtures
```

The pure modules must not import `wgpu` or `winit`.

## Conventions

- Determinism: every random choice in a scene comes from the scene's seed. Never use a
  thread-local or time-based RNG inside a scene. The snapshot mode depends on this.
- Units: world units for the maths, physical pixels for rendering. Sizes the user sets
  (line width, speed) are given for a 1440 px tall screen and scaled with the real height.
  Name variables so the unit is clear (`step_px`, `h_world`, `ppu`).
- No `unwrap()` outside tests and startup; a screensaver that panics leaves the user
  staring at a frozen frame. Log errors and exit cleanly.
- Comments explain why, not what. Keep them as sparse as the sibling project's `flow.js`.

## Verifying visual changes

You cannot see the live screensaver. To check a rendering change, run the snapshot mode
with a fixed seed and read the PNG. Compare before and after with the same seed. For
changes to scene ranges, render a batch of seeds (say 12) and look at all of them.

Running the real fullscreen screensaver grabs every monitor and exits on input; only do
that when asked, and use `--windowed` otherwise.

## Omarchy integration rules

- Wayland app_id must be `org.omarchy.screensaver`. Omarchy's window rules, its idle
  service and the lock handoff all key on it.
- Never edit anything under `/usr/share/omarchy`. User-level changes go in
  `~/.config/omarchy/` (plugins, `shell.json`, menu extensions) and binaries in
  `~/.local/bin`.
- Scripts that change `~/.config/omarchy/shell.json` back it up first and are run only
  when the user asks.
- Omarchy's own scripts (`omarchy-launch-screensaver`, `omarchy-screensaver`, the idle
  plugin) are the model for behaviour; read them in `/usr/share/omarchy` rather than
  guessing. `SPEC.md` §5 summarises them.

## Git

Commit or push only when asked. The repository may not be initialised yet; if it is not,
ask before running `git init`.
