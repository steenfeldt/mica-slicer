#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate batch A fixtures/settings/commands in a temporary directory; optionally run Orca once per case.
Stops immediately on the first failed Orca run. Standard library only. No reference files in the repo.
Usage: python3 tests/oracles/orca_options_a.py /tmp/output [--run-orca] [--include-off]
"""
import json
import math
from pathlib import Path
import shlex
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
ORCA = '/Applications/OrcaSlicer.app/Contents/MacOS/OrcaSlicer'
PROFILES = Path('/Applications/OrcaSlicer.app/Contents/Resources/profiles')


def write_stl(path, triangles):
    with path.open('wb') as f:
        f.write(b'Mica original dimension fixture'.ljust(80, b'\0'))
        f.write(struct.pack('<I', len(triangles)))
        for triangle in triangles:
            f.write(struct.pack('<12fH', 0, 0, 0, *(v for p in triangle for v in p), 0))


def fixtures(out):
    triangles = []
    def quad(a, b, c, d):
        triangles.extend([(a, b, c), (a, c, d)])
    # A single 48x16x3 plate with 3, 5 and 8 mm through holes, 192 facets per circle.
    # The square-to-circle annulus avoids a triangulation dependency.
    for cell, diameter in enumerate([3, 5, 8]):
        outer, inner = [], []
        for i in range(192):
            angle = 2*math.pi*i/192
            x, y = math.cos(angle), math.sin(angle)
            k = 8/max(abs(x), abs(y))
            outer.append((round(8+16*cell+k*x, 6), round(8+k*y, 6)))
            inner.append((round(8+16*cell+diameter/2*x, 7), round(8+diameter/2*y, 7)))
        for i in range(192):
            a,b = outer[i],outer[(i+1)%192]
            c,d = inner[(i+1)%192],inner[i]
            quad((*a,3),(*b,3),(*c,3),(*d,3))
            quad((*d,0),(*c,0),(*b,0),(*a,0))
            quad((*d,0),(*d,3),(*c,3),(*c,0))
            shared = ((cell>0 and abs(a[0]-16*cell)<1e-6 and abs(b[0]-16*cell)<1e-6)
                      or (cell<2 and abs(a[0]-16*(cell+1))<1e-6 and abs(b[0]-16*(cell+1))<1e-6))
            if not shared:
                quad((*a,0),(*b,0),(*b,3),(*a,3))
    write_stl(out/'holes.stl',triangles)
    triangles.clear()
    for i,diameter in enumerate([3,5,8]):
        ring=[(8+16*i+diameter/2*math.cos(2*math.pi*j/192),8+diameter/2*math.sin(2*math.pi*j/192)) for j in range(192)]
        for j,a in enumerate(ring):
            b=ring[(j+1)%192]
            triangles.extend([((8+16*i,8,0),(*b,0),(*a,0)),((8+16*i,8,8),(*a,8),(*b,8))])
            quad((*a,0),(*b,0),(*b,8),(*a,8))
    write_stl(out/'posts.stl',triangles)


def prepare(out):
    out.mkdir(parents=True,exist_ok=True)
    fixtures(out)
    presets={}
    for path in [p for directory in ['OrcaFilamentLibrary','Snapmaker'] for p in sorted((PROFILES/directory).rglob('*.json'))]:
        try:
            value=json.loads(path.read_text())
            if isinstance(value,dict) and 'name' in value and isinstance(value['name'],str):
                presets[value['name']]=value
        except (ValueError,UnicodeError):
            pass
    def flatten(name):
        value=presets[name].copy()
        parent=value.pop('inherits',None)
        return (flatten(parent) if parent else {}) | value
    machine=flatten('Snapmaker U1 (0.4 nozzle)')
    process=flatten('0.20 Standard @Snapmaker U1 (0.4 nozzle)')
    filament=flatten('Snapmaker PLA @U1')
    machine['curr_bed_type']='High Temp Plate'
    # Explicit vectors, plus four --load-filaments entries, avoid ToolOrdering's missing-slot crash.
    for key in ['nozzle_diameter','extruder_offset','retraction_length','retraction_speed','deretraction_speed','z_hop','retract_before_wipe','retract_length_toolchange']:
        if isinstance(machine.get(key),list) and machine[key]:
            machine[key]=(machine[key]*4)[:4]
    cases={
        'xy':(out/'holes.stl',{'xy_hole_compensation':'0.1','xy_contour_compensation':'-0.05','elefant_foot_compensation':'0','enable_arc_fitting':'0','wall_generator':'classic'}),
        'posts':(out/'posts.stl',{'small_perimeter_threshold':'4','small_perimeter_speed':'25%','slow_down_for_layer_cooling':'0','resonance_avoidance':'0','wall_generator':'classic'}),
        'bead':(ROOT/'tests/models/3DBenchy.stl',{'initial_layer_min_bead_width':'125%','wall_generator':'arachne'}),
        'gap':(ROOT/'tests/models/3DBenchy.stl',{'filter_out_gap_fill':'1','wall_generator':'classic'}),
        'flow':(ROOT/'tests/models/3DBenchy.stl',{'small_area_infill_flow_compensation':'1','enable_arc_fitting':'0'}),
    }
    if '--include-off' in sys.argv:
        for name,key in [('gap','filter_out_gap_fill'),('flow','small_area_infill_flow_compensation')]:
            model,overrides=cases[name]
            cases[name+'-off']=(model,overrides | {key:'0'})
    commands=[]
    mica=ROOT/'target/release/mica'
    for name,(model,overrides) in cases.items():
        case=out/name;case.mkdir(exist_ok=True)
        for file,settings in [('machine',machine),('process',process|overrides),('filament',filament | {k:[v] for k,v in overrides.items() if k=='slow_down_for_layer_cooling'})]:
            settings=settings|{'instantiation':'true'}
            (case/f'{file}.json').write_text(json.dumps(settings,indent=2))
        command=[ORCA,'--datadir',str(case/'data'),'--load-settings',f'{case}/machine.json;{case}/process.json','--load-filaments',';'.join([f'{case}/filament.json']*4),'--slice','0','--arrange','1','--outputdir',str(case),'--export-settings',str(case/'resolved.json'),str(model)]
        mica_command=[str(mica),'slice',str(model),'--vendor','Snapmaker','--printer','Snapmaker U1 (0.4 nozzle)','--print','0.20 Standard @Snapmaker U1 (0.4 nozzle)','--filament','Snapmaker PLA @U1','--bed','High Temp Plate','-o',str(case/'mica.gcode')]
        for key,value in overrides.items():
            key={'wall_generator':'perimeter_generator','enable_arc_fitting':'arc_fitting','slow_down_for_layer_cooling':'slowdown_below_layer_time'}.get(key,key)
            value={'0':'disabled','1':'emit_center'}.get(value,value) if key=='arc_fitting' else value
            mica_command+=['--set',f'{key}={value}']
        commands.append((case,command,mica_command))
    (out/'commands.sh').write_text('#!/bin/sh\nset -eu\n'+''.join(
        f'cd {shlex.quote(str(case))}\n{shlex.join(command)}\n{shlex.join(mica_command)}\n'
        + shlex.join([str(mica),'validate',str(case/'mica.gcode'),*mica_command[mica_command.index('--vendor'):mica_command.index('-o')]])+'\n'
        for case,command,mica_command in commands))
    (out/'cases.json').write_text(json.dumps({name:{'model':str(model),'overrides':overrides} for name,(model,overrides) in cases.items()},indent=2))
    return commands


if __name__=='__main__':
    out=Path(sys.argv[1]).resolve()
    commands=prepare(out)
    print(f'Commands and inputs: {out}')
    if '--run-orca' in sys.argv:
        for case,command,_ in commands:
            with (case/'orca.log').open('w') as log:
                result=subprocess.run(command,cwd=case,stdout=log,stderr=subprocess.STDOUT)
            print(f'{case.name}: Orca exit {result.returncode}')
            if result.returncode:
                print('Stopped after first failed attempt; commands.sh contains all reviewer commands.')
                sys.exit(1)
