// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// G-code preview. The 32-byte records and their 8-byte attribute records (actual speed, flow,
// temperature, acceleration as u16 fixed point) stay on the GPU in chunks (group 1); a draw
// walks a chunk's list of visible record indices, so hidden roles and tools cost nothing.
// Each move is drawn at one of three levels of detail (`render::toolpaths::Detail`):
// - `vs_full` + `fs_full`: a six-sided tube with elliptical cross-section and smooth normals (the
//   current layer, always);
// - `vs` + `fs`: the same tube whose shading normal blends to the surface's macro normal (wall
//   side or up) as the stripes the moves make on screen get narrower than 3 px, entirely
//   below 1.5 px (`stripe`: the layer height on walls, the line width on other moves, seen
//   across the view), where the per-pixel tube shading would beat against the pixel grid
//   (moiré);
// - `vs_ribbon` + `fs_ribbon`: one camera-facing quad (2 triangles) as wide as the tube's
//   silhouette, shaded as a round tube impostor with the same macro-normal blend (distant
//   layers while the camera moves; thin ones shade per vertex).
// `vs_id` / `vs_ribbon_id` + `fs_id` write the record index + 1 for the hover pick
// (`render::toolpaths::Pick`).
// params.y = colour mode (`preview::ColorMode` discriminant: 0 feature, 1 tool, 2 speed, 3 fan,
// 4 width, 5 height, 6 layer time, 7 actual speed, 8 volumetric flow, 9 temperature,
// 10 acceleration, 11 layer time (log), 12 colour print: the layer value packs a 4-bit palette
// index per tool, 0 = the tool's colour, and `tools` holds the palette), extra.xy = gradient
// range (natural log for mode 11), extra.z = pixels per mm at unit depth (perspective, eye.w
// = 1) or per mm (orthographic), u.clip = section plane, u.model = the shown plate's world
// offset (records are in bed coordinates).
@group(0) @binding(1) var<storage, read> layer_starts: array<u32>;
@group(0) @binding(2) var<storage, read> layer_values: array<f32>;
struct Record {
    ax: f32, ay: f32, az: f32,
    bx: f32, by: f32, bz: f32,
    size: u32,
    kind: u32,
}
@group(1) @binding(0) var<storage, read> records: array<Record>;
@group(1) @binding(1) var<storage, read> attrs: array<vec2<u32>>;
// The chunk's visible records (indices into `records`), in record order.
@group(1) @binding(2) var<storage, read> visible: array<u32>;
// x: record index of the chunk's first record.
@group(1) @binding(3) var<uniform> chunk: vec4<u32>;
// Everything a vertex can produce; each pipeline passes on only what its fragments read
// (`Tube`, `Ribbon`, `Pick`), which keeps the per-vertex output small.
struct Output {
    clip: vec4<f32>,
    normal: vec3<f32>,
    color: vec3<f32>,
    world: vec3<f32>,
    // The macro normal of the surface the move belongs to.
    face: vec3<f32>,
    // The move's stripe on screen (pixels across the view, `stripe`).
    thickness: f32,
    // Ribbons: the direction across the quad, the direction towards the viewer perpendicular
    // to the move, and the position across (−1..1).
    across: vec3<f32>,
    toward: vec3<f32>,
    s: f32,
    id: u32,
    // Ribbons: the flat (macro-normal) shade, per vertex.
    flat: vec3<f32>,
}
// Tubes find their macro normal from the tube normal (its horizontal part is the side
// direction), so they need one value more than the tube alone: the stripe width, negative
// for walls.
struct Tube {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world: vec3<f32>,
    @location(4) thickness: f32,
}
struct Ribbon {
    @builtin(position) clip: vec4<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world: vec3<f32>,
    @location(3) face: vec3<f32>,
    @location(4) thickness: f32,
    @location(5) across: vec3<f32>,
    @location(6) toward: vec3<f32>,
    @location(7) s: f32,
    @location(9) flat: vec3<f32>,
}
struct Pick {
    @builtin(position) clip: vec4<f32>,
    @location(2) world: vec3<f32>,
    @location(8) @interpolate(flat) id: u32,
}
const roles = array<vec3<f32>, 16>(
    vec3(70., 190., 90.), vec3(70., 190., 90.), vec3(255., 125., 56.), vec3(60., 100., 255.),
    vec3(255., 230., 77.), vec3(176., 48., 41.), vec3(150., 84., 204.), vec3(80., 150., 255.),
    vec3(40., 210., 235.), vec3(240., 64., 64.), vec3(60., 210., 220.), vec3(255., 255., 255.),
    vec3(75., 180., 105.), vec3(75., 210., 220.), vec3(110., 135., 160.), vec3(210., 170., 120.));
