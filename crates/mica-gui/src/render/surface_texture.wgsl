// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: bumpmesh@a6d6d6e6df js/mapping.js :: computeUV, getCubicBlendWeights
// Copyright (c) 2026 CNCKitchen (Stefan Hermann) and contributors (BumpMesh, AGPL-3.0-only)
// Changes: WGSL; the projection Mica's texture skin samples (mica_core::texture::projection),
// in millimetres, shown as bump shading: the height's screen-space gradient tilts the flat
// derivative normal (Mikkelsen's surface gradient), so tile size, rotation and depth can be
// judged before slicing. Parameters ride in `tools[0..7]` (this pass has no tool colours):
//   0: bounds min xyz, projection index      1: tile U, tile V, offset U, offset V (mm)
//   2: cos, sin of the rotation, depth, blend 3: cylinder centre xy, radius, U tile
//   4: sphere radius, U tile, cap cos, axis   5: bounds centre xyz, -
//   6: top limit, bottom limit (degrees), 1 on all surfaces (tops too), -
// params.x: 1 hovered.
@group(0) @binding(1) var height_map: texture_2d<f32>;
@group(0) @binding(2) var height_sampler: sampler;
struct Output { @builtin(position) clip: vec4<f32>, @location(0) world: vec3<f32> }
@vertex fn vs(@location(0) p: vec3<f32>) -> Output {
    var o: Output;
    let world = u.model * vec4(p, 1.0);
    o.world = world.xyz;
    o.clip = u.vp * world;
    return o;
}

const TAU: f32 = 6.283185307;

fn tile(um: f32, vm: f32, su: f32, sv: f32) -> f32 {
    var uv = vec2(um / su + u.tools[1].z / su, vm / sv + u.tools[1].w / sv);
    let d = uv - vec2(0.5);
    let c = u.tools[2].x;
    let s = u.tools[2].y;
    uv = vec2(c * d.x - s * d.y, s * d.x + c * d.y) + vec2(0.5);
    return textureSampleLevel(height_map, height_sampler, vec2(fract(uv.x), 1.0 - fract(uv.y)), 0.0).r;
}

fn to_axis(p: vec3<f32>, axis: f32) -> vec3<f32> {
    if axis < 0.5 { return p.yzx; }
    if axis < 1.5 { return p.zxy; }
    return p;
}

fn three_planes(rel: vec3<f32>, n: vec3<f32>, w: vec3<f32>) -> f32 {
    let su = u.tools[1].x;
    let sv = u.tools[1].y;
    let xy = tile(select(rel.x, -rel.x, n.z < 0.0), rel.y, su, sv);
    let xz = tile(select(rel.x, -rel.x, n.y > 0.0), rel.z, su, sv);
    let yz = tile(select(rel.y, -rel.y, n.x < 0.0), rel.z, su, sv);
    return (xy * w.z + xz * w.y + yz * w.x) / max(w.x + w.y + w.z, 1e-6);
}

fn cubic_weights(n: vec3<f32>, blend: f32) -> vec3<f32> {
    let a = abs(n);
    var hot = vec3(0.0, 0.0, 1.0);
    var primary = a.z;
    var secondary = max(a.x, a.y);
    if a.x >= a.y - 1e-4 && a.x >= a.z - 1e-4 {
        hot = vec3(1.0, 0.0, 0.0);
        primary = a.x;
        secondary = max(a.y, a.z);
    } else if a.y >= a.z - 1e-4 {
        hot = vec3(0.0, 1.0, 0.0);
        primary = a.y;
        secondary = max(a.x, a.z);
    }
    if blend <= 0.001 { return hot; }
    let raw = 1.0 - clamp((primary - secondary) / 0.35, 0.0, 1.0);
    let mixv = blend * raw * raw * (3.0 - 2.0 * raw);
    if mixv <= 0.001 { return hot; }
    let power = 1.0 + (1.0 - mixv) * 11.0;
    let s = pow(a, vec3(power));
    let m = hot * (1.0 - mixv) + s / (s.x + s.y + s.z + 1e-6) * mixv;
    return m / (m.x + m.y + m.z);
}

