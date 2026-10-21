#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Deterministic synthetic ColorMix projects. Optional full config comes from the local oracle,
never from a user's project. python3 tests/models/colormix.py DIRECTORY [--config oracle.ini]
"""
import argparse
import json
from pathlib import Path
import zipfile
from sequential_cubes import cube, Deterministic

CASES = ('mix50', 'mix75', 'mix33', 'painted_band', 'mixed_plain', 'adjacent')


def generate(directory, config=None):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    for case in CASES:
        ratios = [0.75, 0.25] if case == 'mix75' else ([1/3]*3 if case == 'mix33' else [0.5, 0.5])
        fs = {'version': 1, 'physical_extruders': [{'id': i, 'color': '#808080'} for i in range(1, 6)],
              'virtual_extruders': [{'id': 6, 'kind': 'fullspectrum', 'color': '#888888',
                                     'components': [{'extruder': i+1, 'ratio': r} for i, r in enumerate(ratios)]}]}
        objects = [[6]]
        if case == 'mixed_plain': objects = [[6], [4]]
        if case == 'adjacent': objects = [[6, 1]]
        resources, build, metadata = [], [], []
        for oi, parts in enumerate(objects, 1):
            vertices, faces, paint, ranges = [], [], [], []
            for pi, tool in enumerate(parts):
                first = len(faces)
                # A horizontal 4.2..12.2 mm band on every side. Its phase starts at object layer 0.
                bands = [(0, 4.2), (4.2, 12.2), (12.2, 20)] if case == 'painted_band' else [(0, 20)]
                for lo, hi in bands:
                    vv, ff = cube(hi-lo)
                    start = len(vertices)
                    vertices.extend([(x + pi*20, y, z+lo) for x, y, z in vv])
                    # Coplanar internal caps are removed for the painted watertight band mesh.
                    for fi, f in enumerate(ff):
                        if case == 'painted_band' and ((fi < 2 and lo > 0) or (2 <= fi < 4 and hi < 20)):
                            continue
                        faces.append(tuple(start+v for v in f))
                        paint.append('3C' if case == 'painted_band' and lo == 4.2 else '')  # state 6
                ranges.append((first, len(faces)-1, 1 if case == 'painted_band' else tool))
            vx = ''.join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x,y,z in vertices)
            tx = ''.join(f'<triangle v1="{a}" v2="{b}" v3="{c}"'+(f' slic3rpe:mmu_segmentation="{p}"' if p else '')+'/>' for (a,b,c),p in zip(faces,paint))
            resources.append(f'<object id="{oi}" type="model" name="{case}-{oi}"><mesh><vertices>{vx}</vertices><triangles>{tx}</triangles></mesh></object>')
            build.append(f'<item objectid="{oi}" transform="1 0 0 0 1 0 0 0 1 {130+(oi-1)*40} 130 0"/>')
            volumes = ''.join(f'<volume firstid="{a}" lastid="{b}"><metadata type="volume" key="name" value="part-{i}"/><metadata type="volume" key="extruder" value="{t}"/></volume>' for i,(a,b,t) in enumerate(ranges))
            metadata.append(f'<object id="{oi}" instances_count="1">{volumes}</object>')
        with zipfile.ZipFile(directory/f'colormix_{case}.3mf', 'w') as z:
            archive = Deterministic(z)
            archive.writestr('[Content_Types].xml', '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/><Default Extension="json" ContentType="application/json"/></Types>')
            archive.writestr('_rels/.rels', '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>')
            archive.writestr('3D/3dmodel.model', '<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:slic3rpe="http://schemas.slic3r.org/3mf/2017/06"><metadata name="slic3rpe:Version3mf">1</metadata><resources>'+''.join(resources)+'</resources><build>'+''.join(build)+'</build></model>')
            archive.writestr('Metadata/Slic3r_PE_model.config', '<config>'+''.join(metadata)+'</config>')
            archive.writestr('Metadata/Prusa_Slicer_full_spectrum.json', json.dumps(fs, indent=2)+'\n')
            if config: archive.writestr('Metadata/Slic3r_PE.config', config)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--config', type=Path)
    args = parser.parse_args()
    generate(args.directory, args.config.read_text() if args.config else None)
