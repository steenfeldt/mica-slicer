// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Composites the resolved viewport into egui and draws the outlines from the single-sample
// mask: selection (red channel 1.0) and hover (0.5) around their objects, and in red (green
// channel) around the visible part of an object that lies outside the printable volume, the
// same style. color = selection colour, warning = hover colour, danger = outside colour,
// extra.xy = texel size.
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;
@group(0) @binding(3) var mask: texture_2d<f32>;
struct Output { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@builtin(vertex_index) i: u32) -> Output {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var o: Output;
    o.clip = vec4(p[i], 0.0, 1.0);
    o.uv = vec2(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5);
    return o;
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    var c = textureSample(image, image_sampler, o.uv);
    let center = textureSample(mask, image_sampler, o.uv);
    var strongest = 0.0;
    var hits = 0.0;
    var outside = 0.0;
    let t = u.extra.xy;
    for (var r = 1.0; r <= 2.0; r += 1.0) {
        for (var k = 0; k < 8; k++) {
            let a = f32(k) * 0.7853982;
            let off = vec2(cos(a), sin(a)) * t * r * 1.25;
            let m = textureSample(mask, image_sampler, o.uv + off);
            if m.r > 0.01 { hits += 1.0; strongest = max(strongest, m.r); }
            if m.g > 0.5 { outside += 1.0; }
        }
    }
    if center.r <= 0.01 && hits > 0.0 {
        let outline = select(u.warning.rgb, u.color.rgb, strongest > 0.75);
        let alpha = clamp(hits / 6.0, 0.0, 1.0);
        c = vec4(mix(c.rgb, outline, alpha), 1.0);
    }
    // Around the outside part, also where it meets the rest of its object.
    if center.g <= 0.5 && outside > 0.0 {
        c = vec4(mix(c.rgb, u.danger.rgb, clamp(outside / 6.0, 0.0, 1.0)), 1.0);
    }
    return c;
}
