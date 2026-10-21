#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare adhesion filament and extrusion-run bounding boxes, including circular arcs.

Usage: compare-adhesion.py reference.gcode mica.gcode [--bbox-tolerance 0.06]
Loops are matched by geometry, independently of travel ordering or start vertex.
Inputs must use relative E (the CORE One and U1 profiles do).
"""
import argparse
import math
import re
import json


def parse(path):
    x = y = z = 0.0
    role = None
    points = []
    filament = 0.0
    loops = []

    def finish():
        if points:
            loops.append((min(p[0] for p in points), min(p[1] for p in points),
                          max(p[0] for p in points), max(p[1] for p in points)))
            points.clear()

    for line in open(path):
        if line.startswith(';TYPE:'):
            new_role = line[6:].strip()
            if new_role != role:
                finish()
            role = new_role
        if not re.match(r'G[0123] ', line):
            continue
        cmd = line.split()[0]
        w = {key: float(value) for key, value in re.findall(r'([XYZEIJ])(-?[\d.]+)', line.split(';')[0])}
        nx, ny, nz = w.get('X', x), w.get('Y', y), w.get('Z', z)
        if role in ('Skirt/Brim', 'Skirt', 'Brim') and w.get('E', 0) > 0 and ('X' in w or 'Y' in w):
            filament += w['E']
            points.extend([(x, y), (nx, ny)])
            if cmd in ('G2', 'G3'):
                cx, cy = x + w.get('I', 0), y + w.get('J', 0)
                r = math.hypot(x-cx, y-cy)
                a, b = math.atan2(y-cy, x-cx), math.atan2(ny-cy, nx-cx)
                sign = 1 if cmd == 'G3' else -1
                sweep = (sign*(b-a)) % math.tau
                for k in range(4):
                    angle = k*math.pi/2
                    if (sign*(angle-a)) % math.tau <= sweep + 1e-9:
                        points.append((cx+r*math.cos(angle), cy+r*math.sin(angle)))
        elif ('X' in w or 'Y' in w or 'Z' in w) or w.get('E', 0) < 0:
            finish()
        x, y, z = nx, ny, nz
    finish()
    return filament, loops


def compare(reference, mica):
    re, rl = parse(reference)
    me, ml = parse(mica)
    remaining = list(ml)
    errors = []
    pairs = []
    for box in rl:
        if not remaining:
            break
        best = min(range(len(remaining)), key=lambda i: max(abs(a-b) for a,b in zip(box, remaining[i])))
        other = remaining.pop(best)
        error = max(abs(a-b) for a,b in zip(box, other))
        errors.append(error)
        pairs.append({'reference': box, 'mica': other, 'max_error_mm': error})
    return {'reference_filament_mm': re, 'mica_filament_mm': me,
            'filament_difference_percent': (me/re-1)*100 if re else 0 if not me else None,
            'reference_loops': len(rl), 'mica_loops': len(ml),
            'maximum_bbox_error_mm': max(errors, default=0), 'loops': pairs}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference')
    parser.add_argument('mica')
    parser.add_argument('--bbox-tolerance', type=float, default=0.06)
    args = parser.parse_args()
    result = compare(args.reference, args.mica)
    print(json.dumps(result, indent=2))
    delta = result['filament_difference_percent']
    raise SystemExit(0 if delta is not None and abs(delta) <= 2 and
                     result['reference_loops'] == result['mica_loops'] and
                     result['maximum_bbox_error_mm'] <= args.bbox_tolerance else 1)
