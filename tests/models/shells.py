#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes shells.stl: a stepped pyramid and a truncated cone side by side, both 12 mm tall.

The pyramid has three square steps (24, 16 and 8 mm wide, 4 mm tall each), so every step has a top surface
whose solid layers must be carried down into the step below (horizontal shells). The cone narrows from
20 mm to 6 mm across its height; its sloping wall moves inwards by 0.117 mm per 0.2 mm layer, so the walls of
the layers above sit over this layer's infill and "ensure vertical shell thickness" adds solid infill along
the whole wall.

Both solids are closed, consistently oriented triangle meshes. Usage: python3 shells.py [out.stl]
"""

import math
import struct
import sys

STEPS = [(24.0, 0.0, 4.0), (16.0, 4.0, 8.0), (8.0, 8.0, 12.0)]  # width, z0, z1
PYRAMID_CENTRE = (0.0, 0.0)
CONE_CENTRE = (30.0, 0.0)
CONE = (10.0, 3.0, 12.0)  # bottom radius, top radius, height
SEGMENTS = 96


def square(c, w, z):
    """Counter-clockwise corners of a w-wide square centred on c at height z."""
    h = w / 2
    return [(c[0] - h, c[1] - h, z), (c[0] + h, c[1] - h, z), (c[0] + h, c[1] + h, z), (c[0] - h, c[1] + h, z)]


def pyramid(tris):
    c = PYRAMID_CENTRE
    bottom = square(c, STEPS[0][0], 0.0)
    # Bottom face, seen from below (clockwise from above).
    tris.append((bottom[0], bottom[2], bottom[1]))
    tris.append((bottom[0], bottom[3], bottom[2]))
    for k, (w, z0, z1) in enumerate(STEPS):
        lo, hi = square(c, w, z0), square(c, w, z1)
        for i in range(4):
            j = (i + 1) % 4
            tris.append((lo[i], lo[j], hi[j]))
            tris.append((lo[i], hi[j], hi[i]))
        if k + 1 < len(STEPS):
            # Ring between this step's top and the next step's foot, as four trapezoids.
            inner = square(c, STEPS[k + 1][0], z1)
            for i in range(4):
                j = (i + 1) % 4
                tris.append((hi[i], hi[j], inner[j]))
                tris.append((hi[i], inner[j], inner[i]))
        else:
            tris.append((hi[0], hi[1], hi[2]))
            tris.append((hi[0], hi[2], hi[3]))


def cone(tris):
    cx, cy = CONE_CENTRE
    r0, r1, h = CONE
    ring = lambda r, z: [
        (cx + r * math.cos(2 * math.pi * i / SEGMENTS), cy + r * math.sin(2 * math.pi * i / SEGMENTS), z)
        for i in range(SEGMENTS)
    ]
    lo, hi = ring(r0, 0.0), ring(r1, h)
    b, t = (cx, cy, 0.0), (cx, cy, h)
    for i in range(SEGMENTS):
        j = (i + 1) % SEGMENTS
        tris.append((b, lo[j], lo[i]))
        tris.append((t, hi[i], hi[j]))
        tris.append((lo[i], lo[j], hi[j]))
        tris.append((lo[i], hi[j], hi[i]))


def normal(a, b, c):
    u = [b[k] - a[k] for k in range(3)]
    v = [c[k] - a[k] for k in range(3)]
    n = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])
    length = math.sqrt(sum(x * x for x in n)) or 1.0
    return tuple(x / length for x in n)


def main(out):
    tris = []
    pyramid(tris)
    cone(tris)
    with open(out, "wb") as f:
        f.write(b"Mica Slicer test model: stepped pyramid and cone".ljust(80, b" "))
        f.write(struct.pack("<I", len(tris)))
        for a, b, c in tris:
            f.write(struct.pack("<3f", *normal(a, b, c)))
            for p in (a, b, c):
                f.write(struct.pack("<3f", *p))
            f.write(b"\0\0")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "shells.stl")