// 6 ring vertices (angle index), 6 side quads × 2 triangles × 3 = 36 vertices.
const quad = array<vec2<u32>, 6>(vec2(0u, 0u), vec2(1u, 0u), vec2(1u, 1u), vec2(0u, 0u), vec2(1u, 1u), vec2(0u, 1u));
fn gradient(t: f32) -> vec3<f32> {
    // Blue → cyan → green → yellow → red, like slicer speed legends.
    let x = clamp(t, 0.0, 1.0) * 4.0;
    let stops = array<vec3<f32>, 5>(
        vec3(0.16, 0.30, 0.90), vec3(0.15, 0.78, 0.92), vec3(0.22, 0.80, 0.30),
        vec3(0.98, 0.84, 0.18), vec3(0.92, 0.22, 0.18));
    let i = u32(min(x, 3.999));
    return mix(stops[i], stops[i + 1u], fract(x));
}
fn layer_of(index: u32) -> u32 {
    var lo = 0u;
    var hi = arrayLength(&layer_starts);
    while hi - lo > 1u {
        let mid = (lo + hi) / 2u;
        if layer_starts[mid] <= index { lo = mid; } else { hi = mid; }
    }
    return lo;
}
struct Move {
    a: vec3<f32>,
    b: vec3<f32>,
    width: f32,
    height: f32,
    attributes: u32,
    extra: vec4<u32>,
    // Record index in the whole preview.
    index: u32,
}
fn fetch(instance: u32) -> Move {
    let local = visible[instance];
    let r = records[local];
    let at = attrs[local];
    var m: Move;
    m.a = vec3(r.ax, r.ay, r.az);
    m.b = vec3(r.bx, r.by, r.bz);
    m.width = f32(r.size & 65535u) / 1000.0;
    m.height = f32(r.size >> 16u) / 1000.0;
    m.attributes = r.kind;
    m.extra = vec4(at.x & 65535u, at.x >> 16u, at.y & 65535u, at.y >> 16u);
    m.index = chunk.x + local;
    return m;
}
fn color_of(m: Move) -> vec3<f32> {
    let role = m.attributes & 15u;
    let tool = (m.attributes >> 4u) & 15u;
    let mode = u.params.y;
    var color = roles[min(role, 15u)] / 255.0;
    if mode == 1u { color = u.tools[tool].rgb; }
    if mode == 12u {
        let packed = u32(layer_values[layer_of(m.index)]);
        let k = select(0u, (packed >> (4u * tool)) & 15u, tool < 6u);
        color = select(u.tools[tool].rgb, u.tools[k].rgb, k != 0u);
        if role == 14u { color = roles[14] / 255.0; }
    }
    if mode >= 2u && mode != 12u {
        var value = m.width;
        if mode == 2u { value = f32(m.attributes >> 16u); }
        if mode == 3u { value = f32((m.attributes >> 8u) & 255u); }
        if mode == 5u { value = m.height; }
        if mode == 6u { value = layer_values[layer_of(m.index)]; }
        if mode == 7u { value = f32(m.extra.x) / 10.0; }
        if mode == 8u { value = f32(m.extra.y) / 100.0; }
        if mode == 9u { value = f32(m.extra.z) / 10.0; }
        if mode == 10u { value = f32(m.extra.w); }
        if mode == 11u { value = log(max(layer_values[layer_of(m.index)], 0.1)); }
        let range = max(u.extra.y - u.extra.x, 0.0001);
        color = gradient((value - u.extra.x) / range);
        if role == 14u { color = roles[14] / 255.0; }
    }
    return color;
}
// The normal of the surface a move builds, seen from afar: walls (outer, overhang, inner)
// face sideways (towards the viewer) and a little up, everything else faces up.
fn is_wall(role: u32) -> bool {
    return role >= 2u && role <= 4u;
}
fn wall_normal(side: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let outward = side * select(-1.0, 1.0, dot(side, u.eye.xyz - p) >= 0.0);
    // normalize(outward + 1.15 up): about the mean brightness of the tube shading it replaces
    return (outward + vec3(0.0, 0.0, 1.15)) * 0.6561787;
}
fn macro_normal(role: u32, side: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    return select(vec3(0.0, 0.0, 1.0), wall_normal(side, p), is_wall(role));
}
// A move's elliptical cross-section (`rx` sideways, `rz` up) seen from `p` towards the eye:
// the direction across the view (in the side / up plane) and the half-extent along it.
// `dir`, `side`, up form a right-handed basis, so cross(dir, view) = view·side up −
// view·up side; the extent is the ellipse's support in that direction.
fn across_view(side: vec3<f32>, rx: f32, rz: f32, p: vec3<f32>) -> vec4<f32> {
    let view = u.eye.xyz - p;
    let vs = dot(view, side);
    let vu = view.z;
    let n = sqrt(vs * vs + vu * vu);
    if n < 1e-6 {
        return vec4(side, rx);
    }
    let across = (side * -vu + vec3(0.0, 0.0, vs)) / n;
    return vec4(across, sqrt(rx * rx * vu * vu + rz * rz * vs * vs) / n);
}
// The period of the stripes a move's neighbours and it make on screen, at `p` (mm): stacked
// walls repeat by the layer height, seen across the view; other moves lie side by side and
// repeat by their width. (Not the tube's silhouette: neighbours cover part of it.)
fn stripe(side: vec3<f32>, width: f32, height: f32, wall: bool, p: vec3<f32>) -> f32 {
    // The offset to the neighbour (up, or sideways) as it appears on screen: its part
    // perpendicular to the view.
    let view = normalize(u.eye.xyz - p);
    let along = select(dot(side, view), view.z, wall);
    return select(width, height, wall) * sqrt(max(1.0 - along * along, 0.0));
}
// A length at `p` in pixels.
fn pixels(length: f32, p: vec3<f32>) -> f32 {
    let depth = select(1.0, max(distance(u.eye.xyz, p), 1e-3), u.eye.w > 0.5);
    return length * u.extra.z / depth;
}
fn tube(vertex: u32, instance: u32) -> Output {
    var o: Output;
    let m = fetch(instance);
    let a = m.a;
    let b = m.b;
    let width = m.width;
    let height = m.height;
    let dir = normalize(b - a + vec3(0.000001, 0.0, 0.0));
    let side = normalize(vec3(-dir.y, dir.x, 0.0) + vec3(0.000001, 0.0, 0.0));
    let up = vec3(0.0, 0.0, 1.0);
    let q = quad[vertex % 6u];
    let ring = vertex / 6u;
    let k = (ring + q.y) % 6u;
    let angle = (f32(k) + 0.5) * 1.0471976;
    let ca = cos(angle);
    let sa = sin(angle);
    let rx = width * 0.5;
    let rz = height * 0.5;
    let ext = dir * min(width, height) * 0.3;
    let base = select(a - ext, b + ext, q.x == 1u) - up * rz;
    let p = (u.model * vec4(base + side * (ca * rx) + up * (sa * rz), 1.0)).xyz;
    o.clip = u.vp * vec4(p, 1.0);
    o.world = p;
    o.normal = normalize(side * (ca / max(rx, 0.001)) + up * (sa / max(rz, 0.001)));
    let wall = is_wall(m.attributes & 15u);
    o.thickness = pixels(stripe(side, width, height, wall, p), p) * select(1.0, -1.0, wall);
    o.id = m.index;
    o.color = color_of(m);
    return o;
}
fn ribbon(vertex: u32, instance: u32) -> Output {
    var o: Output;
    let m = fetch(instance);
    let dir = normalize(m.b - m.a + vec3(0.000001, 0.0, 0.0));
    let side = normalize(vec3(-dir.y, dir.x, 0.0) + vec3(0.000001, 0.0, 0.0));
    let up = vec3(0.0, 0.0, 1.0);
    let q = quad[vertex % 6u];
    let rx = m.width * 0.5;
    let rz = m.height * 0.5;
    let ext = dir * min(m.width, m.height) * 0.3;
    // The tube's axis end, then the quad across it facing the viewer, as wide as the
    // elliptical cross-section's extent in that direction.
    let centre = (u.model * vec4(select(m.a - ext, m.b + ext, q.x == 1u) - up * rz, 1.0)).xyz;
    let extent = across_view(side, rx, rz, centre);
    let s = f32(q.y) * 2.0 - 1.0;
    let p = centre + extent.xyz * s * extent.w;
    o.clip = u.vp * vec4(p, 1.0);
    o.world = p;
    o.across = extent.xyz;
    // Towards the viewer, perpendicular to the move and to `across`.
    o.toward = cross(extent.xyz, dir);
    if dot(o.toward, u.eye.xyz - centre) < 0.0 { o.toward = -o.toward; }
    o.s = s;
    o.normal = o.toward;
    o.face = macro_normal(m.attributes & 15u, side, p);
    let wall = is_wall(m.attributes & 15u);
    o.thickness = pixels(stripe(side, m.width, m.height, wall, centre), centre);
    o.id = m.index;
    o.color = color_of(m);
    // Thin ribbons shade flat: once per vertex.
    o.flat = shade(o.face, p, o.color);
    return o;
}
@vertex fn vs_full(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Tube {
    let o = tube(vertex, instance);
    return Tube(o.clip, o.normal, o.color, o.world, 0.0);
}
@vertex fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Tube {
    let o = tube(vertex, instance);
    return Tube(o.clip, o.normal, o.color, o.world, o.thickness);
}
@vertex fn vs_ribbon(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Ribbon {
    let o = ribbon(vertex, instance);
    return Ribbon(o.clip, o.color, o.world, o.face, o.thickness, o.across, o.toward, o.s, o.flat);
}
@vertex fn vs_id(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Pick {
    let o = tube(vertex, instance);
    return Pick(o.clip, o.world, o.id);
}
@vertex fn vs_ribbon_id(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Pick {
    let o = ribbon(vertex, instance);
    return Pick(o.clip, o.world, o.id);
}
// How much of the tube shading a fragment keeps: none while its stripe is narrower than
// 1.5 px on screen, all of it from 3 px.
const FLAT_BELOW: f32 = 1.5;
const TUBE_FROM: f32 = 3.0;
// The section view's pipelines cut the toolpaths at the section plane (`clipped`); the
// others leave the discard out, so the GPU can drop hidden fragments before shading them.
override sectioned: bool = true;
fn cut(world: vec3<f32>) -> bool {
    return sectioned && clipped(world);
}
@fragment fn fs(o: Tube) -> @location(0) vec4<f32> {
    if cut(o.world) { discard; }
    let thickness = abs(o.thickness);
    if thickness >= TUBE_FROM {
        return vec4(shade(normalize(o.normal), o.world, o.color), 1.0);
    }
    var face = vec3(0.0, 0.0, 1.0);
    let h = o.normal.xy;
    if o.thickness < 0.0 && dot(h, h) > 1e-12 {
        face = wall_normal(vec3(normalize(h), 0.0), o.world);
    }
    let f = smoothstep(FLAT_BELOW, TUBE_FROM, thickness);
    let n = normalize(mix(face, normalize(o.normal), f));
    return vec4(shade(n, o.world, o.color), 1.0);
}
@fragment fn fs_full(o: Tube) -> @location(0) vec4<f32> {
    if cut(o.world) { discard; }
    let n = normalize(o.normal);
    return vec4(shade(n, o.world, o.color), 1.0);
}
@fragment fn fs_ribbon(o: Ribbon) -> @location(0) vec4<f32> {
    if cut(o.world) { discard; }
    if o.thickness <= FLAT_BELOW {
        return vec4(o.flat, 1.0);
    }
    let f = smoothstep(FLAT_BELOW, TUBE_FROM, o.thickness);
    let s = clamp(o.s, -1.0, 1.0);
    let tube = normalize(o.across) * s + normalize(o.toward) * sqrt(max(1.0 - s * s, 0.0));
    let n = normalize(mix(normalize(o.face), tube, f));
    return vec4(shade(n, o.world, o.color), 1.0);
}
@fragment fn fs_id(o: Pick) -> @location(0) vec4<u32> {
    if cut(o.world) { discard; }
    return vec4(o.id + 1u, 0u, 0u, 0u);
}
