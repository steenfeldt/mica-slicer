#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare per-Z support filament (mm) in two text G-code files.

Usage: python3 scripts/compare-supports.py reference.gcode mica.gcode [report.csv]
Uses ;Z: logical print heights so travel lifts do not become layers. Handles absolute/relative E,
G92, G2/G3 arcs and role transitions; retraction/deretraction and non-XY priming moves are excluded.
"""
import csv
import re
import sys
from collections import defaultdict

ROLES = ('Support material', 'Support material interface')


def read(path):
    layers = defaultdict(lambda: [0.0, 0.0])
    relative = False
    e = 0.0
    x = y = 0.0
    z = None
    role = ''
    with open(path) as f:
        for line in f:
            if line.startswith(';Z:'):
                z = round(float(line[3:]), 3)
            elif line.startswith(';TYPE:'):
                role = line[6:].strip()
            code = line.split(';', 1)[0].strip()
            if not code:
                continue
            cmd = code.split()[0]
            words = {key: float(value) for key, value in re.findall(r'([XYZE])(-?(?:\d+(?:\.\d*)?|\.\d+))', code)}
            if cmd == 'M83':
                relative = True
            elif cmd == 'M82':
                relative = False
            elif cmd == 'G92':
                e = words.get('E', e)
            elif cmd in ('G0', 'G1', 'G2', 'G3'):
                new_x, new_y = words.get('X', x), words.get('Y', y)
                if 'E' in words:
                    amount = words['E'] if relative else words['E'] - e
                    e = e + words['E'] if relative else words['E']
                    if amount > 0 and (new_x != x or new_y != y or cmd in ('G2', 'G3')) and z is not None and role in ROLES:
                        layers[z][ROLES.index(role)] += amount
                x, y = new_x, new_y
    return dict(layers)


def compare(reference, actual, csv_path=None):
    ref, got = read(reference), read(actual)
    rows = []
    for z in sorted(ref.keys() | got.keys()):
        r, g = ref.get(z, [0, 0]), got.get(z, [0, 0])
        rows.append([z, *r, *g, (sum(g)/sum(r)-1)*100 if sum(r) else ''])
    if csv_path:
        with open(csv_path, 'w', newline='') as f:
            writer = csv.writer(f)
            writer.writerow(['z', 'reference_base_mm', 'reference_interface_mm', 'mica_base_mm', 'mica_interface_mm', 'difference_percent'])
            writer.writerows(rows)
    totals = lambda values: [sum(v[i] for v in values.values()) for i in range(2)]
    r, g = totals(ref), totals(got)
    common = ref.keys() & got.keys()
    within = sum(abs(sum(got[z])/sum(ref[z])-1) <= 0.1 for z in common)
    print(f'reference: {len(ref)} support layers, base {r[0]:.3f} mm, interface {r[1]:.3f} mm')
    print(f'mica:      {len(got)} support layers, base {g[0]:.3f} mm, interface {g[1]:.3f} mm')
    if sum(r): print(f'total difference: {(sum(g)/sum(r)-1)*100:+.2f}%')
    print(f'common Z: {len(common)}, within ±10%: {within}; missing: {len(ref.keys()-got.keys())}, extra: {len(got.keys()-ref.keys())}')


if __name__ == '__main__':
    compare(*sys.argv[1:])
