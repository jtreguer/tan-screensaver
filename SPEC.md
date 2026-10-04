# tan-screensaver: specification

A screensaver for Omarchy (Hyprland on Arch) that draws the trajectories of the
`tan()` flow fields from [tan-trajectories](https://github.com/jtreguer/tan-trajectories)
live on screen. Glowing sparks move along the trajectories and leave a trail behind them.
Each scene uses a random equation system, random parameters and a random palette. When the
screen is filled enough, the finished image is held for a moment, fades out, and a new
scene starts.

## 1. Source material

The maths comes from the sibling repository `~/Work/web_dev/tan-trajectories`, file
`flow.js`. That file is the reference for anything not spelled out here. The relevant
parts:

### Equation systems

```
whirlpools:  x' =  tan(k·sin(ω·y)) − d·x
             y' = −tan(k·sin(ω·x)) − d·y

spirals:     x' =  b·sin(y) + tan(a·cos(x+y))
             y' = −b·sin(x) + tan(a·sin(x−y))
```

Presets for both systems (parameters plus view `cx`, `cy`, `span`) are listed in `flow.js`
and should be carried over as data.

### Integration

- The field is normalised to unit speed, so every trajectory advances at the same rate.
- Fourth-order Runge-Kutta with a fixed step `h` in world units, derived from a step in
  pixels: `h = step_px / ppu`, where `ppu = H / span` is pixels per world unit.
- A trajectory stops when the field magnitude drops below `1e-4` or becomes NaN (fixed
  point or `tan` singularity), when it leaves the view plus a 15% margin, or when it
  reaches its maximum arc length.
- Signed curvature at each step is the turn of the unit tangent over half a step:
  `κ = (k1 × k2) / (h/2)`, where `k1`, `k2` are the first two RK4 unit tangents. The
  half step is taken in the direction of travel but the division uses the unsigned `h`,
  so the sign follows the direction of travel: the backward half of a trajectory has the
  opposite sign to the forward half at the same place.

### Colour and accumulation

- Curvature maps to a palette position `t`:
  magnitude mode `t = tanh(|κ| / κ0)`, signed mode `t = 0.5 + 0.5·tanh(κ / κ0)`.
- Palettes are 256-entry lookup tables interpolated linearly between the stops listed in
  `PALETTES` (Magma, Viridis, Ocean, Ember, Aurora, Rose quartz, Neon, Gold, Ink).
- Every step deposits a Gaussian splat into an accumulation buffer holding
  `(r·w, g·w, b·w, w)`. Sigma is `0.45 × line width`, the kernel radius `ceil(2.2·σ)`,
  and weights below 0.01 are skipped.
- Tone mapping per pixel: `a = 1 − exp(−exposure · step_px · w)`, colour
  `= bg + (rgb/w − bg) · a`. Dense regions become opaque, sparse ones fade into the
  background.
- Line width is given for a 1440 px tall output and scaled with the actual height.

### How much is drawn

The web app does not measure coverage. A render is a fixed amount of ink: `seeds` start
points (default 1500) drawn uniformly over the view plus margin by `mulberry32(seed)`,
each traced forward and then, with "trace backward" on (the default), backward in time,
for at most the max arc length in each direction. The downloaded PNG uses a step of
0.5 px; exposure is multiplied by the step, so the look barely depends on it.

Web app defaults, a good starting point: 1500 trajectories, both directions, line width
1.2, exposure 0.35, κ0 1.5, max arc length 30 world units, step 0.5 px.

## 2. What the screensaver does

### Scenes

A scene is one random draw of:

| Field | How it is chosen |
|---|---|
| System | uniform over the enabled systems |
| Parameters | 30% of the time a preset with ±10% jitter on each parameter; otherwise uniform within the scene ranges below |
| View | `span` log-uniform in [6, 30]; `cx`, `cy` uniform in [−π, π] (the fields are periodic or near-periodic, so offsets give variety) |
| Palette | uniform over the enabled palettes; reversed with probability 0.25 |
| Colour mode | magnitude 70%, signed 30% |
| κ0 | log-uniform in [0.6, 3] |
| Background | near-black, taken from the palette's darkest stop scaled down to about 6% luminance, or plain `#0b0c10` |

Scene ranges are narrower than the web app's slider ranges, to avoid empty or degenerate
pictures:

| System | Parameter | Range |
|---|---|---|
| whirlpools | k | 0.9 – 1.5 |
| whirlpools | ω | 0.6 – 2.2 |
| whirlpools | d | 0 – 0.3 |
| spirals | a | 0.2 – 1.35 |
| spirals | b | 0.4 – 1.6 |

The ranges are a first guess. Tune them by looking at snapshots (see §5) and record the
reason for any change in a comment next to the table in the code.

Each scene has a 64-bit seed. All random choices for the scene come from a PRNG seeded
with it, so a scene can be reproduced from its seed alone. The seed is logged at scene
start. Start points are the exception to the PRNG choice: the scene PRNG draws a u32, and
the start points come from `mulberry32` seeded with it, in the same order as `flow.js`.
Splats add up in any order, so the finished screen equals the web app's render of the
same system, parameters, view, palette and u32 seed, up to float rounding. The snapshot
tests rely on this.

### Sparks

- A spark is one start point being traced live. A scene keeps `N` sparks in flight
  (default 80, configurable).
- Every spark has two heads leaving the start point at the same moment, one integrating
  with `+h` and one with `−h`, so the curve grows out in both directions. This is the web
  app's "trace backward" option. Tracing backward can be turned off in the config, which
  leaves one head per spark. Each head stops on its own (stop conditions in §1, max arc
  length per direction); the spark is done when all its heads are.
- Start points are taken in order from the `mulberry32` stream. When a spark is done, the
  next start point is taken, so `N` sparks stay in flight until all `trajectories` start
  points (default 1500) have been used.
- All heads move at the same on-screen speed, set per scene (see "Scene length" below).
  Each frame a head advances by `speed · dt` pixels, split into RK4 steps of 0.5 px, and
  deposits one splat per step into the accumulation buffer.
- About a third of the start area is the margin outside the screen. Any stretch of a
  head's path that lies outside the screen is integrated at once, without waiting for
  frames. Its splats fall off screen and would be clipped anyway, so the final image does
  not change, and spark slots do not sit idle on invisible work. A head that comes back
  into the screen continues at normal speed from the point where it enters.
- Each spark's head is drawn on top of the image as a glow: a small bright core in the
  current palette colour pushed towards white, plus a wider soft halo with additive
  blending. The halo is not written into the accumulation buffer. A short fading tail
  (the last ~25 positions) can be drawn the same way if it looks better; decide by eye.
- A newly spawned spark fades its head in over ~0.3 s and a dying one fades out, so
  sparks do not pop.

### Filling and scene end

The screen is filled by the same rule as the web app: a scene draws a fixed amount of
ink, `trajectories` start points traced up to `arc_length` in each direction, and ends
when the last head has stopped. There is no coverage measurement.

### Scene length

With a fixed amount of ink, the time a scene takes depends on its view. At 1440 px
height, 30 world units of arc is about 1,440 px at span 30 and 7,200 px at span 6; at a
fixed 220 px/s with 160 heads a scene would last anywhere from about 2 to 10 minutes. So
the speed is set per scene to aim at a target duration (default 90 s):

1. At scene start, a CPU pass integrates every half-trajectory at a coarse step (about
   4 px) without splatting, and adds up the on-screen path length `L_px`. Off-screen
   stretches are skipped live (see Sparks), so they do not count.
2. `speed = L_px / (heads_in_flight · scene_seconds)`, clamped to
   [`min_speed`, `max_speed`] (defaults 120 and 600 px/s at 1440 px height, scaled with
   the height). The clamp means some scenes run shorter or longer than the target; the
   final image is never cut short.
3. The pass is pure and deterministic. It runs on a worker thread, started for the next
   scene while the current one holds and fades, so the next scene can start without a
   pause. It is an estimate: sparks finishing at different times leave the last seconds
   with fewer heads in flight, which is acceptable.

When the scene ends:

1. The last heads fade out as they stop.
2. The finished image is held still (default 6 s).
3. It fades to the background over 2.5 s.
4. The accumulation buffer is cleared and the next scene starts.

While holding, the app should stop redrawing until the fade starts, to save power.

### Exiting

The screensaver exits, closing all its windows, on:

- any key press,
- any mouse button or scroll,
- pointer motion of more than 10 px from the first position seen (Wayland sends a motion
  event when the pointer enters the surface; that one must not count), ignoring all motion
  in the first 500 ms,
- loss of focus of all its windows (the lock screen taking over, for instance), matching
  what `omarchy-screensaver` does,
- SIGINT, SIGTERM, SIGHUP.

The cursor is hidden over the screensaver windows.

### Multiple monitors

One process opens one fullscreen window per monitor, each running its own independent
scene sized to that monitor's physical pixels (this machine has 1920×1080 and 2560×1440,
both at scale 1.25; render at physical resolution, not logical). Input on any window ends
all of them.

