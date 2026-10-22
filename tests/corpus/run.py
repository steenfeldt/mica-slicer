#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Offline safety/quality corpus. Only numeric reference summaries are stored in the repo."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = Path(__file__).with_name('manifest.json')
REFERENCES = Path(__file__).with_name('references.json')
ALIASES = {'External perimeter':'Outer wall', 'Perimeter':'Inner wall', 'Internal infill':'Sparse infill',
           'Solid infill':'Internal solid infill', 'Top solid infill':'Top surface',
           'Bridge infill':'Bridge', 'Support material':'Support', 'Support material interface':'Support interface',
           'Skirt':'Skirt/Brim', 'Skirt/Brim':'Skirt/Brim'}


def layer_tool_summary(path):
    analyze = runpy.run_path(str(ROOT / 'scripts/compare-multitool.py'))['analyze']
    layers = analyze(path)['layers']
    return {'count': len(layers), 'sha256': hashlib.sha256(json.dumps(layers, separators=(',', ':')).encode()).hexdigest()}


def roles(data):
    result = {}
    for role, value in data.items():
        role = ALIASES.get(role, role)
        result[role] = result.get(role, 0) + value
    return result


def without_presets(profile):
    """The profile arguments without --print/--filament (vendor, printer and bed stay)."""
    out, skip = [], False
    for arg in profile:
        if skip:
            skip = False
        elif arg in ('--print', '--filament'):
            skip = True
        else:
            out.append(arg)
    return out


def run(command, timeout=1800):
    return subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=timeout)


def validate(binary, path, profile):
    result = run([str(binary), 'validate', str(path), *profile, '--json'])
    try:
        data = json.loads(result.stdout)
    except ValueError:
        raise RuntimeError(result.stderr or result.stdout or 'validator produced no report')
    return data


def generate(directory):
    sys.path.insert(0, str(ROOT / 'tests/models'))
    import overhang_staircase
    import bridges
    import support_organic
    from support_overhangs import write_model
    overhang_staircase.generate(directory / 'overhang_staircase.stl')
    bridges.main(directory)
    support_organic.main(directory)
    write_model(directory / 'tall_tower.stl', [(0, 0, 0, 4, 4, 80)])
    import sequential_cubes
    sequential_cubes.generate(directory / 'sequential_cubes.3mf')
    sequential_cubes.generate_small(directory / 'sequential_cubes_small.3mf')
    import colormix
    colormix.generate(directory)
    import vase
    vase.generate(directory / 'vase.stl')


def model_path(case, temp):
    if 'private_env' in case:
        return Path(os.environ.get(case['private_env'], case['private_default'])).expanduser()
    return temp / case['generated'] if 'generated' in case else ROOT / case['model']


def timed_run(command, timeout=1800):
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr)
        deadline = time.monotonic()+timeout
        while True:
            pid, status, usage = os.wait4(child.pid, os.WNOHANG)
            if pid:
                child.returncode = os.waitstatus_to_exitcode(status)
                break
            if time.monotonic()>deadline:
                child.kill()
                os.wait4(child.pid, 0)
                child.returncode = -9
                raise subprocess.TimeoutExpired(command, timeout)
            time.sleep(0.01)
        stdout.seek(0); stderr.seek(0)
        result = subprocess.CompletedProcess(command, child.returncode, stdout.read().decode(errors='replace'), stderr.read().decode(errors='replace'))
        rss = usage.ru_maxrss / (1024**2 if platform.system() == 'Darwin' else 1024)
        return result, rss


