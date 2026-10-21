# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: adapted
# Source: orca@8500fcdcca src/libslic3r/PrintConfig.cpp :: PrintConfigDef
# Source: orca@8500fcdcca src/libslic3r/PrintConfig.hpp :: FullPrintConfig
# Source: orca@8500fcdcca src/libslic3r/Preset.cpp :: s_Preset_*_options
# Source: snapmaker-orca@b1831e5dcb src/libslic3r/PrintConfig.cpp :: PrintConfigDef
# Source: snapmaker-orca@b1831e5dcb src/libslic3r/PrintConfig.hpp :: FullPrintConfig
# Source: snapmaker-orca@b1831e5dcb src/libslic3r/Preset.cpp :: s_Preset_*_options
# Changes: Extract declarative FFF option data; fail on unrecognized default expressions.
"""python3 generate_orca_schema.py /path/to/upstream (standard library only).

This is a deliberately pinned-source extractor, not a general C++ parser.
The output records data only; Mica's runtime does not need either checkout.
"""
import json
import pathlib
import re
import sys


def uncomment(text):
    return re.sub(
        r'("(?:\\.|[^"\\])*"|/\*[\s\S]*?\*/|//[^\n]*)',
        lambda m: m[0] if m[0].startswith('"') else '', text,
    )


def strings(text):
    return [json.loads(x) for x in re.findall(r'"(?:\\.|[^"\\])*"', text)]


def number(text):
    return format(float(text.strip().rstrip('f')), '.15g')


def default_value(ty, constructor, enum, args, block, enum_maps, constants):
    args = args.strip()[1:-1].strip()  # Constructor parentheses or braces.
    if ty in ('Enum', 'Enums'):
        if not enum:
            found = re.search(r'def->enum_keys_map\s*=\s*&s_keys_map_(\w+)', block)
            enum = (found[1] if found else
                    re.search(r'ConfigOptionEnum<(\w+)>::get_enum_values', block)[1])
        symbol = args.replace('(int)', '').replace(' ', '').strip('{}').split('::')[-1]
        value = enum_maps[enum][symbol]
        return ([value] if ty == 'Enums' else value), enum
    if ty in ('String', 'Strings'):
        values = strings(args)
        if args and not values:
            raise ValueError(f'non-literal string: {args}')
        return (values if ty == 'Strings' else ''.join(values)), None
    if ty in ('Point', 'Points', 'PointsGroups'):
        points = re.findall(r'Vec2[di]\(([^)]+)\)', args)
        values = ['x'.join(number(v) for v in p.split(',')) for p in points]
        if ty == 'Point':
            return (values[0].replace('x', ',') if values else '0,0'), None
        if args and not points:
            raise ValueError(f'non-literal points: {args}')
        return values, None

    initializer_list = constructor.strip().startswith('{')
    args = args.strip('{} ')
    args = re.sub(r'ConfigOption\w+::nil_value\(\)', 'nil', args)
    args = args.removeprefix('FloatOrPercent{').rstrip('}')
    percent = False
    if ty in ('FloatOrPercent', 'FloatsOrPercents'):
        args, flag = args.rsplit(',', 1)
        percent = flag.strip() in ('true', '1')
    args = re.sub(r'std::numeric_limits<[^>]+>::quiet_NaN\(\)', 'nil', args)
    args = args.replace('NaN', 'nil').replace('true', '1').replace('false', '0')
    for key, value in constants.items():
        args = re.sub(r'\b' + key + r'\b', value, args)
    suffix = '%' if ty in ('Percent', 'Percents') or percent else ''
    values = [v.strip() if v.strip() == 'nil' else number(v) + suffix
              for v in args.split(',')] if args else []
    vector = ty.endswith('s')
    # ConfigOptionFloats(1, default) repeats a value; brace initializers do not.
    if vector and ty != 'FloatsOrPercents' and not initializer_list and len(values) == 2:
        values = [values[1]] * int(values[0])
    return (values if vector else values[0] if values else '0'), None


