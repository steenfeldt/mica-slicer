#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Infill and surface-finishing oracle. Outputs, logs and previews stay in a system-temp directory.

Build with cargo build --offline --release -p mica-cli first.
Example: python3 scripts/compare-infill.py --patterns concentric,honeycomb --models cube30,3DBenchy
"""
import argparse
import collections
import json
import math
import os
import pathlib
import re
import struct
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
# The reference apps (default: out/ref in the checkout; MICA_REF_DIR overrides).
REFERENCE = pathlib.Path(os.environ.get('MICA_REF_DIR', ROOT / 'out/ref'))
PRUSA = REFERENCE / 'PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer'
PATTERNS = 'concentric,honeycomb,3dhoneycomb,hilbertcurve,archimedeanchords,octagramspiral,adaptivecubic,supportcubic,lightning'

def parse(path):
    roles = collections.defaultdict(lambda: [0., 0., 0])
    role = None
    x = y = e = 0.
    relative = False
    for line in path.open():
        if line.startswith(';TYPE:'):
            role = line[6:].strip()
        if line.startswith('M83'):
            relative = True
        if line.startswith('M82'):
            relative = False
        if line.startswith('G92 E'):
            e = float(line.split()[1][1:])
        if not line.startswith(('G0 ', 'G1 ', 'G2 ', 'G3 ')):
            continue
        w = {k: float(v) for k, v in re.findall(r'([XYEIJR])(-?[\d.]+)', line.split(';')[0])}
        nx, ny = w.get('X', x), w.get('Y', y)
        de = w.get('E', 0.) if relative else w.get('E', e) - e
        if de > 0 and ('X' in w or 'Y' in w) and role:
            length = math.hypot(nx-x, ny-y)
            if line.startswith(('G2 ', 'G3 ')):
                if 'R' in w:
                    radius = abs(w['R'])
                    angle = 2 * math.asin(min(1., length / (2 * radius)))
                    if w['R'] < 0:
                        angle = math.tau - angle
                else:
                    cx, cy = x + w.get('I', 0), y + w.get('J', 0)
                    radius = math.hypot(x-cx, y-cy)
                    angle = (math.atan2(ny-cy, nx-cx) - math.atan2(y-cy, x-cx)) % math.tau
                    if line.startswith('G2 '):
                        angle = (-angle) % math.tau
                length = radius * angle
            roles[role][0] += de
            roles[role][1] += length
            roles[role][2] += 1
        x, y = nx, ny
        if not relative:
            e = w.get('E', e)
    return dict(roles)

def cube30(path):
    data = bytearray((ROOT / 'tests/models/cube20.stl').read_bytes())
    if len(data) == 84 + struct.unpack_from('<I', data, 80)[0] * 50:
        for i in range(struct.unpack_from('<I', data, 80)[0]):
            for j in range(9):
                offset = 84 + i*50 + 12 + j*4
                struct.pack_into('<f', data, offset, struct.unpack_from('<f', data, offset)[0] * 1.5)
        path.write_bytes(data)
    else:
        text = data.decode()
        text = re.sub(r'vertex\s+(\S+)\s+(\S+)\s+(\S+)', lambda m: 'vertex ' + ' '.join(str(float(v)*1.5) for v in m.groups()), text)
        path.write_text(text)

def run(cmd, log):
    with log.open('w') as f:
        result = subprocess.run([str(v) for v in cmd], cwd=ROOT, stdout=f, stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(f'{cmd[0]} exited {result.returncode}: {log.read_text()[-2500:]}')

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path)
    parser.add_argument('--patterns', default=PATTERNS)
    parser.add_argument('--models', default='cube30,3DBenchy')
    parser.add_argument('--features', action='store_true')
    parser.add_argument('--reuse-reference', action='store_true')
    parser.add_argument('--preview', action='store_true')
    args = parser.parse_args()
    out = args.output or pathlib.Path(tempfile.mkdtemp(prefix='mica-m7-'))
    out.mkdir(parents=True, exist_ok=True)
    cube30(out / 'cube30.stl')
    print(out, flush=True)
    jobs = [(model, p, {'fill_pattern': p, 'fill_density': '20%'}, 'Internal infill') for model in args.models.split(',') for p in args.patterns.split(',') if p]
    if args.features:
        jobs += [(m, 'ironing', {'ironing': '1', 'ironing_type': 'top'}, 'Ironing') for m in ['cube20', '3DBenchy']]
        jobs += [(m, 'fuzzy-'+g, {'fuzzy_skin': 'external', 'perimeter_generator': g}, 'External perimeter') for m in ['cube20', '3DBenchy'] for g in ['classic', 'arachne']]
    report = []
    for model, name, settings, role in jobs:
        source = out / 'cube30.stl' if model == 'cube30' else ROOT / f'tests/models/{model}.stl'
        ref, mica = [out / f'{model}-{name}-{kind}.gcode' for kind in ['prusa', 'mica']]
        if not (args.reuse_reference and ref.exists()):
            cmd = [PRUSA, '--datadir', REFERENCE / 'datadir', '--printer-profile', 'Prusa CORE One HF0.4 nozzle', '--print-profile', '0.20mm SPEED @COREONE HF0.4', '--material-profile', 'Prusament PLA @COREONE HF0.4', '--center', '125,110', '--no-binary-gcode', '--export-gcode', '-o', ref]
            for key, value in settings.items():
                cmd += ['--'+key.replace('_', '-')] + ([] if key == 'ironing' else [value])
            run(cmd+[source], ref.with_suffix('.log'))
        cmd = [ROOT / 'target/release/mica', 'slice', source, '--bundle', REFERENCE / 'datadir/vendor/PrusaResearch.ini', '-o', mica]
        for key, value in settings.items():
            cmd += ['--set', key+'='+value]
        if args.preview:
            cmd += ['--preview', out / f'{model}-{name}-preview', '--layers', '2,50,95']
        run(cmd, mica.with_suffix('.log'))
        reference, actual = parse(ref).get(role, [0,0,0]), parse(mica).get(role, [0,0,0])
        delta = [100*(m/r-1) if r else None for r,m in zip(reference, actual)]
        limit = 8. if name in ('adaptivecubic', 'supportcubic', 'lightning') else 3.
        metrics = [0, 1] if name == 'ironing' else [0]
        if name.startswith('fuzzy-'):
            limit, metrics = 5., [1, 2]
        passed = all(delta[i] is not None and abs(delta[i]) <= limit for i in metrics)
        row = dict(model=model, feature=name, role=role, reference=reference, mica=actual,
                   delta_percent=delta, tolerance_percent=limit, passed=passed)
        report.append(row)
        print(f'{model:10} {name:20} E {delta[0]:+7.2f}% length {delta[1]:+7.2f}% points {delta[2]:+7.2f}%', flush=True)
        (out/'results.json').write_text(json.dumps(report, indent=2)+'\n')
    if any(not row['passed'] for row in report):
        raise SystemExit('Oracle tolerance exceeded; see results.json')

if __name__ == '__main__':
    main()
