//! The pure modules (everything but `render`) do not use wgpu or winit, so they can be
//! tested on their own and shared by the snapshot and live modes.

pub mod config;
pub mod field;
pub mod integrate;
pub mod palette;
pub mod render;
pub mod rng;
pub mod scene;
pub mod sim;
