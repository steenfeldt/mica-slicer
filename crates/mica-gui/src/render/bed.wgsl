// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// The drawn (fallback) bed and flat bed geometry. params.z selects the pass:
//   1 bed surface: color = plate colour, warning.rgb = minor grid colour, extra.xy = plate
//     origin, extra.w = minor grid step in mm; color.a < 1 draws it see-through (from below).
//   2 hatched exclusion zone (color), 3 flat colour (outline strips).
// Major grid lines are line geometry (crisp under MSAA).
struct Output { @builtin(position) clip: vec4<f32>, @location(0) world: vec3<f32> }
@vertex fn vs(@location(0) p: vec3<f32>) -> Output {
    var o: Output;
    let world = u.model * vec4(p, 1.0);
    o.world = world.xyz;
    o.clip = u.vp * world;
    return o;
}
fn grid_line(local: vec2<f32>, step: f32, aa: vec2<f32>) -> f32 {
    let d = abs(fract(local / step + 0.5) - 0.5) * step;
    let lx = 1.0 - smoothstep(0.0, aa.x, d.x);
    let ly = 1.0 - smoothstep(0.0, aa.y, d.y);
    return max(lx, ly);
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    let local = o.world.xy - u.extra.xy;
    if u.params.z == 3u { return u.color; }
    if u.params.z == 2u {
        let stripe = step(0.5, fract((local.x - local.y) / 8.0));
        return vec4(mix(u.color.rgb * 0.55, u.color.rgb, stripe), u.color.a);
    }
    let aa = max(fwidth(local), vec2(0.02));
    let line = grid_line(local, u.extra.w, aa * 1.1);
    let c = mix(u.color.rgb, u.warning.rgb, line);
    return vec4(c, u.color.a);
}
