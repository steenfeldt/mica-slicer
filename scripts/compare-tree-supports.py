#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Compare support filament and connected bed footprints; optionally render paired G-code layers.

Usage: compare-tree-supports.py reference.gcode mica.gcode report.csv [preview_dir z ...]
Footprints are components of deposited support on the first print layer (flanges can join trunks).
The second-layer component count is also reported to distinguish trunks from flanges. Raster pitch
is 0.08 mm with 8-connectivity. Width comes from ;WIDTH: (0.4 mm fallback). G2/G3 arcs use I/J.
PNG output uses only the Python standard library; reference on left, Mica on right, same XY scale.
"""
import math
import re
import runpy
import struct
import sys
import zlib
from collections import defaultdict
from pathlib import Path

support = runpy.run_path(str(Path(__file__).with_name('compare-supports.py')))


def segments(path):
    result = defaultdict(list)
    relative, e, x, y, z, width, role = False, 0., 0., 0., None, .4, ''
    with open(path) as stream:
        for line in stream:
            if line.startswith(';Z:'):
                z = round(float(line[3:]), 3)
            elif line.startswith(';TYPE:'):
                role = line[6:].strip()
            elif line.startswith(';WIDTH:'):
                width = float(line[7:])
            code = line.split(';', 1)[0].strip()
            if not code:
                continue
            command = code.split()[0]
            words = {k: float(v) for k,v in re.findall(r'([XYZEIJ])(-?(?:\d+(?:\.\d*)?|\.\d+))', code)}
            if command == 'M83':
                relative = True
            elif command == 'M82':
                relative = False
            elif command == 'G92':
                e = words.get('E', e)
            elif command in ('G0','G1','G2','G3'):
                nx,ny = words.get('X',x),words.get('Y',y)
                delta = words.get('E',0.) if relative else words.get('E',e)-e
                e = e+delta
                if delta>0 and z is not None and (nx!=x or ny!=y or command in ('G2','G3')):
                    points = [(x,y),(nx,ny)]
                    if command in ('G2','G3'):
                        cx,cy = x+words.get('I',0.),y+words.get('J',0.)
                        radius = math.hypot(x-cx,y-cy)
                        a,b = math.atan2(y-cy,x-cx),math.atan2(ny-cy,nx-cx)
                        sweep = (b-a)%math.tau if command=='G3' else -((a-b)%math.tau)
                        if abs(sweep)<1e-10:
                            sweep = math.tau if command=='G3' else -math.tau
                        n = max(1,math.ceil(abs(sweep)*radius/.04))
                        points = [(cx+radius*math.cos(a+sweep*i/n),cy+radius*math.sin(a+sweep*i/n)) for i in range(n+1)]
                    result[z].extend((a,b,width,role) for a,b in zip(points,points[1:]))
                x,y = nx,ny
    return dict(result)


def raster(lines, pitch=.08):
    occupied = set()
    for a,b,width,_ in lines:
        r = width/2
        dx,dy = b[0]-a[0],b[1]-a[1]
        n = max(1,math.ceil(math.hypot(dx,dy)/(pitch/2)))
        # Cover the extrusion capsule by densely sampled disks.
        for i in range(n+1):
            x,y = a[0]+dx*i/n,a[1]+dy*i/n
            ix,iy = math.floor(x/pitch),math.floor(y/pitch)
            k = math.ceil(r/pitch)
            for xx in range(ix-k,ix+k+1):
                for yy in range(iy-k,iy+k+1):
                    if ((xx+.5)*pitch-x)**2+((yy+.5)*pitch-y)**2 <= r*r:
                        occupied.add((xx,yy))
    return occupied


def components(cells):
    cells = set(cells)
    count = 0
    while cells:
        count += 1
        todo = [cells.pop()]
        while todo:
            x,y = todo.pop()
            for dx in (-1,0,1):
                for dy in (-1,0,1):
                    neighbor = x+dx,y+dy
                    if neighbor in cells:
                        cells.remove(neighbor)
                        todo.append(neighbor)
    return count


def render(left,right,path):
    all_lines = left+right
    if not all_lines:
        return
    pitch = .08
    points = [p for a,b,_,_ in all_lines for p in (a,b)]
    x0,y0 = (math.floor(min(p[i] for p in points)/pitch)-8 for i in (0,1))
    x1,y1 = (math.ceil(max(p[i] for p in points)/pitch)+8 for i in (0,1))
    w,h = x1-x0+1,y1-y0+1
    width = w*2+12
    pixels = bytearray([24,24,24])*(width*h)
    for side,lines in enumerate((left,right)):
        roles = defaultdict(list)
        for seg in lines:
            roles[seg[3]].append(seg)
        for role,segs in sorted(roles.items(),key=lambda p: p[0].startswith('Support')):
            color = (67,187,113) if role=='Support material' else (40,196,219) if role=='Support material interface' else (170,170,170)
            for x,y in raster(segs,pitch):
                xx,yy = x-x0+side*(w+12),y1-y
                if 0<=xx<width and 0<=yy<h:
                    at = 3*(yy*width+xx)
                    pixels[at:at+3] = bytes(color)
    def chunk(kind,data):
        return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    raw = b''.join(b'\0'+pixels[y*width*3:(y+1)*width*3] for y in range(h))
    path.write_bytes(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',width,h,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b''))


def main(reference,actual,csv_path,preview=None,*zs):
    support['compare'](reference,actual,csv_path)
    ref,got = segments(reference),segments(actual)
    for label,layers in [('reference',ref),('mica',got)]:
        for z in sorted(layers)[:2]:
            paths = [s for s in layers[z] if s[3] in support['ROLES']]
            print(f'{label}: Z={z:g}, support components={components(raster(paths))}')
    if preview:
        directory = Path(preview)
        directory.mkdir(parents=True,exist_ok=True)
        for z in map(float,zs):
            render(ref.get(z,[]),got.get(z,[]),directory/f'z{z:.3f}.png')


if __name__ == '__main__':
    main(*sys.argv[1:])
