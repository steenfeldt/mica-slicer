#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate arm-out T and figurine-like hanging-sphere STL support fixtures.

Usage: python3 tests/models/support_organic.py [output_directory]
The figurine has a tall torso, outstretched arms and a sphere hanging by a narrow neck.
All component shells are closed and outward oriented; intersecting shells are unioned by slicing.
"""
import math
import struct
import sys
from pathlib import Path
sys.dont_write_bytecode = True
from support_overhangs import write_model


def sphere(center, radius, rings=24, segments=48):
    def point(i, j):
        theta, phi = math.pi*i/rings, math.tau*j/segments
        return (center[0]+radius*math.sin(theta)*math.cos(phi),
                center[1]+radius*math.sin(theta)*math.sin(phi),
                center[2]+radius*math.cos(theta))
    triangles = []
    for i in range(rings):
        for j in range(segments):
            a,b,c,d = point(i,j),point(i+1,j),point(i+1,(j+1)%segments),point(i,(j+1)%segments)
            if i < rings-1:
                triangles.append((a,b,c))
            if i > 0:
                triangles.append((a,c,d))
    return triangles


def main(directory):
    directory.mkdir(parents=True, exist_ok=True)
    torso = [(-4,-4,0,4,4,40),(-28,-4,34,28,4,40)]
    write_model(directory/'support_arms.stl', torso)
    path = directory/'support_figurine.stl'
    write_model(path, torso+[(21,-1,26,23,1,35)])
    data = path.read_bytes()
    triangles = sphere((22,0,22),6)+sphere((0,0,45),6)
    with path.open('wb') as out:
        out.write(data[:80])
        out.write(struct.pack('<I',struct.unpack('<I',data[80:84])[0]+len(triangles)))
        out.write(data[84:])
        for tri in triangles:
            a,b,c = tri
            u,v = [b[i]-a[i] for i in range(3)],[c[i]-a[i] for i in range(3)]
            n = (u[1]*v[2]-u[2]*v[1],u[2]*v[0]-u[0]*v[2],u[0]*v[1]-u[1]*v[0])
            length = sum(x*x for x in n)**.5
            out.write(struct.pack('<12fH',*(x/length for x in n),*(x for p in tri for x in p),0))


if __name__ == '__main__':
    main(Path(sys.argv[1]) if len(sys.argv)>1 else Path(__file__).parent)
