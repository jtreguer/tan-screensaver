//! Settings (SPEC §4). Reading the config file arrives with the live window; for now the
//! defaults and the command-line overrides are all there is.

/// Output height at which `line_width` and speeds are taken literally.
pub const REF_HEIGHT_PX: f64 = 1440.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Start points per scene.
    pub trajectories: u32,
    /// Max arc length per direction, world units.
    pub arc_length: f64,
    pub trace_backward: bool,
    pub step_px: f64,
    /// px at `REF_HEIGHT_PX`.
    pub line_width: f64,
    pub exposure: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            trajectories: 1500,
            arc_length: 30.0,
            trace_backward: true,
            step_px: 0.5,
            line_width: 1.2,
            exposure: 0.35,
        }
    }
}
