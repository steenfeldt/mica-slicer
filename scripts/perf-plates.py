#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Measure cat plates with organic supports; outputs stay outside the repository.

Use a baseline executable with plate selection + timing instrumentation but without
performance changes. --plates all measures the legacy global layout, which may fail
printable-volume validation; its status and timings are still recorded.
"""
import argparse
import importlib.util
from pathlib import Path
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('perf', ROOT / 'scripts/perf-slicing.py')
perf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(perf)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model', type=Path)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/mica')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--plates', nargs='+', default=['1', '2', '3', '4', '5'])
    parser.add_argument('--compare', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.is_relative_to(ROOT):
        parser.error('--output must be outside the repository')
    if any(p != 'all' and (not p.isdigit() or int(p) == 0) for p in args.plates):
        parser.error('--plates expects positive numbers or all')
    output.mkdir(parents=True, exist_ok=True)
    options = ['--vendor', 'Snapmaker', '--printer', 'Snapmaker U1 (0.4 nozzle)',
               '--print', '0.20 Standard @Snapmaker U1 (0.4 nozzle)',
               '--filament', 'Snapmaker PLA @U1', '--bed', 'High Temp Plate',
               '--set', 'support_material=1', '--set', 'support_material_buildplate_only=1',
               '--set', 'support_material_style=organic']
    for plate in args.plates:
        selection = ['--all-plates'] if plate == 'all' else ['--plate', plate]
        name = 'cat-' + plate
        try:
            perf.measure(args.binary.resolve(), name, args.model.resolve(), options + selection, output)
        except RuntimeError:
            if plate != 'all' or 'outside the printable volume' not in (output / (name + '.log')).read_text():
                raise
            print(f'{name}: failed; inspect {output / (name + ".log")} and recorded exit status', flush=True)
        else:
            if args.compare:
                perf.compare(name, args.compare, output)


if __name__ == '__main__':
    main()
