#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: adapted
# Source: prusaslicer@b028299c77 src/libslic3r/Geometry/ArcWelder.hpp :: arc_angle
# Changes: Original comparison script; signed-radius arc length formula added.
"""Compare two G-code files per extrusion role (filament and path length), counting G2/G3 arcs.

Usage: scripts/compare-gcode.py reference.gcode mica.gcode [--split-bridges]
--split-bridges distinguishes Prusa's internal bridges by HEIGHT > object layer height.
Only for thin external bridges with bridge_flow_ratio=1, as in the CORE One oracle.
"""
import sys, re, math, collections
split_bridges = "--split-bridges" in sys.argv
def parse(p):
    """Per-role extruded length and filament, counting G1 lines and G2/G3 arcs (I/J centre offsets or signed R)."""
    infer_bridges = split_bridges
    if infer_bridges:
        config = {}
        for line in open(p):
            match = re.match(r"; (thick_bridges|bridge_flow_ratio) = (.*)", line)
            if match: config[match[1]] = match[2].strip()
        if config.get("thick_bridges") != "0" or config.get("bridge_flow_ratio") != "1":
            raise ValueError("--split-bridges requires thick_bridges=0 and bridge_flow_ratio=1")
    z = previous_z = height = layer_height = 0.0
    E = collections.Counter(); L = collections.Counter(); counts=collections.Counter(); role=None; x=y=None
    for line in open(p):
        if line.startswith(";Z:"):
            previous_z, z = z, float(line[3:]); layer_height = z - previous_z
        if line.startswith(";HEIGHT:"): height = float(line[8:])
        if line.startswith(";TYPE:"): role=line[6:].strip(); continue
        if not line.startswith(("G1 ", "G2 ", "G3 ")): continue
        w = dict((t[0], float(t[1:])) for t in line.split(";")[0].split()[1:] if t[0] in "XYZEIJFR")
        nx, ny = w.get("X", x), w.get("Y", y)
        if "E" in w and w["E"] > 0 and x is not None and ("X" in w or "Y" in w):
            counts[line.split()[0]] += 1
            if line.startswith("G1"):
                d = math.hypot(nx-x, ny-y)
            elif "R" in w:
                r = abs(w["R"])
                sweep = 2 * math.asin(min(1, math.hypot(nx-x, ny-y)/(2*r)))
                d = r * (2*math.pi-sweep if w["R"] < 0 else sweep)
            else:
                cx, cy = x + w.get("I", 0), y + w.get("J", 0); r = math.hypot(x-cx, y-cy)
                a0, a1 = math.atan2(y-cy, x-cx), math.atan2(ny-cy, nx-cx)
                sweep = (a1 - a0) % (2*math.pi) if line.startswith("G3") else (a0 - a1) % (2*math.pi)
                d = r * (sweep if sweep > 1e-9 else 2*math.pi)
            actual_role = "Internal bridge infill" if infer_bridges and role == "Bridge infill" and height > layer_height + 0.01 else role
            L[actual_role] += d; E[actual_role] += w["E"]
        x, y = nx, ny
    return E, L, counts
a, b = parse(sys.argv[1]), parse(sys.argv[2])
print(f"{'role':22}{'filament mm':>24}{'path length mm':>28}")
print(f"{'':22}{'Prusa':>10}{'Mica':>8}{'diff':>7}{'Prusa':>11}{'Mica':>9}{'diff':>7}")
for k in sorted(set(a[0])|set(b[0]), key=lambda k:-a[0].get(k,0)):
    pe, me, pl, ml = a[0].get(k,0), b[0].get(k,0), a[1].get(k,0), b[1].get(k,0)
    f = lambda m, p: f"{(m-p)/p*100:+6.1f}%" if p else "     -"
    print(f"{str(k):22}{pe:10.1f}{me:8.1f}{f(me,pe)}{pl:11.0f}{ml:9.0f}{f(ml,pl)}")
print(f"{'total':22}{sum(a[0].values()):10.1f}{sum(b[0].values()):8.1f}")

for label, result in [("first",a),("second",b)]:
    print(f"{label}: {sum(result[2].values())} extrusion moves; G1={result[2]['G1']} G2={result[2]['G2']} G3={result[2]['G3']}")
