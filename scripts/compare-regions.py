#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare deposited filament by extrusion role and slice-height band in text G-code."""
import collections
import json
import pathlib
import re
import sys

ALIASES = {'Outer wall':'External perimeter','Inner wall':'Perimeter','Sparse infill':'Internal infill','Internal solid infill':'Solid infill','Bottom surface':'Solid infill','Brim':'Skirt/Brim','Top surface':'Top solid infill','Gap infill':'Gap fill','Overhang wall':'Overhang perimeter','Support':'Support material','Support interface':'Support material interface'}

def analyze(path):
    totals=collections.defaultdict(float); bands=collections.defaultdict(lambda:collections.defaultdict(float)); layers={}
    z=0.; height=.2; height_pending=False; role='Custom'; active=False; relative=True; last_e=0.; tool=0; debts=collections.defaultdict(float)
    for line in pathlib.Path(path).read_text().splitlines():
        if line==';LAYER_CHANGE':active=True
        if line.startswith(';Z:'):
            z=float(line[3:]); height_pending=True
        # Later HEIGHT tags describe bridge/combined extrusion flow, not the layer's slice plane.
        if line.startswith(';HEIGHT:') and height_pending:
            height=float(line[8:]); height_pending=False
        if line.startswith(';TYPE:'):role=ALIASES.get(line[6:],line[6:])
        code=line.split(';')[0].strip(); fields=code.split(); cmd=fields[0] if fields else ''
        values={k:float(v) for k,v in re.findall(r'\b([XYZET])([-+]?(?:\d*\.\d+|\d+\.?))',code)}
        if cmd=='M83':relative=True
        if cmd=='M82':relative=False
        if re.fullmatch(r'T\d+',cmd):tool=int(cmd[1:])
        if cmd=='G92' and 'E' in values:last_e=values['E']
        if cmd not in ['G0','G1','G2','G3'] or 'E' not in values:continue
        amount=values['E'] if relative else values['E']-last_e; last_e=values['E']
        if amount<0:debts[tool]-=amount;continue
        repayment=min(debts[tool],amount); debts[tool]-=repayment; amount-=repayment
        if active and role!='Custom' and amount>0 and ('X' in values or 'Y' in values):
            totals[role]+=amount
            # The range config is selected at the slicing plane, not at print_z.
            band=int(max(0,z-height/2)/5)*5
            bands[f'{band}-{band+5}'][role]+=amount
            layers.setdefault(round(z,6),collections.defaultdict(float))[role]+=amount
    return {'totals':dict(totals),'bands':dict(bands),'layers':layers}

def compare(ref,mica):
    result={}
    for group in ['totals','bands']:
        a,b=ref[group],mica[group]
        if group=='totals':a,b={'all':a},{'all':b}
        result[group]={k:{r:{'reference':round(a.get(k,{}).get(r,0),4),'mica':round(b.get(k,{}).get(r,0),4),'percent':round(100*(b.get(k,{}).get(r,0)/a[k][r]-1),3) if a.get(k,{}).get(r) else None} for r in sorted(set(a.get(k,{}))|set(b.get(k,{})))} for k in sorted(set(a)|set(b))}
    return result

if __name__=='__main__':print(json.dumps(compare(analyze(sys.argv[1]),analyze(sys.argv[2])),indent=2))
