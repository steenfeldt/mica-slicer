#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare supports printed with other tools in two text G-code files (reference, Mica).

Usage: python3 scripts/compare-support-tools.py reference.gcode mica.gcode [--json]
Reports per file: extruding-tool sequences per layer, T commands, filament (mm) per tool and
role (support base, support interface, tower, model), support/interface layer counts per tool,
support-only layers (not synchronized with object layers), interface Z values, temperature and
fan commands at tool changes, and the estimated time. Standard library only. Handles absolute
and relative E, G92 and arcs; retraction/deretraction (no XY move) is excluded.
"""
import collections
import json
import re
import sys

BASE = {'Support material', 'Support'}
INTERFACE = {'Support material interface', 'Support interface', 'Support transition'}
TOWER = {'Wipe tower', 'Prime tower'}
OTHER = {'Custom', '', 'Skirt', 'Skirt/Brim', 'Brim'}


def kind(role):
    if role in BASE:
        return 'base'
    if role in INTERFACE:
        return 'interface'
    if role in TOWER:
        return 'tower'
    if role in OTHER:
        return 'other'
    return 'model'


def analyze(path):
    tool, role, z = 0, '', None
    relative, e_abs = True, 0.
    x = y = 0.
    layers, layer_z = [], []
    commands, startup_commands = [], 0
    filament = collections.defaultdict(lambda: collections.defaultdict(float))
    support_z = collections.defaultdict(set)
    interface_z = collections.defaultdict(set)
    model_z = set()
    temps_at_change, fans = [], collections.Counter()
    pending_change = None
    time = None
    for raw in open(path, encoding='utf-8', errors='replace'):
        line = raw.strip()
        if line == ';LAYER_CHANGE':
            layers.append([])
            layer_z.append(None)
        elif line.startswith(';Z:'):
            z = round(float(line[3:]), 3)
            if layer_z:
                layer_z[-1] = z
        elif line.startswith(';TYPE:'):
            role = line[6:]
        elif re.match(r'; (estimated printing time \(normal mode\)|model printing time|total estimated time)', line) and time is None:
            time = line.split('=', 1)[1].strip() if '=' in line else line.split(':', 1)[1].strip()
        code = line.split(';', 1)[0].strip()
        if not code:
            continue
        words = code.split()
        cmd = words[0]
        if re.fullmatch(r'T\d+', cmd):
            tool = int(cmd[1:])
            commands.append(tool)
            if not layers:
                startup_commands += 1
            else:
                pending_change = tool
            continue
        if cmd in ('M104', 'M109', 'M104.1', 'M109.1') and layers:
            s = re.search(r'\bS(\d+(?:\.\d*)?)', code)
            t = re.search(r'\bT(\d+)', code)
            if s:
                temps_at_change.append(f'{cmd} T{t[1] if t else tool} S{float(s[1]):g}')
        if cmd == 'M106' and layers:
            s = re.search(r'\bS(\d+(?:\.\d*)?)', code)
            p = re.search(r'\bP(\d+)', code)
            fans[f'T{tool}{" P"+p[1] if p else ""} S{round(float(s[1])) if s else 255}'] += 1
        if cmd == 'M107' and layers:
            fans[f'T{tool} off'] += 1
        if cmd == 'M83':
            relative = True
        elif cmd == 'M82':
            relative = False
        elif cmd == 'G92':
            m = re.search(r'\bE(-?\d*\.?\d+)', code)
            if m:
                e_abs = float(m[1])
        if cmd not in ('G0', 'G1', 'G2', 'G3'):
            continue
        params = {k: float(v) for k, v in re.findall(r'([XYE])(-?\d*\.?\d+)', code)}
        nx, ny = params.get('X', x), params.get('Y', y)
        moved = nx != x or ny != y or cmd in ('G2', 'G3')
        x, y = nx, ny
        if 'E' not in params:
            continue
        e = params['E'] if relative else params['E'] - e_abs
        if not relative:
            e_abs = params['E']
        if e <= 0 or not moved or not layers:
            continue
        k = kind(role)
        filament[tool][k] += e
        if not layers[-1] or layers[-1][-1] != tool:
            layers[-1].append(tool)
        if z is not None:
            if k == 'base':
                support_z[tool].add(z)
            elif k == 'interface':
                interface_z[tool].add(z)
            elif k == 'model':
                model_z.add(z)
    all_support = set().union(*support_z.values(), *interface_z.values()) if (support_z or interface_z) else set()
    return {
        'layers': len(layers),
        'layer_tools': layers,
        'T_commands': len(commands),
        'T_after_start': len(commands) - startup_commands,
        'filament_mm': {f'T{t}': {k: round(v, 2) for k, v in sorted(r.items())} for t, r in sorted(filament.items())},
        'support_layers': {f'T{t}': len(v) for t, v in sorted(support_z.items())},
        'interface_layers': {f'T{t}': len(v) for t, v in sorted(interface_z.items())},
        'interface_z': {f'T{t}': sorted(v) for t, v in sorted(interface_z.items())},
        'support_only_layers': len(all_support - model_z),
        'temperatures': dict(collections.Counter(temps_at_change)),
        'fans': dict(sorted(fans.items())),
        'time': time,
    }


def summary(a):
    seq = collections.Counter(tuple(t) for t in a['layer_tools'])
    return {k: v for k, v in a.items() if k not in ('layer_tools', 'interface_z')} | {
        'layer_sequences': {'/'.join(map(str, k)) or '-': v for k, v in sorted(seq.items())}}


if __name__ == '__main__':
    ref, mica = analyze(sys.argv[1]), analyze(sys.argv[2])
    out = {'equal_layer_tool_sequences': ref['layer_tools'] == mica['layer_tools'],
           'first_sequence_difference': next((i for i, (a, b) in enumerate(zip(ref['layer_tools'], mica['layer_tools'])) if a != b), None),
           'interface_z_equal': ref['interface_z'] == mica['interface_z'],
           'reference': summary(ref), 'mica': summary(mica)}
    print(json.dumps(out, indent=1))
