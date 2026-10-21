#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare per-layer role feedrates, fans, travel length and exterior crossings (text G-code).

Crossings count travel runs intersecting the same layer's emitted external perimeter strokes,
excluding endpoint contacts. This measures XY crossings even if Z-hop clears the wall.
A 2 mm spatial grid avoids quadratic comparisons. Use --csv for every role/layer sample.
"""
import argparse
import collections
import csv
import json
import math
import re
import statistics

ROLES = ('External perimeter', 'Perimeter', 'Overhang perimeter')
NUMBER = re.compile(r'([XYZEFSIJR])([-+]?\d*\.?\d+(?:[eE][-+]?\d+)?)')


def cells(a, b):
    # Exact grid traversal is unnecessary here: the segment's small bounding box is conservative.
    for x in range(math.floor(min(a[0], b[0])/2), math.floor(max(a[0], b[0])/2)+1):
        for y in range(math.floor(min(a[1], b[1])/2), math.floor(max(a[1], b[1])/2)+1):
            yield x, y


def crosses(a, b, p, q):
    v = b[0]-a[0], b[1]-a[1]
    w = q[0]-p[0], q[1]-p[1]
    d = v[0]*w[1]-v[1]*w[0]
    if abs(d) < 1e-10:
        return False
    u = p[0]-a[0], p[1]-a[1]
    t = (u[0]*w[1]-u[1]*w[0])/d
    s = (u[0]*v[1]-u[1]*v[0])/d
    return 1e-6 < t < 1-1e-6 and -1e-8 <= s <= 1+1e-8


def parse(path):
    layers = []
    layer = None
    x = y = None
    e = 0.
    relative = False
    role = ''
    feed = fan = 0.
    active_run = None
    estimate = None
    for raw in open(path):
        if raw.startswith('; estimated printing time (normal mode) = '):
            value = raw.split(' = ')[1]
            estimate = sum(float(n)*dict(d=86400, h=3600, m=60, s=1)[u]
                           for n, u in re.findall(r'(\d+)([dhms])', value))
        if raw.startswith(';LAYER_CHANGE'):
            layer = dict(z=0., roles=collections.defaultdict(list), fans=[],
                         fan_commands=[], travel=[], exterior=[])
            layers.append(layer)
            active_run = None
        elif raw.startswith(';Z:') and layer is not None:
            layer['z'] = float(raw[3:])
        elif raw.startswith(';TYPE:'):
            role = raw[6:].strip()
        code = raw.split(';')[0].strip()
        if not code:
            continue
        cmd = code.split()[0]
        args = {k: float(v) for k, v in NUMBER.findall(code[len(cmd):])}
        if cmd == 'M83':
            relative = True
        elif cmd == 'M82':
            relative = False
        elif cmd in ('M106', 'M107'):
            fan = args.get('S', 255.)/2.55 if cmd == 'M106' else 0.
            if layer is not None and role != 'Custom':
                layer['fan_commands'].append(round(fan, 4))
        elif cmd == 'G92':
            e = args.get('E', e)
        elif cmd in ('G0', 'G1', 'G2', 'G3'):
            nx, ny = args.get('X', x), args.get('Y', y)
            de = (args['E'] if relative else args['E']-e) if 'E' in args else 0.
            e += de
            feed = args.get('F', feed)
            if x is not None and y is not None and layer is not None and role != 'Custom':
                a, b = (x, y), (nx, ny)
                length = math.dist(a, b)
                segments = [(a, b)]
                if cmd in ('G2', 'G3'):
                    if 'R' in args:
                        radius = abs(args['R'])
                        h = math.sqrt(max(0., radius*radius-length*length/4))
                        sign = (1 if cmd == 'G3' else -1)*(1 if args['R'] >= 0 else -1)
                        cx = (x+nx)/2-sign*(ny-y)/max(length, 1e-12)*h
                        cy = (y+ny)/2+sign*(nx-x)/max(length, 1e-12)*h
                    else:
                        cx, cy = x+args.get('I', 0.), y+args.get('J', 0.)
                        radius = math.hypot(x-cx, y-cy)
                    begin, end = math.atan2(y-cy, x-cx), math.atan2(ny-cy, nx-cx)
                    sweep = ((end-begin) if cmd == 'G3' else (begin-end)) % math.tau
                    if sweep < 1e-9:
                        sweep = math.tau
                    length = radius*sweep
                    count = max(1, math.ceil(length/.1))
                    points = [a]+[(cx+radius*math.cos(begin+(1 if cmd == 'G3' else -1)*sweep*i/count),
                                   cy+radius*math.sin(begin+(1 if cmd == 'G3' else -1)*sweep*i/count))
                                  for i in range(1, count)]+[b]
                    segments = list(zip(points, points[1:]))
                if length > 1e-6:
                    if de > 1e-9:
                        layer['roles'][role].append((feed/60., length))
                        layer['fans'].append(fan)
                        if role == 'External perimeter':
                            layer['exterior'].extend(segments)
                        active_run = None
                    else:
                        if active_run is None:
                            active_run = []
                            layer['travel'].append(active_run)
                        active_run.extend(segments)
            x, y = nx, ny
    for layer in layers:
        grid = collections.defaultdict(set)
        for i, (a, b) in enumerate(layer['exterior']):
            for cell in cells(a, b):
                grid[cell].add(i)
        def crossing(run):
            for a, b in run:
                candidates = set().union(*(grid[cell] for cell in cells(a, b)))
                if any(crosses(a, b, *layer['exterior'][i]) for i in candidates):
                    return True
            return False
        layer['crossings'] = sum(crossing(run) for run in layer['travel'])
        layer['travel_length'] = sum(math.dist(a, b) for run in layer['travel'] for a, b in run)
    return layers, estimate


def weighted_median(values):
    total = sum(v[1] for v in values)
    for speed, length in sorted(values):
        total -= 2*length
        if total <= 1e-8:
            return speed


def compare(reference, mica):
    a, at = parse(reference)
    b, bt = parse(mica)
    if len(a) != len(b):
        raise ValueError(f'Layer counts differ: {len(a)} / {len(b)}')
    rows = []
    summary = dict(reference=str(reference), mica=str(mica), layers=len(a),
                   time_seconds=[at, bt], time_error_pct=100*(bt/at-1) if at and bt else None)
    for i, (x, y) in enumerate(zip(a, b), 1):
        if abs(x['z']-y['z']) > 1e-4:
            raise ValueError(f'Layer {i}: Z differs')
        for role in ROLES:
            ref, actual = x['roles'][role], y['roles'][role]
            av = statistics.median(v[0] for v in ref) if ref else None
            bv = statistics.median(v[0] for v in actual) if actual else None
            rows.append(dict(layer=i, z=x['z'], role=role, ref_median=av, mica_median=bv,
                             error_pct=100*(bv/av-1) if av and bv else None,
                             ref_weighted=weighted_median(ref), mica_weighted=weighted_median(actual)))
    summary['speeds'] = {}
    for role in ROLES:
        valid = [r for r in rows if r['role'] == role and r['error_pct'] is not None]
        missing = [r['layer'] for r in rows if r['role'] == role and (r['ref_median'] is None) != (r['mica_median'] is None)]
        summary['speeds'][role] = dict(compared=len(valid), outside_5pct=sum(abs(r['error_pct']) > 5 for r in valid),
            max_error_pct=max((abs(r['error_pct']) for r in valid), default=0), missing_role_layers=missing)
    for metric in ['travel_length', 'crossings']:
        av, bv = sum(x[metric] for x in a), sum(x[metric] for x in b)
        summary[metric] = dict(reference=av, mica=bv, error_pct=100*(bv/av-1) if av else None)
    summary['fan_commands'] = [sum(len(x['fan_commands']) for x in layers) for layers in (a, b)]
    summary['fan_range_mismatched_layers'] = [i for i, (x, y) in enumerate(zip(a, b), 1) if
        x['fans'] and y['fans'] and (abs(min(x['fans'])-min(y['fans'])) > 1 or abs(max(x['fans'])-max(y['fans'])) > 1)]
    return summary, rows


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('reference'); p.add_argument('mica'); p.add_argument('--csv'); p.add_argument('--json')
    args = p.parse_args()
    summary, rows = compare(args.reference, args.mica)
    result = json.dumps(summary, indent=2)
    print(result)
    if args.json:
        open(args.json, 'w').write(result+'\n')
    if args.csv:
        with open(args.csv, 'w', newline='') as f:
            writer = csv.DictWriter(f, fieldnames=rows[0]); writer.writeheader(); writer.writerows(rows)


if __name__ == '__main__':
    main()
