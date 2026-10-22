#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare two spiral vase G-code files (a reference slicer's and Mica's).

For every layer: the extruded XY length, the filament, the Z range of its extrusions and how far
each extrusion's Z is from a straight ramp over the layer (Z per segment along the spiral). Prints
a summary and, with --layers, the per-segment ramp of chosen layers.
Usage: python3 scripts/compare-vase.py REFERENCE.gcode MICA.gcode [--layers 4,5,100,-1]
"""
import argparse
import math


def layers(path):
    """[(marker_z, [(x0, y0, x1, y1, z, e)])] per ;LAYER_CHANGE, extrusion moves only."""
    out = []
    x = y = z = 0.
    current = None
    for line in open(path, encoding='utf-8', errors='replace'):
        line = line.strip()
        if line in (';LAYER_CHANGE', '; CHANGE_LAYER'):
            current = [None, []]
            out.append(current)
            continue
        if current is not None and current[0] is None and line.startswith(';Z:'):
            current[0] = float(line[3:])
            continue
        if not line.startswith(('G1 ', 'G0 ')):
            continue
        words = dict((w[0], w[1:]) for w in line.split(';')[0].split()[1:] if w)
        nx = float(words['X']) if 'X' in words else x
        ny = float(words['Y']) if 'Y' in words else y
        nz = float(words['Z']) if 'Z' in words else z
        e = float(words['E']) if 'E' in words else 0.
        if current is not None and e > 0 and (nx != x or ny != y):
            current[1].append((x, y, nx, ny, nz, e))
        x, y, z = nx, ny, nz
    return out


def summary(data):
    rows = []
    previous = 0.
    for z, moves in data:
        length = sum(math.hypot(m[2] - m[0], m[3] - m[1]) for m in moves)
        zs = [m[4] for m in moves]
        spiral = bool(zs) and min(zs) < (z or 0) - 0.001
        deviation = 0.
        if spiral and length > 0:
            done = 0.
            for m in moves:
                done += math.hypot(m[2] - m[0], m[3] - m[1])
                deviation = max(deviation, abs(m[4] - (previous + (z - previous) * done / length)))
        rows.append(dict(z=z, length=length, e=sum(m[5] for m in moves), segments=len(moves),
                         spiral=spiral, zmin=min(zs, default=0), zmax=max(zs, default=0),
                         deviation=deviation))
        if z is not None:
            previous = z
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference')
    parser.add_argument('mica')
    parser.add_argument('--layers', default='')
    args = parser.parse_args()
    data = [layers(args.reference), layers(args.mica)]
    rows = [summary(d) for d in data]
    for name, r in zip(('reference', 'mica'), rows):
        spiral = [x for x in r if x['spiral']]
        print(f"{name}: {len(r)} layers, {len(spiral)} spiral, first spiral layer "
              f"{next((i for i, x in enumerate(r) if x['spiral']), None)}, filament "
              f"{sum(x['e'] for x in r):.2f} mm, max deviation from a straight ramp "
              f"{max((x['deviation'] for x in spiral), default=0):.4f} mm")
    common = min(len(rows[0]), len(rows[1]))
    worst = max(range(common), key=lambda i: abs(rows[0][i]['e'] - rows[1][i]['e']))
    print(f"largest per-layer filament delta: layer {worst + 1} "
          f"{rows[0][worst]['e']:.4f} vs {rows[1][worst]['e']:.4f} mm")
    for spec in filter(None, args.layers.split(',')):
        i = int(spec)
        for name, d in zip(('reference', 'mica'), data):
            z, moves = d[i]
            length = sum(math.hypot(m[2] - m[0], m[3] - m[1]) for m in moves) or 1
            done = 0.
            ramp = []
            for m in moves:
                done += math.hypot(m[2] - m[0], m[3] - m[1])
                ramp.append(f"{done / length:.2f}:{m[4]:.3f}")
            print(f"layer {i} {name} Z{z}: {len(moves)} segments, {length:.2f} mm; "
                  f"fraction:Z {' '.join(ramp[:6])} … {' '.join(ramp[-3:])}")


if __name__ == '__main__':
    main()
