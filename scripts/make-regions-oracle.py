#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Create small Prusa 3MF region fixtures in a caller-supplied temporary directory."""
import math
import pathlib
import sys
import zipfile
from xml.sax.saxutils import escape

FACES = [(0,2,1),(0,3,2),(4,5,6),(4,6,7),(0,1,5),(0,5,4),(1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,0,4),(3,4,7)]
def box(a, b):
    x,y,z=a; u,v,w=b
    return [(x,y,z),(u,y,z),(u,v,z),(x,v,z),(x,y,w),(u,y,w),(u,v,w),(x,v,w)], FACES

def cylinder():
    n=64
    vertices=[(10+3*math.cos(2*math.pi*i/n),10+3*math.sin(2*math.pi*i/n),z) for z in [-1,21] for i in range(n)]+[(10,10,-1),(10,10,21)]
    faces=[]
    for i in range(n):
        j=(i+1)%n
        faces += [(i,j,n+j),(i,n+j,n+i),(2*n,j,i),(2*n+1,n+i,n+j)]
    return vertices,faces

def meta(values, kind):
    return ''.join(f'<metadata type="{kind}" key="{k}" value="{escape(v)}"/>' for k,v in values.items())

def write(root,name,volumes,obj=None,ranges=None):
    vertices=[]; triangles=[]; parts=''
    for kind, (v,f), settings in volumes:
        start=len(triangles); offset=len(vertices); vertices.extend(v); triangles.extend(tuple(i+offset for i in face) for face in f)
        parts+=f'<volume firstid="{start}" lastid="{len(triangles)-1}">'+meta(dict(name=kind,volume_type=kind,**settings),'volume')+'</volume>'
    geometry='<mesh><vertices>'+''.join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x,y,z in vertices)+'</vertices><triangles>'+''.join(f'<triangle v1="{a}" v2="{b}" v3="{c}"/>' for a,b,c in triangles)+'</triangles></mesh>'
    model='<?xml version="1.0"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><metadata name="slic3rpe:Version3mf">1</metadata><resources><object id="1" type="model">'+geometry+'</object></resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 115 100 0"/></build></model>'
    with zipfile.ZipFile(root/(name+'.3mf'),'w',zipfile.ZIP_DEFLATED) as z:
        z.writestr('[Content_Types].xml','<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>')
        z.writestr('_rels/.rels','<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>')
        z.writestr('3D/3dmodel.model',model)
        z.writestr('Metadata/Slic3r_PE_model.config','<config><object id="1" instances_count="1">'+meta(dict(name=name,**(obj or {})),'object')+parts+'</object></config>')
        if ranges:
            z.writestr('Metadata/Prusa_Slicer_layer_config_ranges.xml','<objects><object id="1">'+''.join(f'<range min_z="{a}" max_z="{b}">'+''.join(f'<option opt_key="{k}">{escape(v)}</option>' for k,v in values.items())+'</range>' for a,b,values in ranges)+'</object></objects>')

def main(root):
    root.mkdir(parents=True,exist_ok=True)
    cube=('ModelPart',box((0,0,0),(20,20,20)),{})
    write(root,'plain',[cube])
    write(root,'modifier',[cube,('ParameterModifier',box((5,5,5),(15,15,15)),{'fill_density':'100%'})])
    write(root,'height',[cube],ranges=[(5,10,{'layer_height':'0.2','perimeters':'4','fill_pattern':'gyroid'})])
    write(root,'negative',[cube,('NegativeVolume',cylinder(),{})])
    write(root,'object',[cube],obj={'layer_height':'0.1','perimeters':'3'})
    write(root,'combine',[cube],obj={'layer_height':'0.1','infill_every_layers':'3','solid_infill_every_layers':'20'})
    roof=[('ModelPart',box((8,8,0),(12,12,8)),{}),('ModelPart',box((0,0,8),(20,20,10)),{})]
    support={'support_material':'1','support_material_auto':'1','support_material_style':'snug','dont_support_bridges':'0'}
    write(root,'support',roof,obj=support)
    write(root,'blocker',roof+[('SupportBlocker',box((-1,-1,0),(10,21,11)),{})],obj=support)
    write(root,'enforcer',roof+[('SupportEnforcer',box((10,-1,0),(21,21,11)),{})],obj=dict(support,support_material_auto='0'))

if __name__=='__main__': main(pathlib.Path(sys.argv[1]))
