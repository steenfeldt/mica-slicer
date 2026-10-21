#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: adapted
# Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.hpp :: arc_angle
# Changes: Original per-layer feedrate comparison; analytic arc lengths for length-weighted speeds.
"""Compare cooling feedrates, weighted by extrusion length (G1/G2/G3, relative/absolute E).

Usage: compare-cooling.py reference.gcode mica.gcode [--json report.json]
Reports each role's overall speed, worst per-layer average difference, and estimated time.
"""
import argparse
import collections
import json
import math
import re

ALIASES = {'Outer wall':'External perimeter', 'Inner wall':'Perimeter',
           'Sparse infill':'Internal infill', 'Internal solid infill':'Solid infill',
           'Top surface':'Top solid infill', 'Bottom surface':'Solid infill',
           'Internal Bridge':'Bridge infill', 'Bridge':'Bridge infill',
           'Skirt':'Skirt/Brim', 'Brim':'Skirt/Brim'}

def parse(path):
    layers = []
    role = None
    x = y = None
    e = 0.
    speed = 0.
    relative = False
    estimate = None
    for line in open(path):
        if line.startswith(';LAYER_CHANGE') or line.startswith('; CHANGE_LAYER'):
            layers.append(collections.defaultdict(lambda: [0., 0., set()]))
        if line.startswith(';TYPE:') or line.startswith('; FEATURE:'):
            role = line.split(':', 1)[1].strip()
            role = ALIASES.get(role, role)
        if 'estimated printing time (normal mode)' in line or 'model printing time' in line:
            text = line.split('=', 1)[-1].split(';')[0]
            estimate = sum(float(n)*{'d':86400, 'h':3600, 'm':60, 's':1}[u]
                           for n,u in re.findall(r'([\d.]+)\s*([dhms])', text))
        code = line.split(';')[0].split()
        if not code: continue
        command = code[0]
        if command == 'M83': relative = True
        if command == 'M82': relative = False
        if command not in ('G0','G1','G2','G3','G92'): continue
        values = {k:float(v) for k,v in re.findall(r'([XYZEIJFR])([-+\d.eE]+)', ' '.join(code[1:]))}
        if command == 'G92':
            e = values.get('E', e)
            continue
        if command not in ('G0','G1','G2','G3'): continue
        if 'F' in values: speed = values['F']/60
        nx,ny = values.get('X',x),values.get('Y',y)
        de = (values['E'] if relative else values['E']-e) if 'E' in values else 0.
        if 'E' in values: e += de
        if de>0 and x is not None and y is not None and ('X' in values or 'Y' in values) and layers and role:
            length = math.hypot(nx-x,ny-y)
            if command in ('G2','G3'):
                if 'R' in values:
                    radius = abs(values['R'])
                    sweep = 2*math.asin(min(1.,length/(2*radius)))
                    length = radius*(math.tau-sweep if values['R']<0 else sweep)
                else:
                    cx,cy = x+values.get('I',0),y+values.get('J',0)
                    radius = math.hypot(x-cx,y-cy)
                    a,b = math.atan2(y-cy,x-cx),math.atan2(ny-cy,nx-cx)
                    sweep = ((b-a) if command=='G3' else (a-b))%math.tau
                    length = radius*(sweep if sweep>1e-9 else math.tau)
            stats = layers[-1][role]
            stats[0] += length
            stats[1] += length*speed
            stats[2].add(round(speed,5))
        x,y=nx,ny
    return layers,estimate

def compare(a,b):
    al,at=parse(a);bl,bt=parse(b)
    roles = sorted(set(r for l in al+bl for r in l))
    report = {'layers':[len(al),len(bl)],'estimated_seconds':[at,bt],
              'estimated_error_pct':100*(bt/at-1) if at and bt else None,'roles':{}}
    for role in roles:
        def average(layers):
            length=sum(l[role][0] for l in layers)
            return sum(l[role][1] for l in layers)/length if length else None
        av,bv=average(al),average(bl)
        differences=[]
        for i,(x,y) in enumerate(zip(al,bl),1):
            if x[role][0] and y[role][0]:
                xs,ys=x[role][1]/x[role][0],y[role][1]/y[role][0]
                aset,bset=x[role][2],y[role][2]
                set_error=max(max(min(abs(v-w) for w in bset) for v in aset),
                              max(min(abs(v-w) for w in aset) for v in bset))
                differences.append({'speed_set_error':set_error,'layer':i,'reference':xs,'mica':ys,'delta':ys-xs,
                                    'reference_speeds':sorted(x[role][2]),'mica_speeds':sorted(y[role][2])})
        report['roles'][role]={'reference':av,'mica':bv,'error_pct':100*(bv/av-1) if av and bv else None,
                              'worst_layer':max(differences,key=lambda d:abs(d['delta']),default=None),
                              'layers_over_1mm_s':sum(abs(d['delta'])>1 for d in differences),
                              'feedrate_sets_over_1mm_s':sum(d['speed_set_error']>1 for d in differences),
                              'layers':differences}
    return report

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference');p.add_argument('mica');p.add_argument('--json')
    args=p.parse_args();r=compare(args.reference,args.mica)
    print('layers:',r['layers'],'estimated seconds:',r['estimated_seconds'],'error %:',r['estimated_error_pct'])
    for role,s in r['roles'].items():
        worst=s['worst_layer']
        print(f"{role:23} {str(s['reference']):>18} {str(s['mica']):>18} error%={s['error_pct']} worst={None if not worst else (worst['layer'],round(worst['delta'],3))} layers>1={s['layers_over_1mm_s']} sets>1={s['feedrate_sets_over_1mm_s']}")
    if args.json:
        with open(args.json,'w') as f: json.dump(r,f,indent=2)
if __name__=='__main__': main()
