//! View geometry, RK4 on the unit-speed field, curvature and stop conditions, as in
//! flow.js `render` and `trace`.

use crate::field::Field;

/// Below this field magnitude a point counts as fixed.
const MIN_SPEED: f64 = 1e-4;
/// Trajectories may leave the view by this fraction of `span` before they stop.
const MARGIN: f64 = 0.15;

/// Mapping between world units and output pixels. World y points up, pixel y down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub width_px: f64,
    pub height_px: f64,
    pub ppu: f64,
    pub x0w: f64,
    pub y0w: f64,
    pub xmin: f64,
    pub xmax: f64,
    pub ymin: f64,
    pub ymax: f64,
}

impl View {
    pub fn new(width_px: u32, height_px: u32, cx: f64, cy: f64, span: f64) -> View {
        let (w, h) = (width_px as f64, height_px as f64);
        let ppu = h / span;
        let x0w = cx - (w / 2.0) / ppu;
        let y0w = cy + (h / 2.0) / ppu;
        let margin = MARGIN * span;
        View {
            width_px: w,
            height_px: h,
            ppu,
            x0w,
            y0w,
            xmin: x0w - margin,
            xmax: x0w + w / ppu + margin,
            ymin: y0w - h / ppu - margin,
            ymax: y0w + margin,
        }
    }

    pub fn to_px(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.x0w) * self.ppu, (self.y0w - y) * self.ppu)
    }

    /// Inside the view plus margin. False for NaN.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x > self.xmin && x < self.xmax && y > self.ymin && y < self.ymax
    }

    /// World step for an integration step given in output pixels.
    pub fn h_world(&self, step_px: f64) -> f64 {
        step_px / self.ppu
    }
}

pub fn max_steps(arc_world: f64, h_world: f64) -> u32 {
    (arc_world / h_world).ceil() as u32
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub x: f64,
    pub y: f64,
    /// Signed curvature, world units⁻¹.
    pub kappa: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Field magnitude below 1e-4 or NaN: a fixed point or a tan() singularity.
    FixedPoint,
    OutOfView,
    MaxLength,
}

#[inline]
fn unit(field: &Field, x: f64, y: f64) -> (f64, f64, f64) {
    let (vx, vy) = field.eval(x, y);
    let s = vx.hypot(vy);
    (vx / s, vy / s, s)
}

/// One trajectory being integrated in one time direction.
#[derive(Clone, Debug)]
pub struct Head {
    pub x: f64,
    pub y: f64,
    /// +1 forward in time, −1 backward.
    pub dir: f64,
    pub steps: u32,
    pub stopped: Option<Stop>,
}

impl Head {
    pub fn new(x: f64, y: f64, dir: f64) -> Head {
        Head {
            x,
            y,
            dir,
            steps: 0,
            stopped: None,
        }
    }

    /// Advances one RK4 step. Once it returns a Stop, every later call returns the same.
    pub fn advance(
        &mut self,
        field: &Field,
        view: &View,
        h_world: f64,
        max_steps: u32,
    ) -> Result<Step, Stop> {
        if let Some(stop) = self.stopped {
            return Err(stop);
        }
        let stop = |head: &mut Head, why| {
            head.stopped = Some(why);
            Err(why)
        };
        if self.steps >= max_steps {
            return stop(self, Stop::MaxLength);
        }
        let (x, y, h) = (self.x, self.y, h_world);
        let hh = h * self.dir;
        let k1 = unit(field, x, y);
        let moving = k1.2 > MIN_SPEED;
        if !moving {
            return stop(self, Stop::FixedPoint);
        }
        let k2 = unit(field, x + hh / 2.0 * k1.0, y + hh / 2.0 * k1.1);
        // Turn of the unit tangent over half a step. Dividing by h, not hh, makes the sign
        // follow the direction of travel: a left turn traced backward is a right turn.
        let kappa = (k1.0 * k2.1 - k1.1 * k2.0) / (h / 2.0);
        let k3 = unit(field, x + hh / 2.0 * k2.0, y + hh / 2.0 * k2.1);
        let k4 = unit(field, x + hh * k3.0, y + hh * k3.1);
        self.x = x + hh * (k1.0 + 2.0 * k2.0 + 2.0 * k3.0 + k4.0) / 6.0;
        self.y = y + hh * (k1.1 + 2.0 * k2.1 + 2.0 * k3.1 + k4.1) / 6.0;
        self.steps += 1;
        if !view.contains(self.x, self.y) {
            return stop(self, Stop::OutOfView);
        }
        Ok(Step {
            x: self.x,
            y: self.y,
            kappa,
        })
    }
}

