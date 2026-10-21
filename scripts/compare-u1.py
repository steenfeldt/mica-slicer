#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""U1 text-G-code diagnostics. Never launches a slicer. Usage: compare-u1.py ref.gcode mica.gcode.

Filament excludes retraction repayment; speeds are length-weighted commanded feedrates, including
arcs. Internal/external bridges stay separate. Bottom surface is combined with Internal solid infill
because Mica does not yet expose a separate bottom role. Macro execution time is not observable.
"""
import collections
import importlib.util
import json
import pathlib
import re
import sys


def module(name):
    spec = importlib.util.spec_from_file_location(name, pathlib.Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


sys.dont_write_bytecode = True
regions = module('compare-regions')
cooling = module('compare-cooling')
cooling.ALIASES.update(regions.ALIASES)
cooling.ALIASES.pop('Internal Bridge')
cooling.ALIASES['Bridge'] = 'Bridge'
cooling.ALIASES['Bridge infill'] = 'Bridge'
ORCA = {'External perimeter': 'Outer wall', 'Perimeter': 'Inner wall',
        'Internal infill': 'Sparse infill', 'Solid infill': 'Internal solid infill + Bottom surface',
        'Top solid infill': 'Top surface', 'Bridge infill': 'Bridge',
        'Overhang perimeter': 'Overhang wall', 'Gap fill': 'Gap infill', 'Skirt/Brim': 'Skirt/Brim'}


def diagnostics(path):
    data = pathlib.Path(path).read_text()
    layers, seconds = cooling.parse(path)
    amounts = regions.analyze(path)['totals']
    speeds = collections.defaultdict(lambda: [0., 0.])
    for layer in layers:
        for role, (length, weighted_speed, _) in layer.items():
            speeds[role][0] += length
            speeds[role][1] += weighted_speed
    roles = {ORCA.get(role, role): {'filament_mm': amount,
             'mean_speed_mm_s': speeds[role][1] / speeds[role][0] if speeds[role][0] else None}
             for role, amount in amounts.items()}
    fan = collections.Counter()
    motion = collections.Counter()
    relative = True
    e = 0.
    z = 0.
    layer_z = 0.
    active = False
    lifted = False
    for line in data.splitlines():
        if line == ';LAYER_CHANGE':
            active = True
        if line.startswith(';Z:'):
            layer_z = float(line[3:])
            lifted = z > layer_z + .001
        if line == ';WIPE_START':
            motion['wipe_blocks'] += 1
        fields = line.split(';', 1)[0].split()
        if not fields:
            continue
        cmd = fields[0]
        if cmd in ('M106', 'M107'):
            fan[' '.join(fields)] += 1
        if cmd == 'M83':
            relative = True
        if cmd == 'M82':
            relative = False
        values = {k: float(v) for k, v in re.findall(r'\b([XYZE])([-+]?(?:\d*\.\d+|\d+\.?))', ' '.join(fields))}
        if cmd == 'G92':
            e = values.get('E', e)
        if cmd not in ('G0', 'G1', 'G2', 'G3'):
            continue
        de = (values['E'] if relative else values['E'] - e) if 'E' in values else 0.
        e = (e + de) if relative else values.get('E', e)
        nz = values.get('Z', z)
        xy = 'X' in values or 'Y' in values
        if active:
            if de < 0:
                motion['retract_segments'] += 1
                motion['retracted_mm'] -= de
                motion['xy_retract_segments' if xy else 'e_only_retract_segments'] += 1
            if de <= 0 and nz > z + .001 and nz > layer_z + .001 and not lifted:
                motion['hop_starts'] += 1
                motion['arc_hops' if cmd in ('G2', 'G3') else 'sloped_hops' if xy else 'vertical_hops'] += 1
            lifted = nz > layer_z + .001
        z = nz
    return {'path': str(path), 'layers': len(layers), 'estimated_seconds': seconds,
            'roles': roles, 'fan_commands': dict(fan), 'motion': dict(motion)}


def compare(reference, mica):
    a, b = diagnostics(reference), diagnostics(mica)
    roles = {}
    for role in sorted(a['roles'].keys() | b['roles'].keys()):
        av, bv = a['roles'].get(role, {}), b['roles'].get(role, {})
        amount = av.get('filament_mm', 0)
        roles[role] = {'reference': av, 'mica': bv,
                       'filament_error_pct': 100 * (bv.get('filament_mm', 0) / amount - 1) if amount else None}
    return {'reference': a, 'mica': b, 'roles': roles,
            'time_error_pct': 100 * (b['estimated_seconds'] / a['estimated_seconds'] - 1)
            if a['estimated_seconds'] and b['estimated_seconds'] else None}


if __name__ == '__main__':
    print(json.dumps(compare(sys.argv[1], sys.argv[2]), indent=2))
