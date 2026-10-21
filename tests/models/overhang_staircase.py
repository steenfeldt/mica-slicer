#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Write a closed staircase of seven 4 mm rises, inclined 20..80 degrees from vertical.

Usage: python3 tests/models/overhang_staircase.py /tmp/overhang_staircase.stl
The 12 mm wide extrusion has a flat rear and a sloped front; slopes meet at the risers.
No generated mesh or oracle G-code is stored in the repository.
"""
import math
import sys


def generate(path):
    profile = [(0., 0.), (12., 0.), (12., 2.)]
    x, z = 12., 2.
    for angle in range(20, 81, 10):
        x += 4*math.tan(math.radians(angle))
        z += 4
        profile.append((x, z))
    profile.append((0., z))
    # Polygon is CCW in X/Z. A fan from the top rear corner triangulates both caps
    # without overlaps or inverted faces.
    points = [(x, y, z) for y in (0., 12.) for x, z in profile]
    n = len(profile)
    triangles = []
    for i in range(n):
        j = (i+1) % n
        triangles.extend([(i, n+i, n+j), (i, n+j, j)])
    for i in range(n-2):
        triangles.extend([(n-1, i, i+1), (2*n-1, n+i+1, n+i)])
    with open(path, 'w') as f:
        f.write('solid overhang_staircase\n')
        for face in triangles:
            f.write('facet normal 0 0 0\n outer loop\n')
            for i in face:
                f.write('  vertex %g %g %g\n' % points[i])
            f.write(' endloop\nendfacet\n')
        f.write('endsolid overhang_staircase\n')



if __name__ == '__main__':
    generate(sys.argv[1])