/// Integrates a whole half-trajectory, calling `on_step` for every step that is drawn.
pub fn trace(
    field: &Field,
    view: &View,
    h_world: f64,
    max_steps: u32,
    head: &mut Head,
    mut on_step: impl FnMut(Step),
) -> Stop {
    loop {
        match head.advance(field, view, h_world, max_steps) {
            Ok(step) => on_step(step),
            Err(stop) => return stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHIRL: Field = Field::Whirlpools {
        k: 1.3,
        w: 1.0,
        d: 0.15,
    };

    fn view() -> View {
        View::new(1600, 900, 0.0, 0.0, 12.6)
    }

    #[test]
    fn view_is_centred_and_maps_corners() {
        let v = view();
        let (px, py) = v.to_px(0.0, 0.0);
        assert!((px - 800.0).abs() < 1e-9 && (py - 450.0).abs() < 1e-9);
        let (px, py) = v.to_px(v.x0w, v.y0w);
        assert!(px.abs() < 1e-12 && py.abs() < 1e-12);
        assert!((v.ymax - v.ymin - 1.3 * 12.6).abs() < 1e-9);
    }

    #[test]
    fn origin_of_whirlpools_is_a_fixed_point() {
        let v = view();
        let mut head = Head::new(0.0, 0.0, 1.0);
        assert_eq!(head.advance(&WHIRL, &v, 0.01, 100), Err(Stop::FixedPoint));
        assert_eq!(head.advance(&WHIRL, &v, 0.01, 100), Err(Stop::FixedPoint));
        assert_eq!(head.steps, 0);
    }

    #[test]
    fn nan_field_stops_as_fixed_point() {
        let v = view();
        let f = Field::Whirlpools {
            k: f64::NAN,
            w: 1.0,
            d: 0.0,
        };
        assert_eq!(
            Head::new(1.0, 1.0, 1.0).advance(&f, &v, 0.01, 100),
            Err(Stop::FixedPoint)
        );
    }

    #[test]
    fn stops_at_max_length_after_exactly_max_steps() {
        let v = view();
        let h = v.h_world(0.5);
        let n = max_steps(2.0, h);
        let mut head = Head::new(1.0, 0.5, 1.0);
        let mut count = 0;
        let stop = trace(&WHIRL, &v, h, n, &mut head, |_| count += 1);
        assert_eq!(stop, Stop::MaxLength);
        assert_eq!(count, n);
    }

    #[test]
    fn unit_speed_steps_advance_by_about_h() {
        let v = view();
        let h = v.h_world(0.5);
        let mut head = Head::new(1.0, 0.5, -1.0);
        let (mut px, mut py) = (head.x, head.y);
        trace(&WHIRL, &v, h, 200, &mut head, |s| {
            let d = (s.x - px).hypot(s.y - py);
            assert!((d - h).abs() < 0.01 * h, "step length {d} vs h {h}");
            (px, py) = (s.x, s.y);
        });
    }

    #[test]
    fn leaving_the_view_stops_without_drawing_the_outside_step() {
        // Uniform field to the right: x' = tan(k sin(ω y)) is constant at y = π/2.
        let f = Field::Whirlpools {
            k: 1.0,
            w: 1.0,
            d: 0.0,
        };
        let v = View::new(160, 90, 0.0, std::f64::consts::FRAC_PI_2, 2.0);
        let h = v.h_world(1.0);
        let mut head = Head::new(v.xmax - 1.5 * h, std::f64::consts::FRAC_PI_2, 1.0);
        let mut drawn = Vec::new();
        assert_eq!(
            trace(&f, &v, h, 1000, &mut head, |s| drawn.push(s)),
            Stop::OutOfView
        );
        assert!(drawn.iter().all(|s| v.contains(s.x, s.y)));
        assert_eq!(head.steps as usize, drawn.len() + 1);
    }

    #[test]
    fn curvature_sign_flips_with_time_direction() {
        let v = view();
        let h = v.h_world(0.5);
        let k_fwd = Head::new(1.0, 0.5, 1.0)
            .advance(&WHIRL, &v, h, 10)
            .map(|s| s.kappa);
        let k_bwd = Head::new(1.0, 0.5, -1.0)
            .advance(&WHIRL, &v, h, 10)
            .map(|s| s.kappa);
        let (f, b) = (k_fwd.unwrap(), k_bwd.unwrap());
        assert!(f.abs() > 0.1 && (f + b).abs() < 0.05 * f.abs(), "{f} {b}");
    }
}
