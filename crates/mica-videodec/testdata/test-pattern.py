# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
#
# Regenerates test-pattern-320x240.h264 (the picture of mica_camera::mock::test_pattern,
# 20 frames at 10 fps, one IDR then P frames, constrained baseline, access unit delimiters,
# no SEI). Encoded with Apple's VideoToolbox encoder through ffmpeg (macOS):
#
#   python3 -I test-pattern.py | ffmpeg -f rawvideo -pix_fmt rgb24 -s 320x240 -r 10 -i - \
#     -c:v h264_videotoolbox -profile:v baseline -realtime 1 -g 20 -bf 0 -b:v 300k \
#     -bsf:v h264_metadata=aud=insert,filter_units=remove_types=6 -f h264 test-pattern-320x240.h264
import sys
# Same picture as mica_camera::mock::test_pattern: four quadrants and a dark bar sweeping.
W, H, FPS, N = 320, 240, 10, 20
out = sys.stdout.buffer
for n in range(N):
    t = n / FPS
    bar = int(((t * 0.5) % 1.0) * W)
    rows = []
    for y in range(H):
        row = bytearray()
        for x in range(W):
            if x < W // 2 and y < H // 2:
                c = (220, 40, 40)
            elif y < H // 2:
                c = (40, 200, 60)
            elif x < W // 2:
                c = (50, 70, 220)
            else:
                c = (235, 235, 235)
            if abs(x - bar) < W // 40 + 1:
                c = (c[0] // 3, c[1] // 3, c[2] // 3)
            row += bytes(c)
        rows.append(bytes(row))
    out.write(b"".join(rows))
