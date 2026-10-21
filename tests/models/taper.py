#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate a watertight 40 mm taper, 0.15–2.4 mm wide and 5 mm high.

Exercises thin-feature widening and multiple odd/even bead transitions.
Usage: python3 tests/models/taper.py [output.stl]
"""
import math
import pathlib
import struct
import sys


def generate(path):
    polygon = [(-20., -.075), (20., -1.2), (20., 1.2), (-20., .075)]
    bottom = [(x, y, 0.) for x, y in polygon]
    top = [(x, y, 5.) for x, y in polygon]
    triangles = [(bottom[0], bottom[2], bottom[1]), (bottom[0], bottom[3], bottom[2]),
                 (top[0], top[1], top[2]), (top[0], top[2], top[3])]
    for i in range(4):
        j = (i + 1) % 4
        triangles.extend([(bottom[i], bottom[j], top[j]), (bottom[i], top[j], top[i])])
    with open(path, "wb") as stream:
        stream.write(b"Mica original Arachne taper regression".ljust(80, b" "))
        stream.write(struct.pack("<I", len(triangles)))
        for a, b, c in triangles:
            u = [b[i]-a[i] for i in range(3)]
            v = [c[i]-a[i] for i in range(3)]
            n = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]]
            length = math.sqrt(sum(x*x for x in n))
            stream.write(struct.pack("<12fH", *(x/length for x in n), *a, *b, *c, 0))


if __name__ == "__main__":
    generate(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).with_suffix(".stl"))
