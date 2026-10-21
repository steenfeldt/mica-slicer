#!/usr/bin/env python3
"""Compare text G-code tool ordering, temperatures, and model filament by tool and role.
SPDX-License-Identifier: AGPL-3.0-only
Provenance: original
Usage: python3 scripts/compare-multitool.py reference.gcode mica.gcode
No third-party modules; startup priming, pure-E retractions and firmware macros are excluded
from model filament. Feed both files with relative E (the supported Mica multi-tool mode).
"""
import collections
import json
import re
import sys

ALIASES = {'Outer wall': 'External perimeter', 'Inner wall': 'Perimeter',
           'Sparse infill': 'Internal infill', 'Internal solid infill': 'Solid infill',
           'Top surface': 'Top solid infill', 'Gap infill': 'Gap fill',
           'Overhang wall': 'Overhang perimeter', 'Support': 'Support material',
           'Support interface': 'Support material interface'}

def analyze(path):
    layers = []
    tool, role = 0, 'Custom'
    filament = collections.defaultdict(lambda: collections.defaultdict(float))
    commands = []
    startup_tools = []
    temperatures = collections.Counter()
    with open(path, encoding='utf-8') as source:
        for line in source:
            line = line.strip()
            if line == ';LAYER_CHANGE':
                layers.append([])
            if line.startswith(';TYPE:'):
                role = ALIASES.get(line[6:], line[6:])
            code = line.split(';')[0].strip()
            cmd = code.split(' ')[0]
            if re.fullmatch(r'T\d+', cmd):
                tool = int(cmd[1:])
                commands.append(tool)
                if not layers:
                    startup_tools.append(tool)
            if cmd in ['M104', 'M109', 'M104.1']:
                s = re.search(r'\bS([\d.]+)', code)
                t = re.search(r'\bT(\d+)', code)
                if s:
                    temperatures[(cmd, int(t[1]) if t else tool, float(s[1]))] += 1
            if layers and role != 'Custom' and cmd in ['G0', 'G1', 'G2', 'G3']:
                e = re.search(r'\bE([\d.]+)', code)
                if e and re.search(r'\b[XY][-\d.]', code):
                    if not layers[-1] or layers[-1][-1] != tool:
                        layers[-1].append(tool)
                    filament[tool][role] += float(e[1])
    return {'layers': layers, 'T_commands': len(commands), 'startup_tools': startup_tools,
            'filament_mm': {t: {r: round(v, 3) for r, v in roles.items()} for t, roles in filament.items()},
            'temperatures': {f'{cmd} T{t} S{s:g}': n for (cmd, t, s), n in sorted(temperatures.items())}}

if __name__ == '__main__':
    ref, mica = [analyze(p) for p in sys.argv[1:3]]
    print(json.dumps({'equal_layer_tool_sequences': ref['layers'] == mica['layers'],
                      'reference': ref, 'mica': mica}, indent=2))
