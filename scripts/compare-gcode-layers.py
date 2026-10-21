#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare wall filament, length and continuous extrusion runs on every layer.

Counts G1/G2/G3, relative/absolute E, and width splits without counting a
continuous run twice. Run count is observable G-code continuity, not inset depth.
Usage: compare-gcode-layers.py reference.gcode mica.gcode [--csv result.csv]
"""
import argparse
import collections
import csv
import math
import re

WALLS = ("External perimeter", "Perimeter", "Gap fill", "Overhang perimeter")


def parse(path):
    layers = []
    role = None
    x = y = None
    e = 0.
    relative = False
    active = False
    for line in open(path):
        if line.startswith(";LAYER_CHANGE"):
            layers.append({"z": None, "roles": collections.defaultdict(lambda: [0., 0., 0])})
            active = False
        elif line.startswith(";Z:") and layers:
            layers[-1]["z"] = float(line[3:])
        elif line.startswith(";TYPE:"):
            new = line[6:].strip()
            if new != role:
                active = False
            role = new
        code = line.split(";")[0].split()
        if not code:
            continue
        command = code[0]
        if command == "M83":
            relative = True
        elif command == "M82":
            relative = False
        values = {k: float(v) for k, v in re.findall(r"([XYZEIJ])([-+\d.eE]+)", " ".join(code[1:]))}
        if command == "G92":
            e = values.get("E", e)
            continue
        if command not in ("G0", "G1", "G2", "G3"):
            continue
        nx, ny = values.get("X", x), values.get("Y", y)
        de = (values["E"] if relative else values["E"]-e) if "E" in values else 0.
        if "E" in values:
            e = e + de
        moving = ("X" in values or "Y" in values) and x is not None and y is not None
        if moving and de > 0. and layers:
            distance = math.hypot(nx-x, ny-y)
            if command in ("G2", "G3"):
                cx, cy = x+values.get("I", 0.), y+values.get("J", 0.)
                radius = math.hypot(x-cx, y-cy)
                a, b = math.atan2(y-cy, x-cx), math.atan2(ny-cy, nx-cx)
                sweep = ((b-a) if command == "G3" else (a-b)) % math.tau
                distance = radius * (sweep if sweep > 1e-9 else math.tau)
            stats = layers[-1]["roles"][role]
            stats[0] += de
            stats[1] += distance
            stats[2] += int(not active)
            active = True
        elif moving or de < 0.:
            active = False
        x, y = nx, ny
    return layers


def compare(reference, mica):
    if len(reference) != len(mica):
        raise ValueError(f"Layer counts differ: {len(reference)} vs {len(mica)}")
    rows = []
    for i, (a, b) in enumerate(zip(reference, mica), 1):
        if abs(a["z"]-b["z"]) > 1e-4:
            raise ValueError(f"Layer {i}: Z differs: {a['z']} vs {b['z']}")
        for role in (*WALLS, "All walls"):
            roles = WALLS if role == "All walls" else (role,)
            av = [sum(a["roles"][r][j] for r in roles) for j in range(3)]
            bv = [sum(b["roles"][r][j] for r in roles) for j in range(3)]
            rows.append(dict(layer=i, z=a["z"], role=role,
                             ref_filament=av[0], mica_filament=bv[0],
                             error_pct=(bv[0]/av[0]-1)*100 if av[0] else (0. if bv[0] == 0 else None),
                             ref_length=av[1], mica_length=bv[1], ref_runs=av[2], mica_runs=bv[2]))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference")
    parser.add_argument("mica")
    parser.add_argument("--csv")
    args = parser.parse_args()
    rows = compare(parse(args.reference), parse(args.mica))
    print("role                   max |error|   layers >3%/>5%   run-count mismatches")
    for role in (*WALLS, "All walls"):
        selected = [r for r in rows if r["role"] == role]
        errors = [abs(r["error_pct"]) if r["error_pct"] is not None else math.inf for r in selected]
        print(f"{role:22} {max(errors):10.2f}% {sum(e>3 for e in errors):7}/{sum(e>5 for e in errors):<7}"
              f" {sum(r['ref_runs'] != r['mica_runs'] for r in selected):7}/{len(selected)}")
    if args.csv:
        with open(args.csv, "w", newline="") as stream:
            writer = csv.DictWriter(stream, fieldnames=rows[0].keys())
            writer.writeheader()
            writer.writerows(rows)


if __name__ == "__main__":
    main()
