#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate original painted 3MF oracle projects in a supplied temporary directory."""
import importlib.util
import math
import pathlib
import re
import sys
import zipfile

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("regions", pathlib.Path(__file__).with_name("make-regions-oracle.py"))
regions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(regions)


def annotate(path, annotations, orca=False):
    with zipfile.ZipFile(path) as z:
        entries = {name: z.read(name) for name in z.namelist()}
    model = entries["3D/3dmodel.model"].decode()
    model = model.replace('<model unit=', '<model xmlns:slic3rpe="http://schemas.slic3r.org/3mf/2017/06" unit=')
    index = -1

    def triangle(match):
        nonlocal index
        index += 1
        attrs = annotations.get(index, {})
        return match[0][:-2] + ''.join(f' {key}="{value}"' for key, value in attrs.items()) + '/>'

    model = re.sub(r'<triangle\s[^>]+/>', triangle, model)
    versions = ''.join(f'<metadata name="slic3rpe:{name}PaintingVersion">1</metadata>'
                       for name in ['Mm', 'Seam', 'FdmSupports', 'FuzzySkin'])
    model = model.replace('<resources>', versions + '<resources>')
    if orca:
        for a, b in [('slic3rpe:mmu_segmentation', 'paint_color'), ('slic3rpe:custom_supports', 'paint_supports'),
                     ('slic3rpe:custom_seam', 'paint_seam'), ('slic3rpe:fuzzy_skin', 'paint_fuzzy_skin')]:
            model = model.replace(a + '=', b + '=')
    entries['3D/3dmodel.model'] = model.encode()
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, data in entries.items():
            z.writestr(name, data)


def main(root):
    root.mkdir(parents=True, exist_ok=True)
    cube = ('ModelPart', regions.box((0, 0, 0), (20, 20, 20)), {})
    for name, extra in [('colour', {}), ('colour-width', {'mmu_segmented_region_max_width': '2',
                                                       'mmu_segmented_region_interlocking_depth': '1'}),
                        ('colour-beam', {'interlocking_beam': '1'})]:
        regions.write(root, name, [cube], obj=extra)
        annotate(root / (name + '.3mf'), {i: {'slic3rpe:mmu_segmentation': '8'} for i in range(6, 10)})
    for name, shape in [('colour-split', cube),
                        ('colour-split-asymmetric', ('ModelPart', regions.box((0, 0, 0), (23, 19, 20)), {}))]:
        regions.write(root, name, [shape])
        annotate(root / (name + '.3mf'), {
            6: {'slic3rpe:mmu_segmentation': '081'},
            7: {'slic3rpe:mmu_segmentation': '0802'},
            8: {'slic3rpe:mmu_segmentation': '08083'},
        })
    regions.write(root, 'fuzzy', [cube])
    annotate(root / 'fuzzy.3mf', {i: {'slic3rpe:fuzzy_skin': '4'} for i in [6, 7]})
    roof = [('ModelPart', regions.box((8, 8, 0), (12, 12, 8)), {}),
            ('ModelPart', regions.box((0, 0, 8), (10, 20, 10)), {}),
            ('ModelPart', regions.box((10, 0, 8), (20, 20, 10)), {})]
    support = {'support_material': '1', 'support_material_auto': '1',
               'support_material_style': 'snug', 'dont_support_bridges': '0'}
    for name, auto, attrs in [('support', '1', {}),
                              ('blocker', '1', {i: {'slic3rpe:custom_supports': '8'} for i in [12, 13]}),
                              ('enforcer', '0', {i: {'slic3rpe:custom_supports': '4'} for i in [24, 25]})]:
        regions.write(root, name, roof, obj=dict(support, support_material_auto=auto))
        annotate(root / (name + '.3mf'), attrs)
    n = 96
    vertices = [(10 + 10 * math.cos(2 * math.pi * i / n), 10 + 10 * math.sin(2 * math.pi * i / n), z)
                for z in [0, 20] for i in range(n)] + [(10, 10, 0), (10, 10, 20)]
    faces, attrs = [], {}
    for i in range(n):
        j = (i + 1) % n
        start = len(faces)
        faces.extend([(i, j, n + j), (i, n + j, n + i), (2 * n, j, i), (2 * n + 1, n + i, n + j)])
        if i in [0, 1, n - 2, n - 1]:
            for k in [start, start + 1]:
                attrs[k] = {'slic3rpe:custom_seam': '4'}
    regions.write(root, 'seam', [('ModelPart', (vertices, faces), {})], obj={'seam_position': 'rear'})
    annotate(root / 'seam.3mf', attrs)


if __name__ == '__main__':
    main(pathlib.Path(sys.argv[1]))
