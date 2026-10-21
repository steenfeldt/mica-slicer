// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Objects and the bed sheet model: flat per-triangle normals from derivatives, key + fill +
// hemisphere lighting. params.x flags: 1 hovered, 2 warning stripes over the whole object (its
// skirt or brim does not fit), 16 a section cap (flat, hatched), 32 the object reaches outside
// the printable volume: the fragments outside it (`outside_volume`) get fine red stripes, the
// rest keeps its look. Fragments beyond the section plane (u.clip) are discarded.
struct Output { @builtin(position) clip: vec4<f32>, @location(0) world: vec3<f32> }
@vertex fn vs(@location(0) p: vec3<f32>) -> Output {
    var o: Output;
    let world = u.model * vec4(p, 1.0);
    o.world = world.xyz;
    o.clip = u.vp * world;
    return o;
}
@fragment fn fs(o: Output) -> @location(0) vec4<f32> {
    if clipped(o.world) { discard; }
    let n = normalize(cross(dpdx(o.world), dpdy(o.world)));
    var albedo = u.color.rgb;
    if (u.params.x & 16u) != 0u {
        // A section cap: the cut face, flat-lit with fine diagonal hatching.
        let stripe = step(0.72, fract((o.clip.x - o.clip.y) / 9.0));
        let c = mix(u.color.rgb, u.color.rgb * 0.62, stripe);
        return vec4(c * (0.78 + 0.22 * max(dot(n, normalize(u.light.xyz)), 0.0)), 1.0);
    }
    if (u.params.x & 2u) != 0u {
        let stripe = step(0.5, fract((o.world.x + o.world.y + o.world.z) / 6.0));
        // Warning stripes alternate with dark grey so they read on any filament colour.
        albedo = mix(u.warning.rgb, vec3(0.22, 0.2, 0.2), stripe);
    }
    if (u.params.x & 1u) != 0u { albedo = albedo * 1.12 + vec3(0.04); }
    if (u.params.x & 32u) != 0u && outside_volume(o.world) {
        // Fine diagonal red stripes in screen space (every 6 px, 3 px wide) over the part
        // outside the volume; between them the model's own colour, a little dimmed.
        if fract((o.clip.x + o.clip.y) / 6.0) < 0.5 {
            return vec4(mix(shade(n, o.world, u.danger.rgb), u.danger.rgb, 0.45), 1.0);
        }
        albedo = albedo * 0.82;
    }
    return vec4(shade(n, o.world, albedo), 1.0);
}
// Translucent volumes (modifiers, negative volumes, range slabs, the cut plane): lit with the
// uniform colour's alpha. params.x flags: 1 hovered, 4 cut-out stripes (negative volumes),
// 8 selected (brighter, more opaque).
@fragment fn fs_volume(o: Output) -> @location(0) vec4<f32> {
    if clipped(o.world) { discard; }
    let n = normalize(cross(dpdx(o.world), dpdy(o.world)));
    var albedo = u.color.rgb;
    var alpha = u.color.a;
    if (u.params.x & 4u) != 0u {
        // Screen-space diagonal stripes read as "material taken away".
        let stripe = step(0.5, fract((o.clip.x + o.clip.y) / 14.0));
        albedo = mix(albedo, albedo * 0.35, stripe);
        alpha = mix(alpha, alpha * 1.3, stripe);
    }
    if (u.params.x & 8u) != 0u { albedo = albedo * 1.1 + vec3(0.05); alpha = min(alpha + 0.12, 1.0); }
    if (u.params.x & 1u) != 0u { albedo = albedo * 1.12 + vec3(0.04); }
    return vec4(shade(n, o.world, albedo), alpha);
}
// Selection / hover mask: constant value into the mask's red channel (drawn without a depth
// test, so a selected object's outline shows through what is in front of it).
@fragment fn fs_mask(o: Output) -> @location(0) vec4<f32> {
    if clipped(o.world) { discard; }
    return vec4(u.color.r, 0.0, 0.0, 1.0);
}
// Outside-part mask: every object with a depth test, 1 into the green channel where the
// visible fragment is outside the printable volume (flag 32), else 0.
@fragment fn fs_mask_outside(o: Output) -> @location(0) vec4<f32> {
    if clipped(o.world) { discard; }
    let outside = (u.params.x & 32u) != 0u && outside_volume(o.world);
    return vec4(0.0, select(0.0, 1.0, outside), 0.0, 1.0);
}