If Hyprland's window rules for `org.omarchy.screensaver` (fullscreen + float) fight with
per-output fullscreen requests, fall back to what Omarchy does: the launcher focuses each
monitor in turn and starts one process per monitor with `--output <name>`, and a process
that exits kills its siblings.

## 3. Architecture

### Stack

Rust, with `winit` for windows and input and `wgpu` for rendering. A native binary starts
in milliseconds, uses little memory while idle, and the GPU does the splatting and tone
mapping, which a CPU loop cannot do at 60 fps on a 1440p screen. The alternative
considered was C with SDL3 and OpenGL (both already installed here); Rust was chosen for
the safer codebase and the easier dependency handling through cargo.

### Modules

| Module | Responsibility |
|---|---|
| `field` | Equation systems, parameter ranges, presets. Pure functions. |
| `integrate` | RK4 step, curvature, stop conditions. Pure. |
| `palette` | Palette stops and 256-entry LUTs. |
| `scene` | Random scene draw from a seed. |
| `sim` | Spark pool, start-point stream, per-frame advancement, off-screen skipping; produces the list of splats for the frame. No GPU code. |
| `budget` | Path-length pre-pass and the per-scene speed. |
| `render` | wgpu pipelines: splat pass, tone-map pass, glow pass, fade. |
| `app` | winit event loop, one state per window, input handling, exit. |
| `config` | Config file and CLI flags. |

