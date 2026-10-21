#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare relative-E tower paths, layer sequences, bounds, per-tool filament and timing.
Usage: compare-wipe-tower.py reference.gcode mica.gcode. Standard library only.
Positive XY extrusion excludes deretraction and opaque macros. Final unloading is reported
with its actual Z, separately from the final object layer.
"""
import collections
import json
import math
import re
import sys


def analyze(path):
    tool, role, z, x, y, layer = 0, 'Custom', 0., 0., 0., -1
    tower = collections.defaultdict(float)
    per_tool = collections.defaultdict(float)
    all_tools = collections.defaultdict(float)
    total = collections.defaultdict(float)
    retracted = collections.defaultdict(float)
    relative_e, relative_xyz, e_position = False, False, 0.
    bounds = [math.inf, math.inf, -math.inf, -math.inf]
    layer_bounds = collections.defaultdict(lambda: [math.inf, math.inf, -math.inf, -math.inf])
    sequences, commands = [], []
    time = None
    for line in open(path):
        line = line.strip()
        if line == ';LAYER_CHANGE':
            layer += 1
            sequences.append([])
        if line.startswith(';TYPE:'):
            role = line[6:]
        if line.startswith('; estimated printing time (normal mode) ='):
            time = line.split('=', 1)[1].strip()
        code = line.split(';', 1)[0].split()
        if not code:
            continue
        if re.fullmatch(r'T\d+', code[0]):
            tool = int(code[0][1:])
            commands.append(tool)
        if code[0] in ['M82', 'M83']:
            relative_e = code[0] == 'M83'
        if code[0] in ['G90', 'G91']:
            relative_xyz = code[0] == 'G91'
        params = {k: float(v) for k, v in re.findall(r'([XYZE])([-+]?\d*\.?\d+)', ' '.join(code[1:]))}
        if code[0] == 'G92' and 'E' in params:
            e_position = params['E']
        if code[0] not in ['G0', 'G1', 'G2', 'G3']:
            continue
        old = (x, y)
        x, y, z = [params.get(k, 0.) + p if relative_xyz else params.get(k, p)
                   for k, p in zip('XYZ', (x, y, z))]
        e = params.get('E', 0.) if relative_e or relative_xyz else params.get('E', e_position) - e_position
        e_position += e
        retracted[tool] -= e
        if retracted[tool] < 0:
            total[tool] -= retracted[tool]
            retracted[tool] = 0.
        if layer >= 0 and e > 0 and ('X' in params or 'Y' in params):
            all_tools[tool] += e
            if not sequences[-1] or sequences[-1][-1] != tool:
                sequences[-1].append(tool)
            if role in ['Wipe tower', 'Prime tower']:
                key = round(z, 4)
                tower[key] += e
                per_tool[tool] += e
                for b in [bounds, layer_bounds[key]]:
                    b[0] = min(b[0], old[0], x)
                    b[1] = min(b[1], old[1], y)
                    b[2] = max(b[2], old[0], x)
                    b[3] = max(b[3], old[1], y)
    return dict(tower_mm=dict(tower), tower_per_tool_mm=dict(per_tool), total_per_tool_xy_mm=dict(all_tools),
                total_per_tool_explicit_mm=dict(total),
                bounds=bounds, layer_bounds=dict(layer_bounds), tool_sequences=sequences,
                T_commands=commands, print_time=time)


if __name__ == '__main__':
    ref, mica = [analyze(p) for p in sys.argv[1:3]]
    diffs = {z: 100 * (mica['tower_mm'].get(z, 0) / e - 1) for z, e in ref['tower_mm'].items() if e}
    summary = dict(equal_tool_sequences=ref['tool_sequences'] == mica['tool_sequences'],
                   equal_T_commands=ref['T_commands'] == mica['T_commands'],
                   worst_layer_percent=max(map(abs, diffs.values()), default=0),
                   layers_outside_3_percent={z: d for z, d in diffs.items() if abs(d) > 3},
                   extra_tower_z=sorted(set(mica['tower_mm']) - set(ref['tower_mm'])))
    print(json.dumps(dict(summary=summary, reference=ref, mica=mica), indent=2))
