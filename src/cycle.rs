//! Scene phases: draw until done, hold the finished image, fade to the background.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Drawing,
    Holding,
    Fading,
}

/// What changed during `advance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Hold,
    Fade,
    /// The fade is over; start the next scene.
    Next,
}

#[derive(Clone, Debug)]
pub struct Cycle {
    phase: Phase,
    elapsed_s: f64,
    hold_s: f64,
    fade_s: f64,
}

impl Cycle {
    pub fn new(hold_s: f64, fade_s: f64) -> Cycle {
        Cycle {
            phase: Phase::Drawing,
            elapsed_s: 0.0,
            hold_s,
            fade_s,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// `dt` is wall-clock time since the last call, unclamped: the hold is not redrawn,
    /// so its whole length arrives in one call.
    pub fn advance(&mut self, dt: f64, drawing_done: bool) -> Change {
        match self.phase {
            Phase::Drawing if drawing_done => self.enter(Phase::Holding, Change::Hold),
            Phase::Drawing => Change::None,
            Phase::Holding => {
                self.elapsed_s += dt;
                if self.elapsed_s >= self.hold_s {
                    self.enter(Phase::Fading, Change::Fade)
                } else {
                    Change::None
                }
            }
            Phase::Fading => {
                self.elapsed_s += dt;
                if self.elapsed_s >= self.fade_s {
                    self.enter(Phase::Drawing, Change::Next)
                } else {
                    Change::None
                }
            }
        }
    }

    fn enter(&mut self, phase: Phase, change: Change) -> Change {
        self.phase = phase;
        self.elapsed_s = 0.0;
        change
    }

    /// Seconds left in the hold, for scheduling the next wake-up.
    pub fn hold_left_s(&self) -> f64 {
        match self.phase {
            Phase::Holding => (self.hold_s - self.elapsed_s).max(0.0),
            _ => 0.0,
        }
    }

    /// Image opacity for the tone map: 1, falling to 0 during the fade.
    pub fn opacity(&self) -> f32 {
        match self.phase {
            Phase::Fading if self.fade_s > 0.0 => {
                (1.0 - self.elapsed_s / self.fade_s).clamp(0.0, 1.0) as f32
            }
            Phase::Fading => 0.0,
            _ => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_through_the_phases() {
        let mut c = Cycle::new(6.0, 2.5);
        assert_eq!(c.advance(0.016, false), Change::None);
        assert_eq!(c.advance(0.016, true), Change::Hold);
        assert_eq!(c.hold_left_s(), 6.0);
        assert_eq!(c.advance(5.0, true), Change::None);
        assert_eq!(c.advance(1.0, true), Change::Fade);
        assert_eq!(c.opacity(), 1.0);
        assert_eq!(c.advance(1.25, true), Change::None);
        assert!((c.opacity() - 0.5).abs() < 1e-6);
        assert_eq!(c.advance(1.25, true), Change::Next);
        assert_eq!(c.phase(), Phase::Drawing);
        assert_eq!(c.opacity(), 1.0);
    }

    #[test]
    fn zero_hold_and_fade_go_straight_through() {
        let mut c = Cycle::new(0.0, 0.0);
        assert_eq!(c.advance(0.0, true), Change::Hold);
        assert_eq!(c.advance(0.0, true), Change::Fade);
        assert_eq!(c.advance(0.0, true), Change::Next);
    }
}