`field`, `integrate`, `palette`, `scene`, `sim` and `budget` must not depend on wgpu or
winit, so they can be unit tested and used by the snapshot mode.

### Rendering

Per window:

1. **Accumulation texture**, `Rgba32Float` (or `Rgba16Float` if 32-bit float blending is
   unavailable), at the window's physical size, holding `(r·w, g·w, b·w, w)`.
2. **Splat pass**: the frame's splats are uploaded as an instance buffer
   (pixel position, LUT index; the LUT is a uniform) and drawn as quads covering the
   pixels `round(p) ± r`, with additive blending; the fragment shader computes the
   Gaussian weight and discards below 0.01. As in `flow.js`, pixel `i` is centred on
   coordinate `i`. Splats whose footprint misses the screen are dropped on the CPU. The
   accumulation texture is never cleared during a scene.
3. **Tone-map pass**: full-screen triangle reading the accumulation texture, applying the
   formula from §1 and the scene fade factor, writing to the swapchain.
4. **Glow pass**: spark heads (and tails, if used) as additive quads on top.

The tone-map pass writes raw values like `flow.js` does, so the swapchain must use a
non-sRGB format (or a non-sRGB view of it); an sRGB target would gamma-encode the image
and the screen would no longer match the snapshot.

Present mode `Fifo` (vsync). Clamp `dt` to 50 ms so a stalled frame does not make sparks
jump.

## 4. Configuration

`~/.config/tan-screensaver/config.toml`, every key optional:

```toml
sparks = 80              # start points in flight per screen (two heads each)
trajectories = 1500      # start points per scene, as in the web app
arc_length = 30          # world units per direction
trace_backward = true
scene_seconds = 90       # target; the speed is derived from it
min_speed = 120          # px/s at 1440 px height
max_speed = 600          # px/s at 1440 px height
step_px = 0.5
line_width = 1.2         # px at 1440 px height
exposure = 0.35
hold_seconds = 6
fade_seconds = 2.5
systems = ["whirlpools", "spirals"]
palettes = []            # empty = all
```

CLI flags:

| Flag | Effect |
|---|---|
| `--windowed` | Normal window instead of fullscreen; only Escape or closing the window exits. For development. |
| `--seed <u64>` | First scene seed. |
| `--system <name>` | Force the system for every scene. |
| `--output <name>` | Only open on this monitor. |
| `--snapshot <path.png>` | Headless: trace the whole scene in one go, write the final image, exit. Splats add up in any order, so this is the last frame of the live animation. Takes `--size WxH` (default 2560x1440). |
| `--config <path>` | Alternative config file. |
| `--sparks <n>`, `--trajectories <n>` | Override the config values, mostly for trying values in snapshots. |

Flags override the config file, which overrides the defaults. An invalid or unreadable
config file is logged and the defaults are used; the screensaver still starts.

