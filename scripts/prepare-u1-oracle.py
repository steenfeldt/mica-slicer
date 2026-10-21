#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Prepare single-tool U1 CLI inputs only; never launch Orca or overwrite source presets.

Resolve parents within the selected vendor plus OrcaFilamentLibrary. Base names such as
fdm_filament_pla are reused by many vendors and cannot be indexed globally by name.
"""
import argparse
import json
import pathlib
import tempfile


def prepare(profiles, output, machine='Snapmaker U1 (0.4 nozzle)',
            process='0.20 Standard @Snapmaker U1 (0.4 nozzle)', filament='Snapmaker PLA @U1'):
    index = {}
    for vendor in ('OrcaFilamentLibrary', 'Snapmaker'):
        manifest = json.loads((profiles / (vendor + '.json')).read_text())
        for kind in ('machine', 'process', 'filament'):
            for item in manifest.get(kind + '_list', []):
                index[kind, item['name']] = profiles / vendor / item['sub_path']

    def flatten(kind, name, seen=()):
        if name in seen:
            raise ValueError('cyclic inheritance: ' + name)
        data = json.loads(index[kind, name].read_text())
        result = flatten(kind, data['inherits'], (*seen, name)) if data.get('inherits') else {}
        result.update(data)
        result.pop('inherits', None)
        return result

    output.mkdir(parents=True, exist_ok=True)
    for kind, name in [('machine', machine), ('process', process), ('filament', filament)]:
        data = flatten(kind, name)
        for key in ('description', 'setting_id', 'filament_id'):
            data.pop(key, None)
        if kind == 'machine':
            data['curr_bed_type'] = 'High Temp Plate'
            if len(data['nozzle_diameter']) != 4:
                raise ValueError('U1 oracle requires four explicit machine slots')
        (output / (kind + '.json')).write_text(json.dumps(data, indent=2) + '\n')
    return output


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profiles', type=pathlib.Path,
                        default=pathlib.Path('/Applications/OrcaSlicer.app/Contents/Resources/profiles'))
    parser.add_argument('--output', type=pathlib.Path)
    parser.add_argument('--machine', default='Snapmaker U1 (0.4 nozzle)')
    parser.add_argument('--process', default='0.20 Standard @Snapmaker U1 (0.4 nozzle)')
    parser.add_argument('--filament', default='Snapmaker PLA @U1')
    args = parser.parse_args()
    output = args.output or pathlib.Path(tempfile.mkdtemp(prefix='mica-u1-oracle-'))
    print(prepare(args.profiles, output, args.machine, args.process, args.filament))
