// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Full-screen background: one flat colour (color), with a barely visible lift towards the
// centre so the plate does not float on a dead surface.
struct Output { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@builtin(vertex_index) i: u32) -> Output {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var o: Output;
    o.clip = vec4(p[i], 1.0, 1.0);
    o.uv = vec2(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5);
    return o;
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    let r = length(o.uv - vec2(0.5, 0.5));
    let c = u.color.rgb + vec3(0.012 * (1.0 - smoothstep(0.1, 0.9, r)));
    return vec4(c, 1.0);
}
