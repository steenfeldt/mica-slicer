#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Offline end-to-end calibration gate. Outputs live in the system temp directory.

Run after cargo build --offline -p mica-cli:
    python3 tests/calibration/check.py [target/debug/mica]
The independent source schedule oracle is documented in docs/calibration.md.
"""
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BINARY = (ROOT / (sys.argv[1] if len(sys.argv) > 1 else 'target/debug/mica')).resolve()
PROFILES = json.loads((ROOT / 'tests/corpus/manifest.json').read_text())['printers']
DIRECTORY = Path(tempfile.mkdtemp(prefix='mica-calibration-'))
CASES = [
    ('flow', ['--pass', '1']), ('flow', ['--pass', '2']),
    ('pa-line', ['--start', '0', '--end', '0.02', '--step', '0.005', '--speed', '100']),
    ('pa-pattern', ['--start', '0', '--end', '0.02', '--step', '0.005', '--speed', '100']),
    ('pa-tower', ['--start', '0', '--end', '0.02', '--step', '0.005', '--speed', '100']),
    ('temperature', ['--start', '230', '--end', '220', '--step', '5']),
    ('retraction', ['--start', '0', '--end', '0.4', '--step', '0.1']),
    ('max-volumetric-speed', ['--start', '5', '--end', '7', '--step', '0.5']),
]


def run(args):
    result = subprocess.run([str(BINARY), *args], cwd=ROOT, capture_output=True, text=True)
    if result.returncode:
        raise AssertionError(f'{args}:\n{result.stdout}\n{result.stderr}')
    return result.stdout


def samples(text):
    return [float(v) for v in re.findall(r'; calibration sample=([^\s]+)', text)]


def close(actual, expected):
    assert len(actual) == len(expected), (len(actual), len(expected))
    assert all(abs(a-b) < 1e-5 for a,b in zip(actual,expected)), (actual,expected)


def single_tool(text, tool):
    """The test prints with `tool` alone: selected, and the only tool heated (by T number)."""
    selects = [int(t) for t in re.findall(r'^T(\d+)(?: |$)', text, re.M)]
    assert selects and set(selects) == {tool}, (tool, sorted(set(selects)))
    for line in re.findall(r'^M10[49] [^;\n]*', text, re.M):
        t = re.search(r' T(\d+)', line)
        s = re.search(r' S([\d.]+)', line)
        if t and s and int(t.group(1)) != tool:
            assert float(s.group(1)) == 0, (tool, line)


# Multi-tool printers also print every test on tool 3 (T2): only that tool is used.
RUNS = [(printer, profile, tool) for printer, profile in PROFILES.items()
        for tool in ([0] if printer == 'core' else [0, 2])]
rows = []
for printer, profile, tool in RUNS:
    for case_index, (kind, options) in enumerate(CASES):
        path = DIRECTORY / f'{printer}-T{tool}-{case_index}-{kind}.gcode'
        run(['calibrate', kind, *options, '--tool', str(tool + 1), *profile, '-o', str(path)])
        report = json.loads(run(['validate', str(path), *profile, '--json']))
        assert report['errors'] == 0, (path, report)
        assert not any(d['rule'] == 'layer-position' for d in report['diagnostics']), (path, report)
        text = path.read_text()
        if printer != 'core':
            single_tool(text, tool)
        values = samples(text)
        if kind == 'flow':
            expected = list(range(-20, 21, 5)) if case_index == 0 else list(range(-9, 1))
            assert len(values) == len(expected)*10, (path, len(values))
            close(values[:len(expected)], expected)
        elif kind in ('pa-line', 'pa-pattern'):
            expected = [0., 0.005, 0.01, 0.015, 0.02]
            close(values, expected*(4 if kind == 'pa-pattern' else 1))
            pattern = r'SET_PRESSURE_ADVANCE ADVANCE=([\d.]+)' if printer == 'u1' else r'M572 S([\d.]+) ; calibration'
            commands = [float(v) for v in re.findall(pattern, text)]
            assert commands and commands[-1] == 0., (path, commands[-1:])
            assert all(any(abs(v-x)<1e-8 for x in commands) for v in expected)
        else:
            heights = [float(v) for v in re.findall(r'^;Z:([^\s]+)', text, re.M)]
            assert len(values) == len(heights), (path, len(values), len(heights))
            if kind == 'pa-tower':
                close(values, [math.floor(z)*0.005 for z in heights])
            elif kind == 'retraction':
                # The runtime computes with unrounded heights; compare away from exact floors.
                for v,z in zip(values,heights):
                    if abs((z-0.4)-round(z-0.4)) > 1e-7:
                        assert abs(v-math.floor(max(0,z-0.4))*0.1) < 1e-5
                assert min(values) == 0 and 0.4 in values and max(values) <= 0.5
                # Check actual E-only negative moves, rather than only the sample comments.
                retracts = [float(v) for v in re.findall(r'^G1 E(-[\d.]+)', text, re.M)]
                # Profiles for all three printers use relative E. Startup purge may retract too.
                assert any(abs(v+0.4) < 1e-4 for v in retracts), (path, retracts[-10:])
            elif kind == 'temperature':
                n=len(heights)
                close(values, [230 if i<=1 else math.trunc((235+i/(n-1)*(220-235))/5)*5 for i in range(n)])
                close([float(v) for v in re.findall(r'M104 S([\d.]+)(?: T\d+)? ; calibration temperature',text)],values)
            else:
                close(values,[5+z*0.5 for z in heights])
        rows.append({'printer':printer,'tool':tool+1,'kind':kind,'options':options,'errors':report['errors'],'warnings':report['warnings'],'file':str(path)})
        print(printer,f'tool {tool+1}',kind,' '.join(options),'errors=',report['errors'],'warnings=',report['warnings'],flush=True)
(DIRECTORY / 'results.json').write_text(json.dumps(rows,indent=2)+'\n')
print(f'{len(rows)} validated calibrations; artifacts: {DIRECTORY}')
