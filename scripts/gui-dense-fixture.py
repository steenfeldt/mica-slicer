#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate a watertight 3,800,000-triangle sphere for the GUI memory check.

Usage: python3 scripts/gui-dense-fixture.py /tmp/mica-dense.stl
No external libraries. Output must live outside the repository.
"""
import math
import struct
import sys

COLUMNS, RINGS = 1900, 1000

def ring(i):
    phi = math.pi * i / (RINGS + 1)
    radius, z = 30 * math.sin(phi), 30 + 30 * math.cos(phi)
    return [(radius * math.cos(2 * math.pi * j / COLUMNS),
             radius * math.sin(2 * math.pi * j / COLUMNS), z)
            for j in range(COLUMNS)]

with open(sys.argv[1], "wb") as out:
    out.write(b"Mica original dense sphere".ljust(80, b"\0"))
    out.write(struct.pack("<I", 2 * COLUMNS * RINGS))
    def triangle(a, b, c):
        out.write(struct.pack("<12fH", 0, 0, 0, *a, *b, *c, 0))
    previous = ring(1)
    for j in range(COLUMNS):
        triangle((0, 0, 60), previous[j], previous[(j+1) % COLUMNS])
    for i in range(2, RINGS+1):
        current = ring(i)
        for j in range(COLUMNS):
            k = (j+1) % COLUMNS
            triangle(previous[j], current[j], current[k])
            triangle(previous[j], current[k], previous[k])
        previous = current
    for j in range(COLUMNS):
        triangle(previous[j], (0, 0, 0), previous[(j+1) % COLUMNS])
print("3,800,000 triangles written to", sys.argv[1])
