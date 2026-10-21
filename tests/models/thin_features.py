#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes thin_features.stl: a 20 x 20 x 5 mm block with three closed slots (0.5, 0.8 and 1.2 mm wide,
14 mm long) and a free-standing 0.6 mm thin wall next to it.

The slots are placed so that the ribs between them are 1.0 mm (slot 1-2) and 2.0 mm (slot 2-3) wide, which
leaves gaps between the wall loops for gap fill; the 0.6 mm wall is too thin for a wall loop and is only
printed as a thin wall.

The solid is built on a grid of all feature coordinates: every grid cell is either filled or empty, top and
bottom faces are emitted per filled cell and side faces on every cell edge between filled and empty, so the
mesh is watertight without T-junctions. Usage: python3 thin_features.py [out.stl]
"""

import struct
import sys

HEIGHT = 5.0
BLOCK = (0.0, 0.0, 20.0, 20.0)
SLOTS = [  # x0, y0, x1, y1
    (8.0, 3.0, 8.5, 17.0),  # 0.5 mm
    (9.5, 3.0, 10.3, 17.0),  # 0.8 mm
    (12.3, 3.0, 13.5, 17.0),  # 1.2 mm
]
WALL = (23.0, 2.0, 23.6, 18.0)  # 0.6 mm thin wall


def inside(r, x, y):
    return r[0] <= x <= r[2] and r[1] <= y <= r[3]


def filled(x, y):
    if inside(WALL, x, y):
        return True
    return inside(BLOCK, x, y) and not any(inside(s, x, y) for s in SLOTS)


def main(out):
    rects = [BLOCK, WALL] + SLOTS
    xs = sorted({v for r in rects for v in (r[0], r[2])})
    ys = sorted({v for r in rects for v in (r[1], r[3])})
    nx, ny = len(xs) - 1, len(ys) - 1

    def cell(i, j):
        if not (0 <= i < nx and 0 <= j < ny):
            return False
        return filled((xs[i] + xs[i + 1]) / 2, (ys[j] + ys[j + 1]) / 2)

    tris = []

    def quad(a, b, c, d):
        # Counter-clockwise seen from outside.
        tris.append((a, b, c))
        tris.append((a, c, d))

    for i in range(nx):
        for j in range(ny):
            if not cell(i, j):
                continue
            x0, x1, y0, y1 = xs[i], xs[i + 1], ys[j], ys[j + 1]
            quad((x0, y0, HEIGHT), (x1, y0, HEIGHT), (x1, y1, HEIGHT), (x0, y1, HEIGHT))
            quad((x0, y0, 0.0), (x0, y1, 0.0), (x1, y1, 0.0), (x1, y0, 0.0))
            if not cell(i - 1, j):
                quad((x0, y0, 0.0), (x0, y0, HEIGHT), (x0, y1, HEIGHT), (x0, y1, 0.0))
            if not cell(i + 1, j):
                quad((x1, y0, 0.0), (x1, y1, 0.0), (x1, y1, HEIGHT), (x1, y0, HEIGHT))
            if not cell(i, j - 1):
                quad((x0, y0, 0.0), (x1, y0, 0.0), (x1, y0, HEIGHT), (x0, y0, HEIGHT))
            if not cell(i, j + 1):
                quad((x0, y1, 0.0), (x0, y1, HEIGHT), (x1, y1, HEIGHT), (x1, y1, 0.0))

    def normal(t):
        (ax, ay, az), (bx, by, bz), (cx, cy, cz) = t
        u = (bx - ax, by - ay, bz - az)
        v = (cx - ax, cy - ay, cz - az)
        n = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])
        length = sum(c * c for c in n) ** 0.5
        return tuple(c / length for c in n)

    with open(out, "wb") as f:
        f.write(b"Mica Slicer test model: thin_features".ljust(80, b" "))
        f.write(struct.pack("<I", len(tris)))
        for t in tris:
            f.write(struct.pack("<3f", *normal(t)))
            for v in t:
                f.write(struct.pack("<3f", *v))
            f.write(b"\0\0")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "thin_features.stl")