The snapshot mode is the main way to check visual changes without sitting in front of the
screen. It must produce the same image for the same seed and size.

## 5. Omarchy integration

How Omarchy runs its screensaver today (Omarchy with the Quickshell-based shell,
Hyprland 0.56):

- The idle service, plugin `omarchy.idle` at
  `/usr/share/omarchy/shell/plugins/services/idle/Service.qml`, runs
  `omarchy-launch-screensaver` after `idle.screensaver` seconds (set in
  `~/.config/omarchy/shell.json`).
- That script starts a terminal per monitor running `omarchy-screensaver` (TTE text
  effects), with window class `org.omarchy.screensaver`.
- The idle service watches Hyprland `openwindow` and `closewindow` events for that class.
  If all screensaver windows close before the lock deadline, it treats that as activity
  and cancels the lock.
- Hyprland rules in `default/hypr/apps/system.lua` make that class fullscreen, floating,
  with a slide animation.
- `omarchy-toggle-screensaver` sets the `screensaver-off` toggle, which
  `omarchy-launch-screensaver` respects unless called with `force`.

Consequences for this project:

1. **Window class**: every window uses Wayland app_id `org.omarchy.screensaver`
   (`winit` `WindowAttributesExtWayland::with_name`). This gives the existing window rules,
   lock handoff and dismissal tracking for free.
2. **Launcher**: a script `tan-screensaver-launch` that exits early if a screensaver is
   already running, respects `omarchy-toggle-enabled screensaver-off` unless given
   `force`, and starts the binary.
3. **Hooking into idle**: `omarchy-launch-screensaver` is called by name, and
   `/usr/share/omarchy/bin` comes before `~/.local/bin` in `PATH`, so shadowing it does
   not work. Never edit files under `/usr/share/omarchy`; updates overwrite them. Instead,
   clone the idle plugin as a user plugin (`~/.config/omarchy/plugins/julien.idle`), change
   its launch command to `tan-screensaver-launch`, and add `omarchy.idle` to
   `disabledPlugins` in `shell.json`. This is the same pattern already used here for
   `julien.background` cloned from `omarchy.background`. Read
   `/usr/share/omarchy/shell/plugins/README.md` before doing this, and check after each
   Omarchy update whether the upstream idle service changed.
4. **Menu**: the Omarchy menu entry `system.screensaver` can be pointed at
   `tan-screensaver-launch force` through `~/.config/omarchy/extensions/omarchy-menu.jsonc`.
5. **Install**: `make install` (or a `scripts/install.sh`) builds in release mode and
   copies the binary and launcher to `~/.local/bin`. The plugin clone is a separate,
   explicit step (`scripts/install-omarchy-hook.sh`) because it changes the shell config,
   and it must back up `shell.json` first. The clone is generated by that script from the
   installed `omarchy.idle`; it is not published or installed as a plugin repository
   through `omarchy plugin add`.

## 6. Milestones

1. **Core maths**: `field`, `integrate`, `palette`, `scene`, with tests. Port check: a
   small Node script runs `flow.js` from the sibling repo and dumps a few reference
   trajectories (positions, κ and palette index), palette LUTs and `mulberry32` output as
   TOML fixtures (TOML because `toml` is already a dependency); the Rust tests must match
   them to 1e-9.
2. **Snapshot renderer**: headless wgpu, whole scene in one go, PNG out. Check against
   `flow.js` run in Node with the same system, parameters, view, palette and u32 seed, at
   a small size (say 320×180): every channel must be within ±2 of the Node output.
   `flow.js` exports its module for Node, so the tool script can call `render` directly.
3. **Live window**: one windowed screen, sparks moving, glow, per-scene speed from the
   length pre-pass, scene end when the last head stops, fades.
4. **Screensaver behaviour**: fullscreen on every monitor, exit rules, hidden cursor,
   app_id, signals.
5. **Omarchy integration**: launcher, install script, idle plugin clone, menu entry.
6. **Tuning**: scene ranges, spark count, trajectory count, target duration, by looking at many
   snapshots and live runs.

## 7. Open questions

- Should sparks be the only thing drawn, or should a faint full render of the scene be
  revealed underneath as they go? Start with sparks only.
- Should consecutive scenes avoid repeating the same system or palette? Probably yes,
  once the basics work.
- Battery: on a laptop on battery, lower the frame rate or spark count. Not needed on this
  desktop; leave a hook for it.
