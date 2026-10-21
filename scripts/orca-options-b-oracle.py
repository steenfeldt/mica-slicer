#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: adapted
# Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.hpp :: arc_angle
# Changes: Original oracle orchestration and fixtures; arc winding integrated into per-loop diagnostics.
"""Reproduce batch B oracles offline; all models, presets, logs and G-code stay in temp.

Stops launching Orca after the first failed attempt. Four explicit filament slots are mandatory.
Use --output to retain and inspect the commands, validation reports and per-role comparisons.
"""
import argparse
import importlib.util
import json
import math
import re
from pathlib import Path
import shlex
import struct
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parents[1]
U1 = ['--vendor', 'Snapmaker', '--printer', 'Snapmaker U1 (0.4 nozzle)', '--print',
      '0.20 Standard @Snapmaker U1 (0.4 nozzle)', '--filament', 'Snapmaker PLA @U1', '--bed', 'High Temp Plate']
ALIASES = {'External perimeter': 'Outer wall', 'Perimeter': 'Inner wall', 'Internal infill': 'Sparse infill',
           'Solid infill': 'Internal solid infill', 'Bottom surface': 'Internal solid infill',
           'Top solid infill': 'Top surface', 'Bridge infill': 'Bridge', 'Internal bridge infill': 'Internal Bridge',
           'Overhang perimeter': 'Overhang wall', 'Gap fill': 'Gap infill', 'Skirt': 'Skirt/Brim'}


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def cylinder(path):
    triangles = []
    n = 128
    for i in range(n):
        a, b = [2 * math.pi * j / n for j in (i, i + 1)]
        p, q = (15*math.cos(a), 15*math.sin(a)), (15*math.cos(b), 15*math.sin(b))
        lo, hi, lo2, hi2 = (*p, 0), (*p, 12), (*q, 0), (*q, 12)
        triangles.extend([(lo, lo2, hi2), (lo, hi2, hi), ((0, 0, 0), lo2, lo), ((0, 0, 12), hi, hi2)])
    with path.open('wb') as f:
        f.write(bytes(80)); f.write(struct.pack('<I', len(triangles)))
        for tri in triangles:
            f.write(struct.pack('<12fH', 0, 0, 0, *tri[0], *tri[1], *tri[2], 0))


def roles(data):
    out = {}
    for role, amount in data['roles'].items():
        key = ALIASES.get(role, role)
        if key not in ('Custom', 'custom', 'Travel', 'Mixed'):
            out[key] = out.get(key, 0.) + amount
    return out


