#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate a watertight 40 mm tall twisted, flared vase body (solid; spiral vase prints its shell).

The outline is a six-lobed rounded star whose radius flares from 18 to 24 mm and which twists
30 degrees over the height, so the spiral's Z ramp and its XY smoothing both have work to do.
Usage: python3 tests/models/vase.py [output.stl]
"""
import math
import pathlib
import struct
import sys

HEIGHT = 40.
LAYERS = 40     # rings along the height
SEGMENTS = 120  # points around


def ring(k):
    z = HEIGHT * k / LAYERS
    t = z / HEIGHT
    base = 18. + 6. * math.sin(t * math.pi / 2)
    twist = math.radians(30.) * t
    points = []
    for i in range(SEGMENTS):
        a = 2 * math.pi * i / SEGMENTS
        r = base + 1.6 * math.cos(6 * (a - twist))
        points.append((round(r * math.cos(a), 5), round(r * math.sin(a), 5), round(z, 5)))
    return points


def generate(path):
    rings = [ring(k) for k in range(LAYERS + 1)]
    triangles = []
    for k in range(LAYERS):
        lo, hi = rings[k], rings[k + 1]
        for i in range(SEGMENTS):
            j = (i + 1) % SEGMENTS
            triangles.append((lo[i], lo[j], hi[j]))
            triangles.append((lo[i], hi[j], hi[i]))
    bottom = (0., 0., 0.)
    top = (0., 0., HEIGHT)
    for i in range(SEGMENTS):
        j = (i + 1) % SEGMENTS
        triangles.append((bottom, rings[0][j], rings[0][i]))
        triangles.append((top, rings[-1][i], rings[-1][j]))
    with open(path, "wb") as stream:
        stream.write(b"Mica original spiral vase test body".ljust(80, b" "))
        stream.write(struct.pack("<I", len(triangles)))
        for a, b, c in triangles:
            u = [b[i] - a[i] for i in range(3)]
            v = [c[i] - a[i] for i in range(3)]
            n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
            length = math.sqrt(sum(x * x for x in n)) or 1.
            stream.write(struct.pack("<12fH", *(x / length for x in n), *a, *b, *c, 0))


if __name__ == "__main__":
    generate(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).with_suffix(".stl"))
