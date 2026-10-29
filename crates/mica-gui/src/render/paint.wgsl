// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Painted facets over an object; the pipeline's depth bias makes them win the depth test
// against the face they lie on. Colour painting (pattern 0) is the facet's
// own colour, solid and lit like the mesh. Every other kind is a screen-space pattern over
// the model's base colour: a diagonal hatch (support), stripes (seam) or a stipple (fuzzy
// skin), blended at the vertex colour's alpha (full for the active tool, half alongside
// another tool). Between the pattern's marks the fragment is discarded, so the filament or
// colour-painting colour underneath stays visible and recognisable at any zoom. Colour
// painting carries its filament's alpha (translucent filaments blend). `fs_solid` and
// `fs_marks` draw only the colour painting or only the patterns (a translucent object's paint,
// around its own colour pass).
struct Input {
    @location(0) p: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) pattern: u32,
}
struct Output {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) pattern: u32,
}
@vertex fn vs(v: Input) -> Output {
    var o: Output;
    let world = (u.model * vec4(v.p, 1.0)).xyz;
    o.world = world;
    o.color = v.color;
    o.pattern = v.pattern;
    o.clip = u.vp * vec4(world, 1.0);
    return o;
}
// A small integer hash for the stipple: the same dot for the same pixel cell every frame.
fn cell_hash(c: vec2<u32>) -> f32 {
    var h = c.x * 374761393u + c.y * 668265263u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xffffu) / 65535.0;
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    return painted(o);
}
@fragment fn fs_solid(o: Output) -> @location(0) vec4<f32> {
    if o.pattern != 0u { discard; }
    return painted(o);
}
@fragment fn fs_marks(o: Output) -> @location(0) vec4<f32> {
    if o.pattern == 0u { discard; }
    return painted(o);
}
fn painted(o: Output) -> vec4<f32> {
    if clipped(o.world) { discard; }
    let n = normalize(cross(dpdx(o.world), dpdy(o.world)));
    // A touch of self-illumination keeps the paint colour readable in shadow.
    let lit = shade(n, o.world, o.color.rgb);
    if o.pattern == 0u {
        return vec4(mix(lit, o.color.rgb, 0.25), o.color.a);
    }
    let px = o.clip.xy;
    var mark = false;
    var tint = o.color.rgb;
    if o.pattern == 1u {
        // Hatch: lines every 7 px, about a third covered.
        mark = fract((px.x + px.y) / 7.0) < 0.36;
    } else if o.pattern == 2u {
        // Stripes the other way, every 6 px, half covered.
        mark = fract((px.x - px.y) / 6.0) < 0.5;
    } else {
        // Stipple: one dot in roughly every other 2 px cell, lightened towards white.
        let cell = vec2<u32>(u32(px.x) / 2u, u32(px.y) / 2u);
        mark = cell_hash(cell) < 0.42;
        tint = mix(o.color.rgb, vec3(1.0, 1.0, 0.92), 0.35);
    }
    if !mark {
        discard;
    }
    return vec4(mix(shade(n, o.world, tint), tint, 0.45), o.color.a);
}
