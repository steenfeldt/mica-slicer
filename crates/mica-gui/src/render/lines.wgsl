// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Coloured 3D line list (printable volume outline, helpers).
struct Output { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) c: vec4<f32>) -> Output {
    var o: Output;
    o.clip = u.vp * u.model * vec4(p, 1.0);
    o.color = c;
    return o;
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> { return o.color; }
