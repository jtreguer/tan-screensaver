//! Pure core of the screensaver: no wgpu or winit here, so it can be tested and shared
//! with the snapshot mode.

pub mod field;
pub mod integrate;
pub mod palette;
pub mod rng;
pub mod scene;
