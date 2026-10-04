// Average colour per pixel, opacity from accumulated density, as flow.js. The background
// is in 0..255 like flow.js; accumulated colour is in 0..1 (see Renderer::start).

struct Params {
    background: vec4<f32>,
    exposure_step: f32,
    // 1 shows the image, 0 only the background.
    fade: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var acc: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let s = textureLoad(acc, vec2<i32>(pos.xy), 0);
    let bg = p.background.rgb;
    var c = bg;
    if s.a > 0.0 {
        let a = 1.0 - exp(-p.exposure_step * s.a);
        c = bg + (s.rgb * 255.0 / s.a - bg) * a;
    }
    c = mix(bg, c, p.fade);
    return vec4<f32>(c / 255.0, 1.0);
}
