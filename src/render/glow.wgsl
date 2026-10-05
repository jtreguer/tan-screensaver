// Spark heads drawn on top of the tone-mapped image with additive blending: a small core
// pushed towards white plus a wide soft halo in the palette colour.

struct Params {
    size: vec2<f32>,
    core_sigma: f32,
    halo_sigma: f32,
    radius: f32,
    core_gain: f32,
    halo_gain: f32,
    whiten: f32,
};

@group(0) @binding(0) var<uniform> p: Params;

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) centre: vec2<f32>,
    @location(1) @interpolate(flat) colour: vec3<f32>,
    @location(2) @interpolate(flat) intensity: f32,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) at: vec2<f32>,
    @location(1) colour: vec3<f32>,
    @location(2) intensity: f32,
) -> VertexOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    // Splat coordinates centre pixel i on i; framebuffer pixel i spans [i, i + 1).
    let px = at + 0.5 + corner * p.radius;
    var out: VertexOut;
    out.pos = vec4<f32>(px.x / p.size.x * 2.0 - 1.0, 1.0 - px.y / p.size.y * 2.0, 0.0, 1.0);
    out.centre = at;
    out.colour = colour;
    out.intensity = intensity;
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let d = in.pos.xy - 0.5 - in.centre;
    let r2 = dot(d, d);
    let core = exp(-r2 / (2.0 * p.core_sigma * p.core_sigma)) * p.core_gain;
    let halo = exp(-r2 / (2.0 * p.halo_sigma * p.halo_sigma)) * p.halo_gain;
    let c = mix(in.colour, vec3<f32>(1.0), p.whiten) * core + in.colour * halo;
    return vec4<f32>(c * in.intensity, 0.0);
}
