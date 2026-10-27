// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// A plate tag lying on the ground in front of its plate (`render::tags`). The texture holds
// coverage masks, not colours: r = the rounded chip, g = the text or icon, b = the rim. The
// colours come from the uniform (color = chip fill, warning = text, extra = rim), so the
// theme, hover and alert states never re-rasterise the tag.
@group(0) @binding(1) var tag_texture: texture_2d<f32>;
@group(0) @binding(2) var tag_sampler: sampler;
struct Output { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) uv: vec2<f32>) -> Output {
    var o: Output;
    o.clip = u.vp * u.model * vec4(p, 1.0);
    o.uv = uv;
    return o;
}
// Straight-alpha "over".
fn over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return vec4(0.0);
    }
    return vec4((top.rgb * top.a + bottom.rgb * bottom.a * (1.0 - top.a)) / a, a);
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    let m = textureSample(tag_texture, tag_sampler, o.uv);
    var c = vec4(u.color.rgb, u.color.a * m.r);
    c = over(vec4(u.extra.rgb, u.extra.a * m.b), c);
    c = over(vec4(u.warning.rgb, u.warning.a * m.g), c);
    return c;
}
