#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Generate inputs only; never launch slicers. See docs/wipe-tower.md."""
import argparse
import json
import pathlib
import struct
import tempfile
import zipfile


# cube20 is ASCII; this synthetic mesh has the same 20 mm dimensions.
CUBE = (
    [(0, 0, 0), (20, 0, 0), (20, 20, 0), (0, 20, 0),
     (0, 0, 20), (20, 0, 20), (20, 20, 20), (0, 20, 20)],
    [(0, 2, 1), (0, 3, 2), (4, 5, 6), (4, 6, 7), (0, 1, 5), (0, 5, 4),
     (1, 2, 6), (1, 6, 5), (2, 3, 7), (2, 7, 6), (3, 0, 4), (3, 4, 7)],
)


def mesh(path):
    data = path.read_bytes()
    count = struct.unpack_from('<I', data, 80)[0]
    vertices, faces, indices = [], [], {}
    for i in range(count):
        triangle = []
        for j in range(3):
            vertex = struct.unpack_from('<3f', data, 84 + i * 50 + 12 + j * 12)
            if vertex not in indices:
                indices[vertex] = len(vertices)
                vertices.append(vertex)
            triangle.append(indices[vertex])
        faces.append(triangle)
    return vertices, faces


def project(root, name, tools, geometry, orca):
    vertices, faces = geometry
    if 'benchys' in name:
        # The repository's CC0 Benchy already has this XY translation baked in.
        vertices = [(x - 125, y - 105, z) for x, y, z in vertices]
    resources, build, config, instances = [], [], ['<config>'], []
    vertex_xml = ''.join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x, y, z in vertices)
    face_xml = ''.join(f'<triangle v1="{a}" v2="{b}" v3="{c}"/>' for a, b, c in faces)
    for i, tool in enumerate(tools, 1):
        resources.append(
            f'<object id="{i}" type="model" name="Model {i}"><mesh>'
            f'<vertices>{vertex_xml}</vertices><triangles>{face_xml}</triangles></mesh></object>')
        x, y = ((100 + i * 35, 110) if 'cubes' in name
                else [(85, 80), (180, 80), (85, 170), (180, 170)][i - 1])
        build.append(f'<item objectid="{i}" transform="1 0 0 0 1 0 0 0 1 {x} {y} 0"/>')
        if orca:
            config.append(
                f'<object id="{i}"><metadata key="name" value="Model {i}"/>'
                f'<metadata key="extruder" value="{tool}"/>'
                f'<part id="{i + 100}" subtype="normal_part" firstid="0" lastid="{len(faces) - 1}">'
                f'<metadata key="name" value="Model {i}"/></part></object>')
            instances.append(
                f'<model_instance><metadata key="object_id" value="{i}"/>'
                '<metadata key="instance_id" value="0"/></model_instance>')
        else:
            config.append(
                f'<object id="{i}" instances_count="1">'
                f'<metadata type="object" key="name" value="Model {i}"/>'
                f'<metadata type="object" key="extruder" value="{tool}"/>'
                f'<volume firstid="0" lastid="{len(faces) - 1}">'
                f'<metadata type="volume" key="name" value="Model {i}"/></volume></object>')
    if orca:
        config.append('<plate><metadata key="plater_id" value="1"/>' + ''.join(instances) + '</plate>')
    config.append('</config>')
    with zipfile.ZipFile(root / (name + '.3mf'), 'w', zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(
            '[Content_Types].xml',
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>')
        archive.writestr(
            '_rels/.rels',
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Target="/3D/3dmodel.model" Id="rel0" '
            'Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>')
        archive.writestr(
            '3D/3dmodel.model',
            '<?xml version="1.0"?><model unit="millimeter" '
            'xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">'
            '<metadata name="slic3rpe:Version3mf">1</metadata><resources>' + ''.join(resources)
            + '</resources><build>' + ''.join(build) + '</build></model>')
        archive.writestr('Metadata/model_settings.config' if orca
                         else 'Metadata/Slic3r_PE_model.config', ''.join(config))


def profiles(root, profiles_dir):
    # Resolve parents within the selected vendor plus OrcaFilamentLibrary, as Orca does.
    # Base names such as fdm_filament_pla/fdm_filament_common exist in many vendors with
    # different values (Snapmaker: 60 mm/s retraction, 14 mm³/s, 0 mm³ minimal purge);
    # a global name index picks an arbitrary vendor's copy.
    index = {}
    for vendor in ('OrcaFilamentLibrary', 'Snapmaker'):
        manifest = json.loads((profiles_dir / (vendor + '.json')).read_text())
        for kind in ('machine', 'process', 'filament'):
            for item in manifest.get(kind + '_list', []):
                index[kind, item['name']] = profiles_dir / vendor / item['sub_path']

    def flatten(kind, name):
        data = json.loads(index[kind, name].read_text())
        result = flatten(kind, data['inherits']) if data.get('inherits') else {}
        result.update(data)
        result.pop('inherits', None)
        return result

    presets = [('machine', 'Snapmaker U1 (0.4 nozzle)'),
               ('process', '0.20 Standard @Snapmaker U1 (0.4 nozzle)'),
               ('filament', 'Snapmaker PLA @U1')]
    flattened = {kind: flatten(kind, name) for kind, name in presets}
    explicit = set().union(*(data.keys() for data in flattened.values()))
    schema_path = pathlib.Path(__file__).resolve().parents[1] / 'crates/mica-profiles/src/orca/orca_schema.json'
    schema = json.loads(schema_path.read_text())
    for kind, name in presets:
        data = dict(flattened[kind])
        if kind in ['machine', 'process']:
            data.update(default_filament_profile=['Snapmaker PLA'] * 4,
                        default_filament_colour=[''] * 4,
                        filament_colour=['#FF0000', '#00FF00', '#0000FF', '#FFFFFF'],
                        filament_map=['1', '2', '3', '4'], filament_map_mode='Manual')
        if kind == 'process':
            # Orca validates some default vectors before it expands the filament presets.
            # Supplying only colours/mapping still leaves e.g. filament_is_support at size 1.
            # Read existing pinned schema data; never copy vendor data into the repository.
            # Extruder-variant vectors are sized by Orca itself; four-entry copies abort it
            # (ConfigOptionVector::set_with_restore_2(): invalid size found).
            variant = {'default_nozzle_volume_type', 'extruder_variant_list',
                       'filament_extruder_variant', 'filament_self_index'}
            for key, definition in schema.items():
                default = definition.get('default')
                if (key not in explicit and key not in variant and isinstance(default, list)
                        and len(default) == 1):
                    data.setdefault(key, default * 4)
            data['physical_extruder_map'] = ['0', '1', '2', '3']
            data.update(enable_prime_tower='1', curr_bed_type='High Temp Plate',
                        flush_volumes_matrix=['0' if i == j else '100'
                                              for i in range(4) for j in range(4)],
                        flush_volumes_vector=['140'] * 8,
                        wipe_tower_x=['15'], wipe_tower_y=['220'])
        if kind == 'filament':
            for i in range(1, 5):
                # Distinct preset IDs prevent the CLI from merging physical filament slots.
                slot = dict(data, name=f'Mica oracle PLA {i}',
                            filament_settings_id=[f'Mica oracle PLA {i}'],
                            filament_id=f'MICA_ORACLE_{i}')
                (root / f'filament{i}.json').write_text(json.dumps(slot))
        else:
            (root / f'{kind}.json').write_text(json.dumps(data))
    (root / 'xl-matrix.ini').write_text('wiping_volumes_matrix = '
        + ','.join('0' if i == j else '140' for i in range(5) for j in range(5)) + '\n')


def main():
    parser = argparse.ArgumentParser(
        description='Create tower projects and explicit four-slot Orca profiles in a temporary directory.')
    parser.add_argument('--output', type=pathlib.Path)
    parser.add_argument('--plate', action='store_true',
                        help='Three XL cubes (1/3/5), two U1 cubes (1/3), and two single-tool CORE cubes.')
    parser.add_argument('--orca-profiles', type=pathlib.Path,
                        default=pathlib.Path('/Applications/OrcaSlicer.app/Contents/Resources/profiles'))
    args = parser.parse_args()
    root = args.output or pathlib.Path(tempfile.mkdtemp(prefix='mica-wipe-tower-'))
    root.mkdir(parents=True, exist_ok=True)
    repo = pathlib.Path(__file__).resolve().parents[1]
    benchy = mesh(repo / 'tests/models/3DBenchy.stl')
    cases = [
        ('xl-cubes', [1, 2], CUBE, False), ('xl-benchys', [1, 3, 5], benchy, False),
        ('u1-cubes', [1, 2, 3, 4], CUBE, True), ('u1-benchys', [1, 2, 3, 4], benchy, True),
    ] if not args.plate else [
        ('xl-cubes', [1, 3, 5], CUBE, False), ('u1-cubes', [1, 3], CUBE, True),
        ('core-cubes', [1, 1], CUBE, False),
    ]
    for name, tools, geometry, orca in cases:
        project(root, name, tools, geometry, orca)
    profiles(root, args.orca_profiles)
    if args.plate:
        with (root / 'xl-matrix.ini').open('a') as out:
            out.write('skirts = 3\nskirt_distance = 3\nbrim_width = 3\n')
    print(root)


if __name__ == '__main__':
    main()