def main():
    os.environ["SOURCE_DATE_EPOCH"] = "0"
    # The CLI runs as a copy outside the checkout, so name the checkout's pinned profiles
    # explicitly (used only when no PrusaSlicer/OrcaSlicer is installed, as on CI runners).
    os.environ.setdefault("MICA_UPSTREAM_DIR", str(ROOT / 'upstream'))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--quick', action='store_true')
    parser.add_argument('--case', action='append', help='case id (repeatable)')
    parser.add_argument('--printer', choices=list(json.loads(MANIFEST.read_text())['printers']))
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/mica')
    parser.add_argument('--record-reference', action='append', default=[], metavar='CASE/PRINTER=FILE')
    parser.add_argument('--role-tolerance', type=float, default=25., help='maximum role delta percent; role amounts <1mm ignored')
    args = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text())
    references = json.loads(REFERENCES.read_text()) if REFERENCES.exists() else {}
    temp = Path(tempfile.mkdtemp(prefix='mica-corpus-'))
    generate(temp)
    snapshot = temp / ('mica.exe' if os.name == 'nt' else 'mica')
    shutil.copy2(args.binary, snapshot)
    args.binary = snapshot
    for item in args.record_reference:
        key, path = item.split('=', 1)
        case_id, printer = key.split('/')
        case = next(c for c in manifest['cases'] if c['id'] == case_id)
        data = validate(args.binary, path, manifest['printers'][printer]+case.get('validate_args',[]))
        if data['errors']:
            raise RuntimeError(f'{key}: reference has {data["errors"]} validation errors: {data["diagnostics"]}')
        references[key] = {'source_file':Path(path).name, 'source_sha256':hashlib.sha256(Path(path).read_bytes()).hexdigest(),
                           'model_sha256':hashlib.sha256(model_path(case,temp).read_bytes()).hexdigest(),
                           'profile_args':manifest['printers'][printer], 'roles':roles(data['roles']),
                           'filament_mm':data['filament_mm'], 'validator_warnings':data['warnings']}
        if case.get('layer_sequence_checks'):
            references[key]['layer_tools'] = layer_tool_summary(path)
        if case.get('tool_checks'):
            # Which tool prints what (supports in another filament): per-tool roles and tool changes.
            references[key]['tool_roles'] = [roles(t) for t in data['tool_roles']]
            references[key]['toolchanges'] = data['toolchanges']
    if args.record_reference:
        REFERENCES.write_text(json.dumps(references, indent=2, sort_keys=True)+'\n')
        print(f'Recorded {len(args.record_reference)} validated references (numbers only).')
        return 0
    rows = []
    details = []
    failed = 0
    start = time.perf_counter()
    for case in manifest['cases']:
        if args.quick and not case.get('quick') or args.case and case['id'] not in args.case:
            continue
        for printer, profile in manifest['printers'].items():
            if args.printer and printer != args.printer:
                continue
            key = case['id']+'/'+printer
            if printer not in case.get('printers', manifest['printers']):
                rows.append([case['id'],printer,'N/A (printer/fixture)','—','—','—','—','—'])
                continue
            path = model_path(case,temp)
            if not path.is_file():
                if 'private_env' in case:
                    rows.append([case['id'],printer,'SKIP (private absent)','—','—','—','—','—'])
                    continue
                raise RuntimeError(f'missing required corpus model: {path}')
            output = temp/(case['id']+'-'+printer+'.'+case.get('encoding','gcode'))
            log = temp/(case['id']+'-'+printer+'.log')
            # A project case slices with the project's own process and filaments: an explicit
            # --print/--filament naming other presets would replace the project's values.
            slice_profile = without_presets(profile) if case.get('project_presets') else profile
            command = [str(args.binary),'slice',str(path),*slice_profile,*case.get('args',[]),'-o',str(output)]
            print(f'{key}: slicing', flush=True)
            tick = time.perf_counter()
            try:
                result, rss = timed_run(command)
                elapsed = time.perf_counter()-tick
                log.write_text(result.stdout+result.stderr)
                if result.returncode:
                    raise RuntimeError(f'slice exit {result.returncode}: '+result.stderr[:1200])
                # validate_args describe per-tool project filaments the single-filament profile lacks.
                data = validate(args.binary,output,profile+case.get('validate_args',[]))
                (temp/(case['id']+'-'+printer+'.json')).write_text(json.dumps(data,indent=2))
                if sum(v>1 for v in data['filament_mm']) < case.get('min_tools',1):
                    raise RuntimeError('fixture did not exercise the required number of tools')
                if case.get('require_role') and data['roles'].get(case['require_role'],0)<=0:
                    raise RuntimeError('fixture did not print the required tower')
                deltas = []
                ref = references.get(case.get('reference_case',case['id'])+'/'+printer)
                comparison = 'no reference'
                if ref:
                    if ref['model_sha256'] != hashlib.sha256(path.read_bytes()).hexdigest() or ref['profile_args'] != profile:
                        raise RuntimeError('reference model/profile mismatch; recalibrate explicitly')
                    if case.get('layer_sequence_checks') and layer_tool_summary(output) != ref.get('layer_tools'):
                        raise RuntimeError('per-layer tool sequences differ from the oracle')
                    if 'filament_tolerance' in case:
                        for tool, amount in enumerate(ref['filament_mm']):
                            actual_amount = data['filament_mm'][tool] if tool < len(data['filament_mm']) else 0
                            if amount > 1 and abs(actual_amount / amount - 1) * 100 > case['filament_tolerance']:
                                raise RuntimeError(f'T{tool} filament differs by more than {case["filament_tolerance"]}%')
                    actual = roles(data['roles'])
                    # Orca splits bottom surface out of solid infill; Prusa includes it.
                    expected = dict(ref['roles'])
                    for table in (actual,expected):
                        if 'Bottom surface' in table:
                            table['Internal solid infill']=table.get('Internal solid infill',0)+table.pop('Bottom surface')
                    for role in sorted(actual.keys() | expected.keys()):
                        a,b = actual.get(role,0),expected.get(role,0)
                        if max(a,b)<1: continue
                        delta = 100*(a-b)/max(b,1)
                        deltas.append((role,delta))
                    # Per tool: the support base, interface and tower must stay on the tool the
                    # reference prints them with.
                    for tool, expected_tool in enumerate(ref.get('tool_roles', [])):
                        actual_tool = roles(data['tool_roles'][tool]) if tool < len(data['tool_roles']) else {}
                        expected_tool = dict(expected_tool)
                        for table in (actual_tool, expected_tool):
                            if 'Bottom surface' in table:
                                table['Internal solid infill']=table.get('Internal solid infill',0)+table.pop('Bottom surface')
                        for role in sorted(actual_tool.keys() | expected_tool.keys()):
                            a,b = actual_tool.get(role,0),expected_tool.get(role,0)
                            if max(a,b)<1: continue
                            deltas.append((f'T{tool} {role}', 100*(a-b)/max(b,1)))
                    comparison = f'{max((abs(d) for _,d in deltas),default=0):.1f}%'
                    if 'toolchanges' in ref:
                        allowed = case.get('toolchange_tolerance', 0)
                        if abs(data['toolchanges'] - ref['toolchanges']) > allowed:
                            deltas.append(('tool changes', float('inf')))
                        comparison += f', {data["toolchanges"]}/{ref["toolchanges"]} tool changes'
                # A case may name roles with a known, documented difference (validate_note) and
                # widen the tolerance; everything else keeps the gate.
                known = set(case.get('known_role_gaps', []))
                tolerance = case.get('role_tolerance', args.role_tolerance)
                gated = [(role, d) for role, d in deltas
                         if re.sub(r'^T\d+ ', '', role) not in known]
                mismatch = any(abs(d)>tolerance for _,d in gated)
                status = 'FAIL' if data['errors'] or mismatch else 'PASS'
                failed += status == 'FAIL'
                rows.append([case['id'],printer,status,str(data['errors']),str(data['warnings']),comparison,f'{elapsed:.2f}',f'{rss:.1f}' if rss else 'unavailable'])
                details.append({'case':key,'diagnostics':data['diagnostics'],'role_deltas_percent':dict(deltas), 'input_warnings':[line for line in result.stderr.splitlines() if line.startswith(('warning:', '3MF note:', 'Parts of this project'))]})
                print(f'{key}: {status}, {data["errors"]} errors, {data["warnings"]} warnings, {elapsed:.2f}s',flush=True)
            except (RuntimeError,subprocess.TimeoutExpired) as error:
                failed += 1
                rows.append([case['id'],printer,'FAIL','—','—','—',f'{time.perf_counter()-tick:.2f}','—'])
                details.append({'case':key,'failure':str(error)})
                print(f'{key}: FAIL: {error}',flush=True)
    elapsed = time.perf_counter()-start
    report = ['# Mica regression corpus','',f'Mode: {"quick" if args.quick else "full"}. Elapsed: {elapsed:.2f} s. Failures: {failed}.',
              f'Artifacts and diagnostics: `{temp}`. Times exclude compilation. RSS is the slice subprocess peak.',
              f'Per-role gate: ±{args.role_tolerance:g}% for roles using ≥1 mm; missing references are explicitly unmeasured.','',
              '| Model | Printer | Status | Errors | Warnings | Max role delta | Slice s | Peak MiB |',
              '|---|---|---|---:|---:|---:|---:|---:|']
    report += ['| '+' | '.join(row)+' |' for row in rows]
    report += ['', '## Findings', '']
    for detail in details:
        issues = detail.get('diagnostics',[])
        counts = {}
        for issue in issues:
            counts[issue['rule']] = counts.get(issue['rule'],0)+1
        if 'failure' in detail: report += [f'- {detail["case"]}: {detail["failure"].replace(chr(10)," ")}']
        elif counts: report += [f'- {detail["case"]}: '+', '.join(f'{rule} (first {count})' for rule,count in counts.items())]
        for warning in detail.get('input_warnings',[])[:20]:
            report += [f'- {detail["case"]} input: {warning}']
    (ROOT/'out').mkdir(exist_ok=True)
    (ROOT/'out/corpus-report.md').write_text('\n'.join(report)+'\n')
    (temp/'results.json').write_text(json.dumps(details,indent=2))
    # The quick budget (60 s on a developer Mac) can be widened for slower machines (CI runners).
    budget = float(os.environ.get('MICA_CORPUS_QUICK_BUDGET', '60'))
    if args.quick and elapsed>=budget:
        print(f'Quick corpus exceeded {budget:g} seconds: {elapsed:.2f}s',file=sys.stderr)
        failed+=1
    print(f'Report: {ROOT / "out/corpus-report.md"}')
    return bool(failed)


if __name__ == '__main__':
    sys.dont_write_bytecode = True
    sys.exit(main())
