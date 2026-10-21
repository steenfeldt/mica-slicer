#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Measure release slices and compare outputs.

Uses wait4's per-child rusage because macOS time -l may be sandbox-restricted.
All generated models, logs and G-code must be outside the repository.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
# Loading the comparison helper must not leave generated files in the checkout.
sys.dont_write_bytecode = True


def measure(binary, name, model, options, output):
    command = [str(binary), 'slice', str(model), *options,
               '-o', str(output / (name + '.gcode'))]
    with (output / (name + '.log')).open('w') as log:
        start = time.monotonic()
        child = subprocess.Popen(command, stdout=log, stderr=log,
                                 env=dict(os.environ, MICA_TIMINGS='1', SOURCE_DATE_EPOCH='0'))
        _, status, usage = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(status)
        result = dict(elapsed_s=time.monotonic() - start,
                      user_s=usage.ru_utime, system_s=usage.ru_stime,
                      peak_rss_bytes=usage.ru_maxrss * (1 if sys.platform == 'darwin' else 1024),
                      exit=child.returncode, command=command)
    (output / (name + '.json')).write_text(json.dumps(result, indent=2) + '\n')
    if child.returncode:
        raise RuntimeError(f'{name} failed; see {output / (name + ".log")}')
    print(f'{name}: {result["elapsed_s"]:.3f} s, '
          f'{result["user_s"]:.3f} s user, '
          f'{result["peak_rss_bytes"] / 2**20:.1f} MiB', flush=True)


def compare(name, baseline, output):
    before, after = (p / (name + '.gcode') for p in (baseline, output))
    identical = subprocess.run(['cmp', '-s', str(before), str(after)]).returncode == 0
    if not identical:
        raise RuntimeError(f'G-code changed: {name}')
    if name != 'street':
        print(f'  {name}: cmp identical', flush=True)
        return
    spec = importlib.util.spec_from_file_location('painting_comparison', ROOT / 'scripts/compare-painting.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    a, b = (module.read(p)[0] for p in (before, after))
    totals = []
    for tool in sorted(set(a) | set(b)):
        old, new = sum(a[tool].values()), sum(b[tool].values())
        assert abs(new - old) <= 0.0001 * old, (tool, old, new)
        totals.append(dict(tool=tool + 1, before_mm=old, after_mm=new))
    report = dict(byte_identical=identical, tools=totals)
    (output / 'street-comparison.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'  street: cmp {"identical" if identical else "different"}; all tools within 0.01%', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/mica')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--compare', type=Path)
    parser.add_argument('--street', type=Path, help='read-only dense-painted 3MF')
    parser.add_argument('--cat', type=Path, help='read-only cat 3MF; plate 1, U1, supports off')
    parser.add_argument('--u1', action='store_true', help='also compare cube20 and Benchy on U1')
    parser.add_argument('--skip-dense', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    if output.is_relative_to(ROOT):
        parser.error('--output must be outside the repository')
    output.mkdir(parents=True, exist_ok=True)
    fixtures = output / 'fixtures'
    subprocess.run([sys.executable, str(ROOT / 'scripts/make-painting-oracle.py'), str(fixtures)], check=True)
    jobs = [('cube20', ROOT / 'tests/models/cube20.stl', []),
            ('benchy', ROOT / 'tests/models/3DBenchy.stl', [])]
    u1 = ['--vendor', 'Snapmaker', '--printer', 'Snapmaker U1 (0.4 nozzle)',
          '--print', '0.20 Standard @Snapmaker U1 (0.4 nozzle)',
          '--filament', 'Snapmaker PLA @U1', '--bed', 'High Temp Plate']
    if args.u1:
        jobs += [('cube20-u1', ROOT / 'tests/models/cube20.stl', u1),
                 ('benchy-u1', ROOT / 'tests/models/3DBenchy.stl', u1),
                 # Orca-only options: inner-outer-inner walls, bridge line width, supports
                 # with small-overhang removal.
                 ('benchy-u1-orca', ROOT / 'tests/models/3DBenchy.stl', u1 + [
                     '--set', 'wall_sequence=inner-outer-inner wall',
                     '--set', 'bridge_line_width=0.5',
                     '--set', 'support_material=1',
                     '--set', 'support_remove_small_overhang=1'])]
    if not args.skip_dense:
        dense = output / 'dense.stl'
        subprocess.run([sys.executable, str(ROOT / 'scripts/gui-dense-fixture.py'), str(dense)], check=True)
        jobs.append(('dense', dense, []))
    for model in sorted(fixtures.glob('*.3mf')):
        options = ['--set', 'wipe_tower=0']
        if model.stem.startswith('colour'):
            options += ['--printer', 'Original Prusa XL - 5T Input Shaper 0.4 nozzle',
                        '--print', '0.20mm SPEED @XLIS 0.4', '--filament', 'Prusament PLA @XLIS',
                        '--center', '180,180']
        jobs.append((model.stem, model, options))
    if args.street:
        jobs.append(('street', args.street, u1 + ['--center', '135,135', '--set', 'wipe_tower=0']))
    if args.cat:
        # The project's own settings (organic tree supports), and the explicit profiles only.
        jobs.append(('cat', args.cat, u1 + ['--plate', '1']))
        jobs.append(('cat-profiles', args.cat, u1 + ['--plate', '1', '--project-settings', 'ignore']))
    for name, model, options in jobs:
        measure(args.binary.resolve(), name, model, options, output)
        if args.compare:
            compare(name, args.compare, output)


if __name__ == '__main__':
    main()
