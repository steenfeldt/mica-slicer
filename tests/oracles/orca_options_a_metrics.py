#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Read G-code E deltas by role, plus XY hole widths from linear outer-wall paths at Z=1.05.
Usage: python3 tests/oracles/orca_options_a_metrics.py /tmp/output
The hole boundary estimate is centreline extent minus the stated extrusion width.
"""
from collections import defaultdict
import json
import math
from pathlib import Path
import re
import sys


def metrics(file):
    role='';width=0.;x=y=z=e=0.;relative=False
    totals=defaultdict(float);paths=[];current=[];current_width=0.;feed=0.;speeds=defaultdict(set)
    def flush():
        nonlocal current
        if len(current)>8:
            dx=max(p[0] for p in current)-min(p[0] for p in current)
            dy=max(p[1] for p in current)-min(p[1] for p in current)
            if 2<dx<12 and 2<dy<12:
                paths.append({'x':dx-current_width,'y':dy-current_width,'width':current_width})
        current=[]
    for raw in file.read_text().splitlines():
        if raw.startswith(';TYPE:'):
            if raw[6:]!=role:flush()
            role=raw[6:]
        if raw.startswith(';WIDTH:'):width=float(raw[7:])
        line=raw.split(';')[0].strip()
        fields=line.split()
        if not fields:continue
        if fields[0]=='M83':relative=True
        if fields[0]=='M82':relative=False
        if fields[0]=='G92':
            for f in fields[1:]:
                if f.startswith('E'):e=float(f[1:])
        if fields[0] not in ['G0','G1','G2','G3']:continue
        values={m.group(1):float(m.group(2)) for m in re.finditer(r'([XYZEFIJKR])([-+]?\d*\.?\d+(?:[eE][-+]?\d+)?)',line)}
        nx,ny,nz=values.get('X',x),values.get('Y',y),values.get('Z',z)
        if 'F' in values:feed=values['F']/60
        de=values.get('E',0) if relative else values.get('E',e)-e
        if 'E'in values:e=e+de if relative else values['E']
        if de>0 and (nx!=x or ny!=y):
            totals[role]+=de
            if abs(nz-1.05)<0.001:
                speeds[role].add(round(feed,3))
            if role=='Outer wall' and abs(nz-1.05)<0.001:
                if not current:current=[(x,y)];current_width=width
                current.append((nx,ny))
            else:flush()
        elif nx!=x or ny!=y or nz!=z:flush()
        x,y,z=nx,ny,nz
    flush()
    return {'filament_mm':dict(sorted(totals.items())),'holes':sorted(paths,key=lambda p:p['x']),'speeds_at_z1.05':{k:sorted(v) for k,v in speeds.items()}}


if __name__=='__main__':
    out=Path(sys.argv[1])
    report={}
    for case in ['xy','posts','bead','gap','flow']:
        report[case]={}
        for slicer,file in [('orca','plate_1.gcode'),('mica','mica.gcode')]:
            path=out/case/file
            if path.exists():report[case][slicer]=metrics(path)
    print(json.dumps(report,indent=2))
    if '--check' in sys.argv:
        a,b=report['xy']['orca']['holes'],report['xy']['mica']['holes']
        assert len(a)==len(b)==3, (a,b)
        for orca,mica in zip(a,b):
            for axis in ['x','y']:
                assert abs(orca[axis]-mica[axis])<=0.02, (axis,orca,mica)

