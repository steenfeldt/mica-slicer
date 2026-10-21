#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Extract external-loop starts after travel and compare text G-code (standard library only).

Usage: python3 scripts/seam-oracle.py reference.gcode mica.gcode --out /tmp/seams
Writes all starts to CSV and metrics to JSON. Does not assume identical path/island order.
"""
import argparse
import csv
import json
import math
import re
import statistics
import struct
from pathlib import Path


def extract(path):
    xyz = [0.0, 0.0, 0.0]
    e = 0.0
    relative_e = False
    relative_xyz = False
    layer = 0
    role = ""
    travelled = True
    travel_origin = xyz[:]
    current = None
    starts = []
    for raw in Path(path).read_text().splitlines():
        if raw.startswith(";LAYER_CHANGE"):
            layer += 1
            travel_origin = xyz[:]
            travelled = True
            current = None
        if raw.startswith(";TYPE:"):
            role = raw[6:].strip()
        code = raw.partition(";")[0].strip()
        if not code:
            continue
        command = code.split()[0]
        fields = {key: float(value) for key, value in re.findall(r"([XYZEFIJKR])([-+]?(?:\d*\.\d+|\d+\.?\d*))", code)}
        if command == "M83":
            relative_e = True
        elif command == "M82":
            relative_e = False
        elif command == "G90":
            relative_xyz = False
        elif command == "G91":
            relative_xyz = True
        elif command == "G92":
            e = fields.get("E", e)
            for i, axis in enumerate("XYZ"):
                xyz[i] = fields.get(axis, xyz[i])
        elif command in ("G0", "G1", "G2", "G3"):
            old = xyz[:]
            for i, axis in enumerate("XYZ"):
                if axis in fields:
                    xyz[i] = fields[axis] + (old[i] if relative_xyz else 0.0)
            de = fields.get("E", 0.0) if relative_e else fields.get("E", e) - e
            if "E" in fields:
                e = e + fields["E"] if relative_e else fields["E"]
            length = math.dist(old[:2], xyz[:2])
            if command in ("G2", "G3") and ("I" in fields or "J" in fields):
                center = old[0] + fields.get("I", 0.0), old[1] + fields.get("J", 0.0)
                start_angle = math.atan2(old[1] - center[1], old[0] - center[0])
                end_angle = math.atan2(xyz[1] - center[1], xyz[0] - center[0])
                sweep = ((end_angle - start_angle) if command == "G3" else (start_angle - end_angle)) % math.tau
                length = (sweep or math.tau) * math.dist(old[:2], center)
            if length > 1e-6 and ("E" not in fields or de < 0.0):
                travelled = True
                travel_origin = xyz[:]
                current = None
            elif length > 1e-6 and de > 0.0:
                if travelled and layer > 0 and role in ("External perimeter", "Outer wall"):
                    current = dict(layer=layer, x=travel_origin[0], y=travel_origin[1], z=xyz[2], length=0.0)
                    starts.append(current)
                if current is not None:
                    current["length"] += length
                travelled = False
    return starts


def stats(values):
    if not values:
        return {}
    values = sorted(values)
    return dict(count=len(values), mean=statistics.mean(values), median=statistics.median(values),
                p95=values[min(len(values) - 1, math.ceil(len(values) * .95) - 1)], max=max(values))


def by_layer(starts):
    layers = {}
    for seam in starts:
        layers.setdefault(seam["layer"], []).append(seam)
    return layers


def xy(p):
    return p["x"], p["y"]


def alignment(starts):
    layers = by_layer(starts)
    # Longest external loop is a useful stable contour proxy; report all-loop counts separately.
    primary = [max(layers[l], key=lambda p: p["length"]) for l in sorted(layers) if l > 1]
    distances = [math.dist(xy(a), xy(b)) for a, b in zip(primary, primary[1:]) if b["layer"] == a["layer"] + 1]
    if not primary:
        return {}
    center = statistics.mean(p["x"] for p in primary), statistics.mean(p["y"] for p in primary)
    return dict(primary_adjacent_layer_mm=stats(distances),
                primary_rms_spread_mm=math.sqrt(statistics.mean(math.dist(xy(p), center) ** 2 for p in primary)),
                primary_x_range_mm=max(p["x"] for p in primary) - min(p["x"] for p in primary),
                primary_y_range_mm=max(p["y"] for p in primary) - min(p["y"] for p in primary))


def compare(reference, mica):
    a, b = by_layer(reference), by_layer(mica)
    distances = []
    primary = []
    counts_differ = []
    for layer in sorted(a.keys() | b.keys()):
        aa, bb = a.get(layer, []), b.get(layer, [])
        if len(aa) != len(bb):
            counts_differ.append(layer)
        if not aa or not bb:
            continue
        # Bidirectional nearest distances do not require matching print order or equal loop counts.
        distances.extend(min(math.dist(xy(p), xy(q)) for q in aa) for p in bb)
        distances.extend(min(math.dist(xy(p), xy(q)) for q in bb) for p in aa)
        if layer > 1:
            primary.append(math.dist(xy(max(aa, key=lambda p: p["length"])), xy(max(bb, key=lambda p: p["length"]))))
    return dict(reference_starts=len(reference), mica_starts=len(mica), layers_with_different_counts=counts_differ,
                bidirectional_nearest_mm=stats(distances), primary_loop_distance_mm=stats(primary),
                reference_alignment=alignment(reference), mica_alignment=alignment(mica),
                reference_layer_2=a.get(2, []), mica_layer_2=b.get(2, []))


def generate_cylinder(path, radius=10.0, height=20.0, facets=128):
    """Watertight outward-facing cylinder; modulo closes the final edge bit-for-bit."""
    def point(i, z):
        angle = math.tau * (i % facets) / facets
        return radius * math.cos(angle), radius * math.sin(angle), z
    triangles = []
    for i in range(facets):
        a, b, c, d = point(i, 0), point(i + 1, 0), point(i + 1, height), point(i, height)
        triangles.extend([(a, b, c), (a, c, d), ((0, 0, 0), b, a), ((0, 0, height), d, c)])
    with Path(path).open("wb") as stream:
        stream.write(b"Mica seam oracle cylinder".ljust(80, b"\0"))
        stream.write(struct.pack("<I", len(triangles)))
        for triangle in triangles:
            stream.write(struct.pack("<12fH", 0, 0, 0, *[v for p in triangle for v in p], 0))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("mica", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    a, b = extract(args.reference), extract(args.mica)
    for name, starts in (("reference", a), ("mica", b)):
        with (args.out / f"{name}.csv").open("w") as f:
            writer = csv.DictWriter(f, fieldnames=["layer", "x", "y", "z", "length"])
            writer.writeheader()
            writer.writerows(starts)
    result = compare(a, b)
    (args.out / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