def structure(path):
    """Retain per-layer wall role runs, signed winding and ramp Z/feedrate diagnostics."""
    layers = []; layer = None; role = ''; xy = [0., 0.]; z = 0.; feed = 0.; run = None
    def finish():
        nonlocal run
        if run and layer is not None:
            layer['walls'].append({'role': run['role'], 'area2': round(run['area'] + run['last'][0]*run['first'][1] - run['first'][0]*run['last'][1], 3)})
        run = None
    for line in path.read_text().splitlines():
        if line.startswith(';LAYER_CHANGE'):
            finish(); layer = {'walls': [], 'ramps': [], 'bridge_roles': []}; layers.append(layer)
        if line.startswith(';TYPE:'):
            next_role = ALIASES.get(line[6:].strip(), line[6:].strip())
            wall_roles = ('Outer wall', 'Inner wall', 'Overhang wall')
            if run is not None and next_role in wall_roles and (next_role == 'Overhang wall' or run['role'] in (next_role, 'Overhang wall')):
                if next_role != 'Overhang wall': run['role'] = next_role
            else: finish()
            role = next_role
            if layer is not None and role in ('Bridge', 'Internal Bridge'): layer['bridge_roles'].append(role)
        if not line.startswith(('G0 ', 'G1 ', 'G2 ', 'G3 ')): continue
        values = {}
        for w in line.split(';')[0].split()[1:]:
            if w[0] in 'XYZEFIJR':
                try: values[w[0]] = float(w[1:])
                except ValueError: pass
        target = [values.get('X', xy[0]), values.get('Y', xy[1])]
        nz = values.get('Z', z); feed = values.get('F', feed)
        extruding = values.get('E', 0) > 0 and target != xy
        if extruding and layer is not None:
            if role in ('Outer wall', 'Inner wall', 'Overhang wall'):

                if run is None: run = {'role': role, 'area': 0., 'first': xy, 'last': target}
                run['area'] += xy[0] * target[1] - target[0] * xy[1]
                if line.startswith(('G2 ', 'G3 ')):
                    sign = 1 if line.startswith('G3 ') else -1
                    if 'R' in values:
                        radius = abs(values['R'])
                        sweep = 2*math.asin(min(1., math.dist(xy,target)/(2*radius))) if radius else 0.
                        if values['R'] < 0: sweep = math.tau-sweep
                    else:
                        cx, cy = xy[0]+values.get('I',0.), xy[1]+values.get('J',0.)
                        radius = math.hypot(xy[0]-cx,xy[1]-cy)
                        start = math.atan2(xy[1]-cy,xy[0]-cx)
                        end = math.atan2(target[1]-cy,target[0]-cx)
                        sweep = (sign*(end-start)) % math.tau
                        if sweep < 1e-9: sweep = math.tau
                    run['area'] += sign*radius*radius*(sweep-math.sin(sweep))
                run['last'] = target
            if abs(nz-z) > 1e-6: layer['ramps'].append({'z': nz, 'speed': feed/60, 'xy_length': math.dist(xy, target)})
        elif 'X' in values or 'Y' in values: finish()
        xy, z = target, nz
    finish()
    return layers


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/mica')
    parser.add_argument('--orca', type=Path, default=Path('/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer'))
    parser.add_argument('--case', action='append')
    parser.add_argument('--reuse-references', action='store_true', help='Reuse successful reference G-code already present in --output')
    parser.add_argument('--allow-rotations', action='store_true',
                        help="Let Orca's CLI arrange rotate the model (its default). Mica keeps the STL "
                             'orientation, so by default the oracle passes --allow-rotations=0 and slices '
                             'Mica at the centre Orca chose')
    args = parser.parse_args()
    output = (args.output or Path(tempfile.mkdtemp(prefix='mica-orcaopts-b-'))).resolve()
    output.mkdir(parents=True, exist_ok=True)
    sys.path.insert(0, str(ROOT / 'tests/models'))
    import bridges
    import overhang_staircase
    bridges.main(output); overhang_staircase.generate(output / 'staircase.stl'); cylinder(output / 'cylinder.stl')
    prepare = load('prepare_u1', ROOT / 'scripts/prepare-u1-oracle.py')
    wall = {'wall_sequence': 'inner-outer-inner wall', 'wall_loops': '3'}
    cases = []
    for generator in ('classic', 'arachne'):
        for name, model in [('cube', ROOT/'tests/models/cube20.stl'), ('benchy', ROOT/'tests/models/3DBenchy.stl')]:
            cases.append((f'wall-{name}-{generator}', model, {**wall, 'wall_generator': generator}))
        cases.append((f'reverse-{generator}', output/'staircase.stl', {'wall_generator': generator, 'overhang_reverse': '1', 'overhang_reverse_threshold': '50%'}))
    for mode, name in [('external_bridge_only', 'pillars_0'), ('external_bridge_only', 'cavity'),
                       ('internal_bridge_only', 'internal_roof'), ('apply_to_all', 'internal_roof')]:
        cases.append((f'bridge-{mode}-{name}', output/f'{name}.stl', {'enable_extra_bridge_layer': mode,
                      'internal_bridge_angle': '30', 'bridge_density': '80%', 'internal_bridge_density': '85%'}))
    for conditional in ('0', '1'):
        cases.append(('scarf-'+conditional, output/'cylinder.stl', {'seam_slope_type': 'external', 'seam_slope_conditional': conditional,
                      'scarf_angle_threshold': '155', 'scarf_overhang_threshold': '40%', 'seam_slope_min_length': '10',
                      'seam_slope_steps': '10', 'scarf_joint_speed': '50%', 'scarf_joint_flow_ratio': '0.9', 'seam_slope_start_height': '10%'}))
    summary = {}
    for name, model, changes in cases:
        if args.case and name not in args.case: continue
        directory = output / name
        prepare.prepare(Path('/Applications/OrcaSlicer.app/Contents/Resources/profiles'), directory)
        process = directory/'process.json'; data=json.loads(process.read_text());data.update(changes);process.write_text(json.dumps(data, indent=2)+'\n')
        filament = ';'.join([str(directory/'filament.json')]*4)
        command = [str(args.orca.resolve()), '--datadir', str(directory/'data'), '--load-settings', f'{directory}/machine.json;{process}',
                   '--load-filaments', filament, '--slice', '0', '--arrange', '1', *([] if args.allow_rotations else ['--allow-rotations=0']), '--outputdir', str(directory), '--export-settings', str(directory/'resolved.json'), str(model)]
        mica = [str(args.binary.resolve()), 'slice', str(model), *U1]
        mica_changes = {('perimeters' if k=='wall_loops' else 'perimeter_generator' if k=='wall_generator' else k):v for k,v in changes.items()}
        for k,v in mica_changes.items(): mica.extend(['--set', k+'='+v])
        mica.extend(['-o', str(directory/'mica.gcode')])
        (directory/'commands.sh').write_text('cd '+shlex.quote(str(directory))+'\n'+shlex.join(command)+'\n'+shlex.join(mica)+'\n')
        print(name, flush=True)
        ref = directory/'plate_1.gcode'
        if not (args.reuse_references and ref.exists()):
            ref.unlink(missing_ok=True)
            with (directory/'orca.log').open('w') as log:
                result = subprocess.run(command, cwd=directory, stdout=log, stderr=subprocess.STDOUT, timeout=600)
        else:
            result = subprocess.CompletedProcess(command, 0)
        if result.returncode or not ref.exists():
            summary[name] = {'blocked': 'Orca failed; no more launches', 'exit': result.returncode}
            break
        if 'OrcaSlicer 2.4.1' not in ref.read_text()[:1024]:
            summary[name] = {'blocked': 'Reference is not OrcaSlicer 2.4.1; no more launches'}
            break
        # Place Mica's model where Orca placed it (EXCLUDE_OBJECT_DEFINE CENTER).
        center = re.search(r'EXCLUDE_OBJECT_DEFINE NAME=\S+ CENTER=([-\d.]+),([-\d.]+)', ref.read_text())
        if center:
            mica[mica.index('-o'):mica.index('-o')] = ['--center', f'{center[1]},{center[2]}']
            (directory/'commands.sh').write_text('cd '+shlex.quote(str(directory))+'\n'+shlex.join(command)+'\n'+shlex.join(mica)+'\n')
        result = subprocess.run(mica, cwd=ROOT, capture_output=True, text=True, timeout=600)
        (directory/'mica.log').write_text(result.stdout+result.stderr)
        if result.returncode: raise RuntimeError(result.stderr)
        reports = []
        for file in (ref, directory/'mica.gcode'):
            result = subprocess.run([str(args.binary.resolve()), 'validate', str(file), *U1, '--json'], cwd=ROOT, capture_output=True, text=True, timeout=600)
            report=json.loads(result.stdout); reports.append(report);file.with_suffix('.validation.json').write_text(json.dumps(report,indent=2)+'\n')
        a,b=map(roles,reports)
        delta={k: {'orca':a.get(k,0), 'mica':b.get(k,0), 'delta_percent':100*(b.get(k,0)/a[k]-1) if a.get(k) else None} for k in sorted(a.keys()|b.keys())}
        structural={'orca':structure(ref),'mica':structure(directory/'mica.gcode')}
        (directory/'structure.json').write_text(json.dumps(structural,indent=2)+'\n')
        a_layers, b_layers = structural['orca'], structural['mica']
        properties = {}
        if name.startswith('wall-'):
            sequences = lambda layers: [[w['role'] for w in layer['walls']] for layer in layers]
            aa, bb = sequences(a_layers), sequences(b_layers)
            properties['exact_wall_role_sequences'] = sum(a == b for a, b in zip(aa, bb))
            properties['layers'] = [len(aa), len(bb)]
            if 'cube' in name:
                shared = [(i, a, b) for i, (a, b) in enumerate(zip(aa, bb)) if len(a) == len(b) == 3]
                properties['three_wall_layers'] = len(shared)
                properties['three_wall_sequence_matches'] = all(a == b == (['Inner wall', 'Inner wall', 'Outer wall'] if i == 0 else ['Inner wall', 'Outer wall', 'Inner wall']) for i, a, b in shared)
        elif name.startswith('reverse-'):
            reversed_layers = lambda layers: [i for i, layer in enumerate(layers) if any(w['area2'] < -0.01 for w in layer['walls'])]
            properties['reversed_layers'] = [reversed_layers(a_layers), reversed_layers(b_layers)]
            properties['direction_matches'] = properties['reversed_layers'][0] == properties['reversed_layers'][1]
        elif name.startswith('bridge-'):
            bridge_layers = lambda layers: [(i, sorted(set(layer['bridge_roles']))) for i, layer in enumerate(layers) if layer['bridge_roles']]
            properties['bridge_layers'] = [bridge_layers(a_layers), bridge_layers(b_layers)]
            properties['extra_bridge_layers_match'] = properties['bridge_layers'][0] == properties['bridge_layers'][1]
        else:
            properties['second_layer_start_ramp'] = [{'segments': len(layers[1]['ramps']), 'xy_length': sum(p['xy_length'] for p in layers[1]['ramps']), 'speeds': sorted(set(p['speed'] for p in layers[1]['ramps']))} for layers in [a_layers, b_layers]]

        if name.startswith('wall-'):
            structural_gate = properties['exact_wall_role_sequences'] == properties['layers'][0] == properties['layers'][1]
        elif name.startswith('reverse-'):
            structural_gate = properties['direction_matches'] and bool(properties['reversed_layers'][0])
        elif name.startswith('bridge-'):
            structural_gate = properties['extra_bridge_layers_match'] and len(properties['bridge_layers'][0]) >= 2
        else:
            left, right = properties['second_layer_start_ramp']
            structural_gate = left['segments'] == right['segments'] > 0 and left['speeds'] == right['speeds'] and abs(right['xy_length']/left['xy_length']-1) <= 0.03
        summary[name]={'roles':delta, 'structural_properties': properties, 'structural_gate': structural_gate, 'validation_errors': [r['errors'] for r in reports],
                       'role_gate':all(abs(v['delta_percent'])<=3 if v['delta_percent'] is not None else v['mica']<1 for v in delta.values() if max(v['orca'],v['mica'])>=1)}
        (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(output/'summary.json')
    return int(any(not row.get('role_gate', False) or not row.get('structural_gate', False) or any(row.get('validation_errors', [])) for row in summary.values()))


if __name__ == '__main__': raise SystemExit(main())
