//! The pure modules (everything but `render`, `app` and `signals`) do not use wgpu or winit, so they can be
//! tested on their own and shared by the snapshot and live modes.

pub mod app;
pub mod budget;
pub mod config;
pub mod cycle;
pub mod field;
pub mod integrate;
pub mod palette;
pub mod render;
pub mod rng;
pub mod scene;
pub mod signals;
pub mod sim;
