# G-code thumbnails

`mica slice` renders the placed source mesh using the resolved printer profile's `thumbnails` list.
CORE One/XL profiles request 16×16, 313×173 and 480×240 QOI plus 380×285 PNG. The U1 requests
48×48 and 300×300 PNG. Both Orca settings (`thumbnails`, `thumbnails_format`) now map to the
canonical configuration; an empty list disables thumbnails.

The renderer is CPU-only Rust. It uses PrusaSlicer's orthographic
`Rx(-45°) * Rz(45°)` camera, centres on the mesh bounding box, fits its projected corners with
Prusa's 1.025 margin, and applies Orca's additional 1% XY / 2% Z padding for Klipper. A depth buffer,
back-face culling, flat two-light shading and 2× supersampling produce transparent RGBA images.
The first tool's `extruder_colour` overrides `filament_colour`; invalid/missing colors fall back
to `#FF8000`. No bed or shadow is drawn.

Vertices are transformed once per image (12 additional bytes each); faces are streamed with no
per-face allocation. Triangles whose bounds contain no sample centres are skipped. Small triangles
that cover samples are retained to avoid holes in dense meshes. Image/depth buffers are bounded by
the profile's less-than-1000-pixels-per-side limit.

PNG uses Sub-filtered scanlines and the existing `flate2` dependency. QOI is an independent Rust
encoder from the format description, including INDEX, DIFF, LUMA, RUN, RGB and RGBA chunks. Both
preserve alpha; supersampled edges use straight alpha to avoid dark fringes. The CLI's layer preview
shares the compressed PNG writer.

Text G-code uses Prusa's `thumbnail` / `thumbnail_QOI` tags, the base64 character count, and
78-character base64 payload lines. Blocks follow the producer header and precede extrusion-width
comments. Orca blocks follow `HEADER_BLOCK_END` and each has `THUMBNAIL_BLOCK_START/END` wrappers.
Binary G-code carries the same encoded image bytes in `BgcodeMetadata.thumbnails`, supplied through
`prusa_metadata::bgcode_metadata`; its G-code blocks contain no duplicate base64 comments.

## Library API

- `mica_gcode::thumbnails::generate(&Config, &TriangleMesh)` returns encoded `bgcode::Thumbnail`s.
- `render(mesh, width, height, rgb, Camera::{Prusa,Orca})` returns an `Image` with top-down RGBA8 pixels.
- `encode_png`, `encode_png_rgb`, and `encode_qoi` validate dimensions/buffer lengths and return bytes.
- `make_and_check_thumbnail_list(value, default_format)` returns requests plus accumulated error flags.
- `write_text(writer, thumbnails, orca)` writes comment blocks.
- Existing `export_gcode_with_mesh` and `export_file(..., Some(mesh), ...)` generate previews automatically.
  `export_gcode` / `export_file(..., None, ...)` omit them because no geometry is available.

The parser preserves order and duplicate requests, accepts fractional dimensions (truncated to
pixels), accepts case-insensitive formats, and honors legacy `thumbnails_format` when no extension
is given. Relative to Prusa's C++ stream extraction, Mica rejects trailing junk in numeric tokens,
extra slash suffixes and sub-one-pixel dimensions, and permits whitespace around format names.
Invalid list entries fail export. JPG is recognized but explicitly unsupported; PNG and QOI cover
the target profiles. BTT_TFT/COLPIC, painted regions, multiple tool colors, modifiers, bed rendering,
perspective and upstream GPU antialiasing are outside this implementation. Lighting follows Prusa's
shader with a flat face normal and a parallel viewing ray for the specular term.

## Validation and oracle (2026-10-15)

Unit tests decode emitted PNG and QOI (including CRC/zlib, all QOI opcodes, alpha and run lengths),
check base64 lengths/wrapping/tags, validate lists and colors, and check centred, shaded, transparent,
translation-independent rendering. Export tests decode the CORE One binary container, validate all
four image blocks, and compare their bytes with the text comment payloads. U1 tests check the actual
imported profile sizes, wrappers and placement. The live Orca 2.4.1 startup/end/layer comparison still
passes, excluding only thumbnail blocks (Orca's STL CLI has no rendering callback).

The release CLI sliced cube20 and 3DBenchy to CORE One text/binary and U1 text in the system temp
directory. Binary files were decoded with Mica's bgcode module; all text blocks were independently
base64-decoded and their lengths, PNG CRCs/zlib/pixels and dimensions checked. Text and binary image
bytes matched. Prusa's bundled C QOI decoder also successfully decoded every emitted QOI image.
All twelve model/size previews were opened with the image viewer and showed the model clearly.

For a same-geometry visual comparison, the private `dense-painted.3mf` (`MICA_PRIVATE_MODELS`) was
opened read-only and its mesh and build transform extracted to a temporary STL. At
512×512, opaque silhouette bounds were `(66,108)..(500,449)` for Mica versus `(67,109)..(500,449)`
for the supplied Snapmaker `Metadata/plate_1.png`. Orientation and framing agree closely. Mica's
single orange color intentionally does not reproduce that project's painted colors.

Release renderer timings on this machine (excluding STL load and image encoding):

| Mesh | Triangles | Image | Time |
|---|---:|---|---:|
| 3DBenchy | 225,102 | 480×240 | about 6–7 ms |
| Private dense painted model | 1,998,378 | 480×240 | about 27 ms |
| Subdivided cube | 4,320,000 | 480×240 | about 22 ms |

Reproduce the generated images and dense-mesh timing with:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo test --release --offline -p mica-gcode --lib render_oracle -- --ignored --nocapture
```

Images go in `std::env::temp_dir()/mica-thumbnail-oracle`. Optionally set
`MICA_THUMBNAIL_ORACLE_STL` to a bed-oriented STL to render and time it at 480×240 and 512×512 too.
No project-derived fixture is committed. Review the camera basis/lighting, strict parser differences,
Orca mapping additions and container placement when changing this feature.