def extract(root, repo):
    source = root / repo / 'src/libslic3r'
    cpp = uncomment((source / 'PrintConfig.cpp').read_text())
    header = (source / 'PrintConfig.hpp').read_text()
    hpp = uncomment(header)
    preset = uncomment((source / 'Preset.cpp').read_text())
    for name, value in re.findall(r'constexpr const char\*\s*(\w+)\s*=\s*("[^"]*")', header):
        cpp = re.sub(r'\b' + name + r'\b', value, cpp)

    sets = {name: set(strings(body)) for name, body in re.findall(
        r'(s_Preset_\w+_options|m_extruder_option_keys)\s*(?:=\s*)?\{(.*?)\};',
        preset + cpp, re.S)}
    scopes = {}
    for scope in ('print', 'filament', 'printer', 'machine_limits'):
        group = {'printer': 'machine', 'machine_limits': 'machine', 'print': 'process'}.get(scope, scope)
        for key in sets.get('s_Preset_' + scope + '_options', ()):
            scopes.setdefault(key, []).append(group)
    extruder_keys = sets['m_extruder_option_keys']
    for key in extruder_keys:
        scopes.setdefault(key, []).append('machine')
    static_fields = set(re.findall(r'\(\(ConfigOption[^,]+,\s*(\w+)\)\)', hpp))
    enum_maps = {
        name: {symbol.replace(' ', '').split('::')[-1]: value
               for value, symbol in re.findall(
                   r'\{\s*"([^"]+)"\s*,\s*(?:int\()?([\w: ]+)\)?\s*\}', body)}
        for name, body in re.findall(r's_keys_map_(\w+)\s*(?:=\s*)?\{(.*?)\};', cpp, re.S)
    }
    constants_header = source / 'PrintConfigConstants.hpp'
    constants_text = header + (constants_header.read_text() if constants_header.exists() else '')
    constants = dict(re.findall(r'#define\s+(\w+)\s+([\d.]+|true|false)', constants_text))
    constants = {k: {'true': '1', 'false': '0'}.get(v, v) for k, v in constants.items()}
    starts = list(re.finditer(r'(?:this->)?add\("([^"]+)",\s*co(\w+)(?:,\s*(true|false))?\)', cpp))
    result, missing = {}, []
    for i, match in enumerate(starts):
        key, ty, nullable = match.groups()
        if match.start() > cpp.index('void PrintConfigDef::init_sla_params()'):
            continue
        block = cpp[match.end(): starts[i + 1].start() if i + 1 < len(starts) else len(cpp)]
        default = re.search(r'set_default_value\(new ConfigOption(\w+)(?:<(\w+)>)?\s*(.*?)\);', block, re.S)
        if not default and key == 'extruder':
            continue  # Dynamic object selector, no static FFF default.
        if not default:
            missing.append((key, 'no constructor'))
            continue
        cls, enum, args = default.groups()
        try:
            value, enum = default_value(ty, args, enum, args, block, enum_maps, constants)
            entry = {
                'type': ty, 'default': value, 'scope': sorted(set(scopes.get(key, []))),
                'full': key in static_fields, 'extruder': key in extruder_keys,
                'nullable': nullable == 'true' or 'Nullable' in cls,
            }
            if enum:
                entry['enum'] = list(enum_maps[enum].values())
            for prop in ('ratio_over', 'min', 'max'):
                matches = list(re.finditer(r'def->' + prop + r'\s*=\s*([^;]+);', block))
                if matches:
                    val = matches[-1][1].strip()  # Last assignment wins in C++.
                    if prop == 'ratio_over' and val.startswith('"'):
                        entry['ratio'] = json.loads(val)
                    elif re.fullmatch(r'-?[\d.]+', val):
                        entry[prop] = float(val)
            result[key] = entry
        except (ValueError, KeyError, IndexError, TypeError) as error:
            missing.append((key, args, str(error)))

    overrides = re.search(r'filament_extruder_override_keys\s*=\s*\{(.*?)\};', cpp, re.S)
    if overrides:
        overrides = strings(overrides[1])
    else:
        # Snapmaker declares the same data inline in the nullable-override loop.
        section = cpp[cpp.index('def = this->add("min_bead_width"'):]
        overrides = ['filament_' + key for key in strings(
            re.search(r'for \(const char \*opt_key : \{(.*?)\}\) \{', section, re.S)[1])]
    for key in overrides:
        result[key] = {**result[key.removeprefix('filament_')], 'nullable': True,
                       'scope': ['filament'], 'extruder': False, 'full': key in static_fields}
    for axis, speed, acceleration, jerk in re.findall(
            r'\{\s*"([xyze])",\s*\{([^}]+)\},\s*\{([^}]+)\},\s*\{([^}]+)\}\s*\}', cpp):
        for stem, values in [('speed', speed), ('acceleration', acceleration), ('jerk', jerk)]:
            result['machine_max_' + stem + '_' + axis] = {
                'type': 'Floats', 'default': [number(v) for v in values.split(',')],
                'scope': ['machine'], 'full': True, 'extruder': False, 'nullable': False, 'min': 0,
            }
    if missing:
        raise ValueError((repo, missing))
    print(repo, len(result))
    return dict(sorted(result.items()))


if __name__ == '__main__':
    for repo in ('orca', 'snapmaker-orca'):
        data = extract(pathlib.Path(sys.argv[1]), repo)
        path = pathlib.Path(__file__).resolve().parents[1] / 'src/orca' / f'{repo}_schema.json'
        # One option per line keeps generated-data reviews manageable.
        path.write_text('{\n' + ',\n'.join(
            '  ' + json.dumps(k) + ': ' + json.dumps(v, ensure_ascii=False)
            for k, v in data.items()) + '\n}\n')