fn grey(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let mode = u.tools[0].w;
    let rel = world - u.tools[0].xyz;
    let su = u.tools[1].x;
    let sv = u.tools[1].y;
    if mode < 0.5 {
        let b = pow(abs(n), vec3(4.0));
        return three_planes(rel, n, b / (b.x + b.y + b.z + 1e-6));
    }
    if mode < 1.5 {
        return three_planes(rel, n, cubic_weights(n, u.tools[2].w));
    }
    if mode < 2.5 {
        let axis = u.tools[4].w;
        let p = to_axis(world, axis);
        let nz = to_axis(n, axis).z;
        let zmin = to_axis(u.tools[0].xyz, axis).z;
        let r = u.tools[3].z;
        let rx = p.x - u.tools[3].x;
        let ry = p.y - u.tools[3].y;
        let theta = atan2(ry, rx);
        let side = tile((theta / TAU + 0.5) * r * TAU, p.z - zmin, u.tools[3].w, sv);
        if u.tools[2].w <= 0.001 { return side; }
        let raw = clamp((abs(nz) - (u.tools[4].z - 0.25)) / 0.5, 0.0, 1.0);
        let cap = raw * raw * (3.0 - 2.0 * raw);
        return mix(side, tile(rx + r, ry + r, su, sv), cap);
    }
    if mode < 3.5 {
        let c = u.tools[5].xyz;
        let q = world - c;
        let r = length(q);
        let phi = acos(clamp(q.z / max(r, 1e-6), -1.0, 1.0));
        let theta = atan2(q.y, q.x);
        let big = u.tools[4].x;
        return tile((theta / TAU + 0.5) * TAU * big, phi * big, u.tools[4].y, sv);
    }
    if mode < 4.5 { return tile(rel.x, rel.y, su, sv); }
    if mode < 5.5 { return tile(rel.x, rel.z, su, sv); }
    return tile(rel.y, rel.z, su, sv);
}

// The engine's angle masks: nothing on flat surfaces, fading in over 10° above the limits.
// On all surfaces only the bottom limit applies.
fn angle_fade(n: vec3<f32>) -> f32 {
    let a = degrees(acos(clamp(abs(n.z), 0.0, 1.0)));
    let limit = select(u.tools[6].y, u.tools[6].x, n.z >= 0.0);
    let all = u.tools[6].z > 0.5;
    if all && n.z >= 0.0 { return 1.0; }
    var f = select(smoothstep(3.0, 8.0, a), 1.0, all);
    if limit > 0.0 { f = f * smoothstep(limit, limit + 10.0, a); }
    return f;
}

@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    if clipped(o.world) { discard; }
    let sx = dpdx(o.world);
    let sy = dpdy(o.world);
    var n = normalize(cross(sx, sy));
    let v = normalize(u.eye.xyz - o.world);
    if dot(n, v) < 0.0 { n = -n; }
    let g = grey(o.world, n);
    let fade = angle_fade(n);
    let h = g * u.tools[2].z * fade * 1.6;
    // Surface gradient bump (Mikkelsen 2010), with the derivative normal facing the viewer.
    let r1 = cross(sy, n);
    let r2 = cross(n, sx);
    let det = dot(sx, r1);
    let grad = sign(det) * (dpdx(h) * r1 + dpdy(h) * r2);
    let bumped = normalize(abs(det) * n - grad);
    var albedo = u.color.rgb * (1.0 - 0.18 * fade * (1.0 - g));
    if (u.params.x & 1u) != 0u { albedo = albedo * 1.12 + vec3(0.04); }
    return vec4(shade(bumped, o.world, albedo), 1.0);
}
