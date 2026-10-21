#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare deposited filament by physical tool/role and external-wall seam starts.

Usage: compare-painting.py reference.gcode mica.gcode [--seams]
Supports relative/absolute E and subtracts retraction repayment. Seam comparison
is intended for the single-wall-boundary cylinder fixture, one seam per layer.
"""
import collections
import json
import math
import re
import sys

ALIASES = {'Outer wall': 'External perimeter', 'Inner wall': 'Perimeter',
           'Sparse infill': 'Internal infill', 'Internal solid infill': 'Solid infill',
           'Bottom surface': 'Solid infill', 'Top surface': 'Top solid infill',
           'Gap infill': 'Gap fill', 'Overhang wall': 'Overhang perimeter',
           'Support': 'Support material', 'Support interface': 'Support material interface',
           'Bridge': 'Bridge infill', 'Internal Bridge': 'Internal bridge infill'}


def read(path):
    amount = collections.defaultdict(collections.Counter)
    debt = collections.defaultdict(float)
    absolute = collections.defaultdict(float)
    seams, tools = {}, []
    tool, layer, x, y, relative, role, pending = 0, -1, 0., 0., True, 'Custom', False
    with open(path, encoding='utf-8') as source:
        for text in source:
            if text.strip() in [';LAYER_CHANGE', '; CHANGE_LAYER']:
                layer += 1
                tools.append([])
            if text.startswith(';TYPE:') or text.startswith('; FEATURE: '):
                role = text.split(':', 1)[1].strip()
                role = ALIASES.get(role, role)
                pending = role == 'External perimeter'
            code = text.split(';')[0].strip()
            if not code:
                continue
            cmd = code.split()[0]
            values = {k: float(v) for k, v in re.findall(r'\b([XYE])([-+]?\d*\.?\d+)', code)}
            if re.fullmatch(r'T\d+', cmd):
                tool = int(cmd[1:])
            elif cmd in ['M82', 'M83']:
                relative = cmd == 'M83'
            elif cmd == 'G92' and 'E' in values:
                absolute[tool] = values['E']
            elif cmd in ['G0', 'G1', 'G2', 'G3']:
                extrusion = 0.
                if 'E' in values:
                    delta = values['E'] if relative else values['E'] - absolute[tool]
                    absolute[tool] = values['E']
                    if delta < 0:
                        debt[tool] -= delta
                    else:
                        paid = min(delta, debt[tool])
                        debt[tool] -= paid
                        extrusion = delta - paid
                if layer >= 0 and extrusion > 0 and ('X' in values or 'Y' in values) and role != 'Custom':
                    amount[tool][role] += extrusion
                    if not tools[-1] or tools[-1][-1] != tool:
                        tools[-1].append(tool)
                    if pending:
                        seams.setdefault(layer, (x, y))
                        pending = False
                x, y = values.get('X', x), values.get('Y', y)
    return amount, seams, tools


def main():
    ref, ref_seams, ref_tools = read(sys.argv[1])
    mica, mica_seams, mica_tools = read(sys.argv[2])
    rows = []
    for tool in sorted(set(ref) | set(mica)):
        for role in sorted(set(ref[tool]) | set(mica[tool])):
            a, b = ref[tool][role], mica[tool][role]
            rows.append(dict(tool=tool, role=role, reference_mm=a, mica_mm=b,
                             difference_percent=100 * (b / a - 1) if a else None,
                             within_3_percent=abs(b-a) <= 0.03*a + 0.0001))
    out = dict(roles=rows, all_roles_within_3_percent=all(r['within_3_percent'] for r in rows),
               equal_layer_tool_sequences=ref_tools == mica_tools,
               reference_tool_mm={t: sum(r.values()) for t, r in ref.items()},
               mica_tool_mm={t: sum(r.values()) for t, r in mica.items()})
    if '--seams' in sys.argv:
        pairs = sorted(set(ref_seams) & set(mica_seams))
        out['seams'] = dict(reference_count=len(ref_seams), mica_count=len(mica_seams),
                            max_distance_mm=max((math.dist(ref_seams[i], mica_seams[i]) for i in pairs), default=None),
                            positions=[dict(layer=i, reference=ref_seams[i], mica=mica_seams[i]) for i in pairs])
    print(json.dumps(out, indent=2))


if __name__ == '__main__':
    main()
