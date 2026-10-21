#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate watertight T and bridge support oracles, with a roof at Z=10 mm.

Usage: python3 tests/models/support_overhangs.py [output_directory]
The shared coordinate grid emits only exposed faces, with no internal faces or T-junctions.
"""
import struct
import sys
from pathlib import Path


def write_model(path, boxes):
    axes = [sorted({b[axis + side] for b in boxes for side in (0, 3)}) for axis in range(3)]
    xs, ys, zs = axes
    occupied = set()
    for i in range(len(xs)-1):
        for j in range(len(ys)-1):
            for k in range(len(zs)-1):
                mid = [(axis[index]+axis[index+1])/2 for axis, index in zip(axes, (i,j,k))]
                if any(all(b[a] < mid[a] < b[a+3] for a in range(3)) for b in boxes):
                    occupied.add((i,j,k))
    triangles = []
    def quad(a,b,c,d):
        triangles.extend(((a,b,c),(a,c,d)))
    for i,j,k in sorted(occupied):
        x0,x1,y0,y1,z0,z1=xs[i],xs[i+1],ys[j],ys[j+1],zs[k],zs[k+1]
        if (i,j,k-1) not in occupied:
            quad((x0,y0,z0),(x0,y1,z0),(x1,y1,z0),(x1,y0,z0))
        if (i,j,k+1) not in occupied:
            quad((x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1))
        if (i-1,j,k) not in occupied:
            quad((x0,y0,z0),(x0,y0,z1),(x0,y1,z1),(x0,y1,z0))
        if (i+1,j,k) not in occupied:
            quad((x1,y0,z0),(x1,y1,z0),(x1,y1,z1),(x1,y0,z1))
        if (i,j-1,k) not in occupied:
            quad((x0,y0,z0),(x1,y0,z0),(x1,y0,z1),(x0,y0,z1))
        if (i,j+1,k) not in occupied:
            quad((x0,y1,z0),(x0,y1,z1),(x1,y1,z1),(x1,y1,z0))
    with path.open('wb') as f:
        f.write(b'Mica support oracle: generated orthogonal solid'.ljust(80,b' '))
        f.write(struct.pack('<I',len(triangles)))
        for tri in triangles:
            a, b, c = tri
            u, v = [b[i]-a[i] for i in range(3)], [c[i]-a[i] for i in range(3)]
            normal = (u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0])
            length = sum(x*x for x in normal)**0.5
            f.write(struct.pack('<12fH', *(x/length for x in normal), *(v for point in tri for v in point), 0))


def main(directory):
    directory.mkdir(parents=True,exist_ok=True)
    write_model(directory/'support_t.stl', [(10,0,0,20,8,10),(0,0,10,30,8,12)])
    write_model(directory/'support_bridge.stl', [(0,0,0,5,8,10),(25,0,0,30,8,10),(0,0,10,30,8,12)])


if __name__ == '__main__':
    main(Path(sys.argv[1]) if len(sys.argv)>1 else Path(__file__).parent)
