// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Shared uniform block, prepended to every Mica shader at pipeline creation.
struct Uniforms {
    vp: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x: visibility masks / flags, y: colour mode, z: pass kind, w: base index.
    params: vec4<u32>,
    // Pass-specific values (see each shader).
    extra: vec4<f32>,
    // Key light direction (towards the light), w = intensity.
    light: vec4<f32>,
    // Camera position, w = 1 for perspective.
    eye: vec4<f32>,
    // Hemisphere ambient.
    sky: vec4<f32>,
    ground: vec4<f32>,
    // Warning tint for objects outside the volume (prepare) / secondary colour (blit).
    warning: vec4<f32>,
    tools: array<vec4<f32>, 16>,
    // Section plane: fragments with dot(xyz, world) > w are cut away (xyz = 0 keeps everything).
    clip: vec4<f32>,
    // Printable volume for the outside-part test (objects, mask): xy the plate's origin
    // (world), z the maximum print height, w the tolerance (mm).
    volume: vec4<f32>,
    // volume_counts[0].x: points of the bed outline, [0].y: exclusion zones, then each zone's
    // point count in order ([0].z, [0].w, [1].x … [1].w; at most six zones).
    volume_counts: array<vec4<u32>, 2>,
    // The bed outline, then each exclusion zone (bed mm), two points per vec4.
    volume_points: array<vec4<f32>, 32>,
    // Colour of the part outside the volume: its stripes and its outline.
    danger: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Uniforms;

fn clipped(world: vec3<f32>) -> bool {
    return dot(u.clip.xyz, world) > u.clip.w;
}

fn volume_point(i: u32) -> vec2<f32> {
    let v = u.volume_points[i / 2u];
    return select(v.zw, v.xy, (i & 1u) == 0u);
}
fn volume_count(i: u32) -> u32 {
    let v = u.volume_counts[i / 4u];
    let k = i % 4u;
    if k == 0u { return v.x; }
    if k == 1u { return v.y; }
    if k == 2u { return v.z; }
    return v.w;
}
// Signed distance from `p` to the polygon of `n` points from `start` (crossing rule for the
// sign): negative inside, positive outside.
fn polygon_distance(p: vec2<f32>, start: u32, n: u32) -> f32 {
    var inside = false;
    var d = 1e9;
    for (var k = 0u; k < n; k++) {
        let a = volume_point(start + k);
        let b = volume_point(start + (k + 1u) % n);
        let ab = b - a;
        let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-12), 0.0, 1.0);
        d = min(d, length(p - (a + ab * t)));
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * ab.x {
            inside = !inside;
        }
    }
    return select(d, -d, inside);
}
// Whether a world point lies outside the printable volume of its plate: past the bed outline,
// inside an exclusion zone, above the maximum print height or below the bed.
fn outside_volume(world: vec3<f32>) -> bool {
    let eps = u.volume.w;
    if world.z > u.volume.z + eps || world.z < -eps { return true; }
    let p = world.xy - u.volume.xy;
    let n = volume_count(0u);
    if n >= 3u && polygon_distance(p, 0u, n) > eps { return true; }
    var start = n;
    let zones = min(volume_count(1u), 6u);
    for (var z = 0u; z < zones; z++) {
        let m = volume_count(2u + z);
        if m >= 3u && polygon_distance(p, start, m) < -eps { return true; }
        start += m;
    }
    return false;
}

fn shade(n_in: vec3<f32>, world: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    let l = normalize(u.light.xyz);
    // Orthographic cameras place the eye far along the view direction, so this holds for both.
    let v = normalize(u.eye.xyz - world);
    // Derivative normals have no consistent winding: face the viewer.
    var n = n_in;
    if dot(n, v) < 0.0 { n = -n; }
    let key = max(dot(n, l), 0.0) * 0.62 * u.light.w;
    let fill_dir = normalize(vec3(-l.x, -l.y, 0.35));
    let fill = max(dot(n, fill_dir), 0.0) * 0.3;
    let hemi = mix(u.ground.rgb, u.sky.rgb, n.z * 0.5 + 0.5) * 0.6;
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 28.0) * 0.14 * u.light.w;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.28;
    return albedo * (hemi + vec3(key + fill)) * (1.0 - rim) + vec3(spec);
}
