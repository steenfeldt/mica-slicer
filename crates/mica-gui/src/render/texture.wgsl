// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 resources/shaders/110/printbed.fs
// Changes: WGSL; the radial background gradient takes its two colours from the theme
// (color = centre, warning = edge) instead of constants; the see-through flag is params.x
// bit 0 and keeps PrusaSlicer's rule (line art at half alpha, no background) for a camera
// below the bed or an inactive plate.
// Bed texture quad: vertices carry xyz + uv (image top at the back of the bed).
@group(0) @binding(1) var bed_texture: texture_2d<f32>;
@group(0) @binding(2) var bed_sampler: sampler;
struct Output { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) uv: vec2<f32>) -> Output {
    var o: Output;
    o.clip = u.vp * u.model * vec4(p, 1.0);
    o.uv = uv;
    return o;
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    let fore = textureSample(bed_texture, bed_sampler, o.uv);
    let t = smoothstep(0.0, 0.5, length(o.uv - vec2(0.5, 0.5)));
    let back = mix(u.color.rgb, u.warning.rgb, t);
    let see_through = (u.params.x & 1u) != 0u;
    if see_through {
        return vec4(fore.rgb, fore.a * 0.5);
    }
    return vec4(mix(back, fore.rgb, fore.a), 1.0);
}
