// Gaussian splats added into the accumulation texture as (r·w, g·w, b·w, w), matching
// flow.js `splat`: pixel i is centred on coordinate i, the footprint is round(p) ± radius,
// weights below 0.01 are skipped.

struct Params {
    size: vec2<f32>,
    inv_2_sigma2: f32,
    radius: f32,
    lut: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> p: Params;

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) centre: vec2<f32>,
    @location(1) @interpolate(flat) colour: vec3<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @location(0) at: vec2<f32>, @location(1) ci: u32) -> VertexOut {
    // floor(x + 0.5) is Math.round; WGSL's round() goes to even on ties.
    let lo = floor(at + 0.5) - p.radius;
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = lo + corner * (2.0 * p.radius + 1.0);
    var out: VertexOut;
    out.pos = vec4<f32>(px.x / p.size.x * 2.0 - 1.0, 1.0 - px.y / p.size.y * 2.0, 0.0, 1.0);
    out.centre = at;
    out.colour = p.lut[ci].rgb;
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let d = in.pos.xy - 0.5 - in.centre;
    let w = exp(-dot(d, d) * p.inv_2_sigma2);
    if w < 0.01 {
        discard;
    }
    return vec4<f32>(in.colour * w, w);
}
