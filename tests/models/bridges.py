#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate bridge oracles in a supplied temp directory (no checked-in binary meshes).

Pillars span 20 mm at 0/30/90 degrees. The solid box has a sparse printed core;
its first roof skin exercises internal bridges. The cavity is geometric air.
The final bridge has a through-hole in its roof and a bed-supported base.
"""
import math
import struct
import sys
from pathlib import Path
from support_overhangs import write_model


def rotate_stl(path, degrees):
    data = bytearray(path.read_bytes())
    sine, cosine = math.sin(math.radians(degrees)), math.cos(math.radians(degrees))
    for i in range(struct.unpack_from('<I', data, 80)[0]):
        for j in range(4):  # normal and the three vertices
            pos = 84 + i * 50 + j * 12
            x, y = struct.unpack_from('<2f', data, pos)
            struct.pack_into('<2f', data, pos, cosine*x-sine*y, sine*x+cosine*y)
    path.write_bytes(data)


def main(directory):
    directory.mkdir(parents=True, exist_ok=True)
    pillars = [(0,0,0,5,12,10), (25,0,0,30,12,10), (0,0,10,30,12,12)]
    for angle in (0, 30, 90):
        path = directory / f'pillars_{angle}.stl'
        write_model(path, pillars)
        rotate_stl(path, angle)
    write_model(directory / 'internal_roof.stl', [(0,0,0,50,40,12)])
    write_model(directory / 'cavity.stl', [
        (0,0,0,40,30,2), (0,0,2,3,30,12), (37,0,2,40,30,12),
        (3,0,2,37,3,12), (3,27,2,37,30,12), (0,0,12,40,30,14)])
    write_model(directory / 'hole_overhang.stl', [
        (0,0,0,30,20,2), (0,0,2,5,20,10), (25,0,2,30,20,10),
        (0,0,10,30,7,12), (0,13,10,30,20,12),
        (0,7,10,12,13,12), (18,7,10,30,13,12)])


if __name__ == '__main__':
    main(Path(sys.argv[1]))
