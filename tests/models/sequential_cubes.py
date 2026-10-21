#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Write a 3MF with several 20 mm cubes spread over the bed, for sequential (object by
object) printing. Prusa flavour by default (Slic3r_PE_model.config), Orca with --orca.

Usage: python3 tests/models/sequential_cubes.py out.3mf [--orca] [--at x,y ...] [--height h,...]
       [--tools t,...]
"""
import argparse
import pathlib
import zipfile

# 95 mm apart: clear of the CORE One and XL gantry models and of the U1's 72.5 mm radius.
DEFAULT_POSITIONS = [(40, 110), (135, 110), (230, 110)]
# The MINI's 180 mm bed: 60 mm apart, printed right to left. The MINI's head reaches 43 mm to
# the right of the nozzle but only 17 mm to the left (geometries.json), so left to right would
# crash into the previous cube.
SMALL_POSITIONS = [(150, 90), (90, 90), (30, 90)]


def cube(size_z):
    v = [(0, 0, 0), (20, 0, 0), (20, 20, 0), (0, 20, 0),
         (0, 0, size_z), (20, 0, size_z), (20, 20, size_z), (0, 20, size_z)]
    # Centred on the origin in XY so build transforms are object centres.
    v = [(x - 10, y - 10, z) for x, y, z in v]
    f = [(0, 2, 1), (0, 3, 2), (4, 5, 6), (4, 6, 7), (0, 1, 5), (0, 5, 4),
         (1, 2, 6), (1, 6, 5), (2, 3, 7), (2, 7, 6), (3, 0, 4), (3, 4, 7)]
    return v, f


class Deterministic:
    """Fixed timestamps: the same input gives the same bytes (corpus references hash it)."""

    def __init__(self, archive):
        self.archive = archive

    def writestr(self, name, data):
        info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
        self.archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED)


def write(path, positions, heights=None, orca=False, tools=None):
    path = pathlib.Path(path)
    heights = heights or [20.] * len(positions)
    tools = tools or [1] * len(positions)
    resources, build, config, instances = [], [], ['<?xml version="1.0" encoding="UTF-8"?><config>'], []
    for i, ((x, y), h, tool) in enumerate(zip(positions, heights, tools), 1):
        vertices, faces = cube(h)
        vertex_xml = ''.join(f'<vertex x="{a}" y="{b}" z="{c}"/>' for a, b, c in vertices)
        face_xml = ''.join(f'<triangle v1="{a}" v2="{b}" v3="{c}"/>' for a, b, c in faces)
        resources.append(
            f'<object id="{i}" type="model" name="Cube {i}"><mesh>'
            f'<vertices>{vertex_xml}</vertices><triangles>{face_xml}</triangles></mesh></object>')
        build.append(f'<item objectid="{i}" transform="1 0 0 0 1 0 0 0 1 {x} {y} 0" printable="1"/>')
        if orca:
            config.append(
                f'<object id="{i}"><metadata key="name" value="Cube {i}"/>'
                f'<metadata key="extruder" value="{tool}"/>'
                f'<part id="{i + 100}" subtype="normal_part" firstid="0" lastid="{len(faces) - 1}">'
                f'<metadata key="name" value="Cube {i}"/></part></object>')
            instances.append(
                f'<model_instance><metadata key="object_id" value="{i}"/>'
                '<metadata key="instance_id" value="0"/></model_instance>')
        else:
            config.append(
                f'<object id="{i}" instances_count="1">'
                f'<metadata type="object" key="name" value="Cube {i}"/>'
                f'<metadata type="object" key="extruder" value="{tool}"/>'
                f'<volume firstid="0" lastid="{len(faces) - 1}">'
                f'<metadata type="volume" key="name" value="Cube {i}"/></volume></object>')
    if orca:
        config.append('<plate><metadata key="plater_id" value="1"/>' + ''.join(instances) + '</plate>')
    config.append('</config>')
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as zipped:
        archive = Deterministic(zipped)
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


def generate(path):
    write(path, DEFAULT_POSITIONS)


def generate_small(path):
    write(path, SMALL_POSITIONS)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('output')
    parser.add_argument('--orca', action='store_true')
    parser.add_argument('--at', action='append', help='object centre x,y (repeat per cube)')
    parser.add_argument('--height', help='comma-separated cube heights')
    parser.add_argument('--tools', help='comma-separated one-based tool per cube')
    args = parser.parse_args()
    positions = ([tuple(float(v) for v in p.split(',')) for p in args.at]
                 if args.at else DEFAULT_POSITIONS)
    heights = [float(h) for h in args.height.split(',')] if args.height else None
    tools = [int(t) for t in args.tools.split(',')] if args.tools else None
    write(args.output, positions, heights, args.orca, tools)


if __name__ == '__main__':
    main()
