//! Headless rendering of a whole scene to an RGBA buffer.

use std::path::Path;

use super::{err, Error, Gpu, Renderer, CHUNK};
use crate::sim::Drawing;

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Traces every trajectory of the drawing and returns the tone-mapped image, RGBA8,
/// rows top to bottom. Splats add up in any order, so this equals the final frame of the
/// live animation.
pub fn snapshot(gpu: &Gpu, drawing: &Drawing) -> Result<Vec<u8>, Error> {
    let (w, h) = (drawing.view.width_px as u32, drawing.view.height_px as u32);
    let row_bytes = w * 4;
    let padded_row =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let limits = gpu.device.limits();
    let max_side = limits.max_texture_dimension_2d;
    if w > max_side || h > max_side {
        return Err(Error(format!(
            "{w}x{h} exceeds this GPU's {max_side} px texture limit"
        )));
    }
    if padded_row as u64 * h as u64 > limits.max_buffer_size {
        return Err(Error(format!(
            "{w}x{h} exceeds this GPU's readback buffer limit"
        )));
    }

    // Out of memory or invalid sizes would otherwise reach wgpu's default handler, which
    // panics.
    let validation = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out_of_memory = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let renderer = Renderer::new(gpu, w, h, OUTPUT_FORMAT);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snapshot"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("snapshot readback"),
        size: padded_row as u64 * h as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Pop both before checking, so no scope is left open on the device.
    let oom = pollster::block_on(out_of_memory.pop());
    let invalid = pollster::block_on(validation.pop());
    if let Some(e) = oom.or(invalid) {
        return Err(err(&format!("allocating a {w}x{h} snapshot"), e));
    }

    renderer.start(gpu, drawing);
    let mut batch = Vec::with_capacity(CHUNK);
    drawing.trace_all(|s| {
        batch.push(s);
        if batch.len() == CHUNK {
            renderer.splat(gpu, &batch);
            batch.clear();
        }
    });
    renderer.splat(gpu, &batch);

    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.tone_map(&mut encoder, &target.create_view(&Default::default()));
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: None,
            },
        },
        target.size(),
    );
    gpu.queue.submit([encoder.finish()]);

    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(|e| err("waiting for the GPU", e))?;
    rx.recv()
        .map_err(|e| err("buffer mapping callback", e))?
        .map_err(|e| err("mapping readback buffer", e))?;

    let mapped = readback
        .get_mapped_range(..)
        .map_err(|e| err("reading readback buffer", e))?;
    let mut pixels = Vec::with_capacity((row_bytes * h) as usize);
    for row in mapped.chunks(padded_row as usize) {
        pixels.extend_from_slice(&row[..row_bytes as usize]);
    }
    Ok(pixels)
}

/// Writes RGBA8 pixels as an RGB PNG.
pub fn save_png(path: &Path, width_px: u32, height_px: u32, rgba: &[u8]) -> Result<(), Error> {
    let file = std::fs::File::create(path).map_err(|e| err(&path.display().to_string(), e))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width_px, height_px);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| err("writing PNG", e))?;
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .collect();
    writer
        .write_image_data(&rgb)
        .map_err(|e| err("writing PNG", e))?;
    writer.finish().map_err(|e| err("writing PNG", e))
}
