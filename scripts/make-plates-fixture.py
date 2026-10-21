#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Write tests/models/plates-orca.3mf: a small synthetic Orca/Snapmaker-style project with two
plates, four filaments with colours, per-object tools, colour painting and project settings.

Plate 1 ("" = unnamed) holds two cubes at Orca's global grid origin: object 1 (tool 1, its +X
and +Y faces colour-painted tool 2 and tool 3) and object 2 (tool 2). Plate 2 ("Lid", locked)
holds one cube one grid stride to the right (bed 270 mm, 20 % gap). A fourth cube is
unassigned and sits between the plates, like an object dragged off every plate in Orca.
The project settings name the Snapmaker U1 presets and change the infill density.

    python3 scripts/make-plates-fixture.py tests/models/plates-orca.3mf
"""
import json
import pathlib
import sys
import zipfile

sys.dont_write_bytecode = True

FACES = [(0, 2, 1), (0, 3, 2), (4, 5, 6), (4, 6, 7), (0, 1, 5), (0, 5, 4),
         (1, 2, 6), (1, 6, 5), (2, 3, 7), (2, 7, 6), (3, 0, 4), (3, 4, 7)]
# Faces 6, 7 are the +X side, faces 8, 9 the +Y side (see FACES: vertices 1, 2, 5, 6 have x = u).


def box(size):
    s = size
    return [(0, 0, 0), (s, 0, 0), (s, s, 0), (0, s, 0), (0, 0, s), (s, 0, s), (s, s, s), (0, s, s)]


def mesh(size, paint=None):
    verts = ''.join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x, y, z in box(size))
    tris = ''
    for i, (a, b, c) in enumerate(FACES):
        attrs = ''
        if paint and i in paint:
            attrs = f' paint_color="{paint[i]}"'
        tris += f'<triangle v1="{a}" v2="{b}" v3="{c}"{attrs}/>'
    return f'<mesh><vertices>{verts}</vertices><triangles>{tris}</triangles></mesh>'


def main(out):
    stride = 270 * 1.2
    # Nibble codes (hex digits reversed on the wire): 4 = state 1 (tool 1), 8 = state 2
    # (tool 2), 0C = extended state 3 (tool 3): code C then the extension nibble 0.
    paint = {6: '8', 7: '8', 8: '0C', 9: '0C'}
    objects = ''.join([
        f'<object id="1" type="model">{mesh(20, paint)}</object>',
        f'<object id="2" type="model">{mesh(15)}</object>',
        f'<object id="3" type="model">{mesh(25)}</object>',
        f'<object id="4" type="model">{mesh(10)}</object>',
    ])
    items = ''.join([
        '<item objectid="1" transform="1 0 0 0 1 0 0 0 1 60 60 0" printable="1"/>',
        '<item objectid="2" transform="1 0 0 0 1 0 0 0 1 150 120 0" printable="1"/>',
        f'<item objectid="3" transform="1 0 0 0 1 0 0 0 1 {stride + 100} 100 0" printable="1"/>',
        f'<item objectid="4" transform="1 0 0 0 1 0 0 0 1 {stride - 20} 300 0" printable="1"/>',
    ])
    model = ('<?xml version="1.0" encoding="UTF-8"?>'
             '<model unit="millimeter" xml:lang="en-US" '
             'xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" '
             'xmlns:BambuStudio="http://schemas.bambulab.com/package/2021">'
             '<metadata name="Application">OrcaSlicer-2.4.1</metadata>'
             f'<resources>{objects}</resources><build>{items}</build></model>')

    def obj(id_, name, extruder):
        return (f'<object id="{id_}"><metadata key="name" value="{name}"/>'
                f'<metadata key="extruder" value="{extruder}"/>'
                f'<part id="{id_}" subtype="normal_part"><metadata key="name" value="{name}"/>'
                f'<metadata key="matrix" value="1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1"/></part></object>')

    def plate(id_, name, locked, instances):
        inst = ''.join(f'<model_instance><metadata key="object_id" value="{o}"/>'
                       f'<metadata key="instance_id" value="0"/></model_instance>' for o in instances)
        return (f'<plate><metadata key="plater_id" value="{id_}"/>'
                f'<metadata key="plater_name" value="{name}"/>'
                f'<metadata key="locked" value="{locked}"/>'
                f'<metadata key="filament_maps" value="1 2 3 4"/>{inst}</plate>')

    settings = ('<?xml version="1.0" encoding="UTF-8"?><config>'
                + obj(1, 'painted', 1) + obj(2, 'second', 2) + obj(3, 'lid', 3) + obj(4, 'loose', 4)
                + plate(1, '', 'false', [1, 2]) + plate(2, 'Lid', 'true', [3]) + '</config>')
    project = {
        'printer_settings_id': 'Snapmaker U1 (0.4 nozzle)',
        'printer_model': 'Snapmaker U1',
        'printer_variant': '0.4',
        'print_settings_id': '0.20 Standard @Snapmaker U1 (0.4 nozzle)',
        'filament_settings_id': ['Generic PLA @System', 'Generic PLA @System',
                                 'Generic PETG @System', 'Generic PLA @System'],
        'filament_colour': ['#CB8F29', '#000000', '#FFFFFF', '#FB0207'],
        'filament_type': ['PLA', 'PLA', 'PETG', 'PLA'],
        'nozzle_diameter': ['0.4', '0.4', '0.4', '0.4'],
        'printable_area': ['0.5x1', '270.5x1', '270.5x271', '0.5x271'],
        'printable_height': '270',
        'different_settings_to_system': ['sparse_infill_density', '', '', '', '', ''],
        'sparse_infill_density': '8%',
        'wipe_tower_x': ['165', '165'],
        'wipe_tower_y': ['233', '233'],
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
        z.writestr('[Content_Types].xml',
                   '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                   '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                   '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>')
        z.writestr('_rels/.rels',
                   '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                   '<Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>')
        z.writestr('3D/3dmodel.model', model)
        z.writestr('Metadata/model_settings.config', settings)
        z.writestr('Metadata/project_settings.config', json.dumps(project, indent=2))


if __name__ == '__main__':
    main(pathlib.Path(sys.argv[1]))
