#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""ColorMix CLI oracle: six generated XL 5T projects, 100 layers each, no user models.
Reference outputs/config and Mica's settings directory live in the system temporary directory.
Usage: python3 tests/oracles/colormix.py [--output TEMP_DIRECTORY] [--reference PRUSASLICER]
Build the release mica-cli first. No packages beyond Python's standard library are needed.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
# The reference apps (default: out/ref in the checkout; MICA_REF_DIR overrides).
REF_ROOT = Path(os.environ.get('MICA_REF_DIR', ROOT / 'out/ref'))
sys.path.insert(0, str(ROOT / 'tests/models'))
import colormix
spec = importlib.util.spec_from_file_location('multitool', ROOT / 'scripts/compare-multitool.py')
multitool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(multitool)
PROFILE = ['--printer-profile', 'Original Prusa XL - 5T Input Shaper 0.4 nozzle',
           '--print-profile', '0.20mm SPEED @XLIS 0.4', '--material-profile', 'Prusament PLA @XLIS']


def run(command, log, env):
    result = subprocess.run([str(v) for v in command], cwd=ROOT, env=env,
                            capture_output=True, text=True, timeout=1800)
    log.write_text(result.stdout + result.stderr)
    if result.returncode:
        raise RuntimeError(f'Exit {result.returncode}, see {log}')
    return result


def totals(data, model_only):
    return {tool: sum(v for role, v in roles.items()
                      if not model_only or role not in ('Skirt', 'Skirt/Brim', 'Wipe tower'))
            for tool, roles in data['filament_mm'].items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--reference', type=Path, default=REF_ROOT / 'PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer')
    parser.add_argument('--datadir', type=Path, default=REF_ROOT / 'datadir')
    args = parser.parse_args()
    out = args.output or Path(tempfile.mkdtemp(prefix='mica-colormix-oracle-'))
    out.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    # The CLI only reads/writes this fresh config directory, including validate launches.
    with tempfile.TemporaryDirectory(prefix='mica-colormix-config-') as config_dir:
        env['MICA_CONFIG_DIR'] = config_dir
        reference = [args.reference, '--datadir', args.datadir]
        run(reference + PROFILE + ['--save', out / 'xl.ini', '--export-3mf', '-o', out / 'base.3mf',
                                    ROOT / 'tests/models/cube20.stl'], out / 'profile.log', env)
        ini = (out / 'xl.ini').read_text()
        # Prusa's CLI saves a single-entry matrix for a multi-tool preset. Its tower prime()
        # indexes a square matrix even with custom volumes disabled: provide all 25 slots.
        matrix = ','.join('0' if i == j else '140' for i in range(5) for j in range(5))
        ini = ini.replace('wiping_volumes_matrix = 0\n', f'wiping_volumes_matrix = {matrix}\n')
        (out / 'xl.ini').write_text(ini)
        embedded = ''.join('; ' + line + '\n' for line in ini.splitlines() if line and not line.startswith('#'))
        colormix.generate(out, embedded)
        binary = ROOT / 'target/release/mica'
        profiles = json.loads((ROOT / 'tests/corpus/manifest.json').read_text())['printers']['xl']
        results = {}
        for case in colormix.CASES:
            model = out / f'colormix_{case}.3mf'
            ref, mica = out / f'ref-{case}.gcode', out / f'mica-{case}.gcode'
            run(reference + ['--load', out / 'xl.ini', '--no-binary-gcode', '--export-gcode', '-o', ref, model], out / f'ref-{case}.log', env)
            # No printer, process, filament or ColorMix flags: use the embedded project.
            run([binary, 'slice', model, '-o', mica], out / f'mica-{case}.log', env)
            a, b = multitool.analyze(ref), multitool.analyze(mica)
            mismatches = [i for i in range(max(len(a['layers']), len(b['layers'])))
                          if (a['layers'][i] if i < len(a['layers']) else None) !=
                             (b['layers'][i] if i < len(b['layers']) else None)]
            record = {'layers': [len(a['layers']), len(b['layers'])], 'mismatches': mismatches,
                      'reference': a, 'mica': b}
            for name, model_only in [('model', True), ('all_roles', False)]:
                af, bf = totals(a, model_only), totals(b, model_only)
                record[name + '_delta_percent'] = {t: 100 * (bf.get(t, 0) / v - 1) for t, v in af.items() if v > 0}
                assert all(abs(d) <= 3 for d in record[name + '_delta_percent'].values()), record
            validation = json.loads(run([binary, 'validate', mica, *profiles, '--json'], out / f'validate-{case}.json', env).stdout)
            record['validation_errors'] = validation['errors']
            assert validation['errors'] == 0, record
            assert len(a['layers']) == len(b['layers']) == 100 and not mismatches, record
            results[case] = record
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            print(f'{case}: 100 layers, 0 mismatches, 0 validation errors; filament within 3%', flush=True)
    print(out)


if __name__ == '__main__':
    main()
