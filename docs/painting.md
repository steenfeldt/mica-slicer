# Facet painting from 3MF

> The GUI's painting tools (brushes, overlay, undo, the slicer hand-off) are described in
> [gui-painting.md](gui-painting.md); they produce the same `FacetPainting` this page reads.

Mica reads and applies Prusa and Orca/Bambu facet painting through the existing 3MF CLI and GUI
import path. No new CLI flags or dependencies are required. Select the actual printer and its
profiles as usual; embedded project presets are still retained as metadata, not activated.

| Painting | Prusa triangle attribute | Orca/Bambu attribute | Applied to |
|---|---|---|---|
| Supports | `slic3rpe:custom_supports` | `paint_supports` | Normal, tree and organic contact masks |
| Seams | `slic3rpe:custom_seam` | `paint_seam` | The existing object seam plan |
| Material | `slic3rpe:mmu_segmentation` | `paint_color` | Per-layer regions and physical-tool assignments |
| Fuzzy skin | `slic3rpe:fuzzy_skin` | `paint_fuzzy_skin` | Classic and Arachne wall painting hooks |

## API and representation

`threemf::Part::painting` and `regions::Volume::painting` contain `painting::FacetPainting`.
Its four `FacetStates` streams store sorted `(triangle index, stream offset)` records and a shared
byte-per-nibble buffer. Meshes are not copied for each state. `push(triangle, wire_string)` validates
the reversed hexadecimal TriangleSelector stream, including all edge-split patterns, recursive
children and extended material states through 255. Truncated, trailing and malformed codes fail
import with the attribute and triangle identified. Recursion is limited to 64 levels.

`FacetStates::visit(mesh, include_default, callback)` reconstructs leaf triangles on demand.
`triangle_range` carries annotations into Prusa volume ranges. Mesh winding repair and mirrored
component/instance transforms preserve the original split-tree orientation. Translations and
ordinary transforms operate on mesh vertices, so painting follows the volume without a separate
transform cache. The importer also accepts literal `#` characters in Orca internal model filenames;
archive traversal checks remain in place.

`input::load` carries painting into the existing region model, and `PrintObject::prepare_regions`
applies all four streams. `painting::query::PaintedQuery` implements `seam::painting::SeamPainting`.
It builds one state-tagged BVH over painted leaf facets, with blocker precedence for intersecting
query spheres. Fuzzy painting uses the same geometry query through `FuzzySkinPainting`; wall edges
are sampled at 0.1 mm before displacement so a stripe between original vertices is detected.
Fuzzy displacement retains Mica's deterministic random sequence and skips the first layer.

Downward painted support facets are clipped to object-layer slabs. Their projected polygons join
volume enforcer/blocker masks before normal or tree contact generation. Manual support mode honors
enforcers; painting alone does not enable supports. Orca's `normal(manual)`/`tree(manual)` map to
this mode (see [tree-supports.md](tree-supports.md#enforcers-only-orca-manual-and-prusa-for-support-enforcers-only-2026-10-16)). Existing support routing and landing policies
remain applicable.

Colour sections are projected onto slice boundaries, filtered, and partitioned with Prusa's
coloured-boundary Voronoi graph traversal. Computation uses local coordinates and boundary cleanup
to avoid near-duplicate Voronoi sites. Top and bottom painted facets seed material skins that
propagate through configured solid layers. `mmu_segmented_region_max_width` cuts secondary material
to a shell around the dominant material; `mmu_segmented_region_interlocking_depth` alternates the
cut width between layers. The `interlocking_beam` family generates voxel-based alternating beams,
including interface depth, air-boundary avoidance, angle, beam width and layer count. Orca preset
and metadata mapping recognizes these eight canonical options.

Nonzero material states are one-based tools; zero retains the inherited region assignment. Invalid
tool indices fail slicing. Painted material sets perimeter, sparse-infill and solid-infill tools
after modifiers have applied their settings. Thus a modifier can change density or speed without
replacing a painted colour. Paths retain the normal `PathAssignment.region` and physical tool,
including XL/U1 tool changes and the existing tower planner. Shared shell detection includes the
upstream simulated single-material perimeter inset, preventing internal colour boundaries from
creating spurious solid shells. The original model silhouette is retained independently of the
colour partition for seams, bridge detection and supports. Unassigned partition remainder retains
the inherited material. Disabled supports skip the otherwise expensive bridge-coverage prepass.

## Oracle results, 2026-10-16

The generated colour fixtures use PrusaSlicer 2.9.6 with the XL 5T Input Shaper, 0.20 mm SPEED and
Prusament PLA profiles. Support, seam and fuzzy fixtures use CORE One HF0.4, 0.20 mm SPEED and
Prusament PLA. Towers are disabled for the material comparisons.

| Fixture | Largest absolute role/tool difference | Result |
|---|---:|---|
| Cube, two adjacent side faces painted tool 2 | 1.096% | All roles within ±3%; identical layer/tool sequence |
| Cube, 2 mm material width and 1 mm interlocking depth | 0.787% | All roles within ±3% |
| Asymmetric box with one-, two- and three-edge split trees | 0.513% | All roles within ±3% |
| Supported roof | 0.191% | All roles within ±3% |
| Painted blocker on half the roof | 1.630% | All roles within ±3% |
| Painted enforcer, automatic supports disabled | 1.669% | All roles within ±3% |
| Cylinder with an enforced seam stripe | 0.146% | 100 seams, maximum position difference 0.0483 mm |
| Painted fuzzy side | 0.310% for shared roles | Extra 0.08021 mm overhang filament absent from reference |
| Cube with interlocking beams | 58.785% on tool 2 solid infill | 1.66516 versus 1.04869 mm; other roles within ±3% |
| Symmetric split-tree cube | 7.579% | Tool 2 wall/infill differences; reference also logs missing Voronoi vertices |

The last three rows are open parity issues, not passing ±3% results. The asymmetric split fixture
exercises the same split encodings without those reference Voronoi warnings. Normal support painting
has the numerical oracle above; tree/organic painting has contact-mask integration tests.

The original seven-instance dragon imported and exported with three tools. A successful Orca 2.4.1
reference used a temporary copy with explicit four-slot machine/process/filament vectors and no
tower; the original project was not changed. The holder comparison reuses the successful normalized
Orca reference described in [regions.md](regions.md).

| Dragon physical tool | Orca deposited filament | Mica deposited filament | Difference |
|---|---:|---:|---:|
| 1, primary material | 32,136.46 mm | 38,403.31 mm | +19.50% |
| 2, dark accent | 388.73 mm | 627.29 mm | +61.37% |
| 3, light accent | 19.12 mm | 20.33 mm | +6.32% |

The final Mica dragon export contains 152 layers and completed in 235.6 seconds; Orca emits 151
layers. Both use the same dominant and accent tools, but these totals do **not** establish material
parity. Primary walls differ by less than 1.9%, while primary solid infill is +122.6% and sparse infill
is −49.5%. The shell/bridge pipeline and accent segmentation remain review targets. Brim and startup
configuration also differ; the table is deposited G-code after retraction repayment, not footer estimates.

The holder exports 525 layers using tools 1 and 2. Deposited model filament is 52,070.49/7,150.72 mm
in Mica versus 53,323.42/7,112.24 mm in Orca (−2.35%/+0.54%, excluding brim). Orca additionally
deposits 919.09 mm of tool-2 brim absent from this Mica run. Per-role parity is still incomplete:
tool-2 walls are approximately 18% higher, and tool-1 bridge/top-skin differences persist.

All five instances of five-plates finish individually at 116 layers each. All 20 cat instances also finish,
covering all five plates. Their respective geometric filament totals from the API smoke helper,
in tool order 1–4, are 6,279/3,100/1,117/5,388 mm and 101,731/11,346/7,318/326 mm. These are geometry
measurements before exporter filament multipliers and do not assert Orca parity.

Dense-painted also completes all 114 layers, using tools 1, 2 and 4 with approximately
145/6,218/706 mm geometric filament. Its 1,998,378 source triangles include 1,249,588 annotated
triangles. Preparation took 437 seconds and the complete instance slice took 649 seconds
(654 seconds including import), so dense painted models remain a performance review target.
The dragon and holder were exported as G-code; five-plates, cat and dense-painted were validated through
all finished path batches with the API helper. These dense-painted times describe the original
implementation; later optimizations made them much faster.

The single five-plates Orca attempt used explicit four-slot vectors, arrangement disabled and all five
plates; it exited 232. Its application log identifies the cause: project version 2.5.0.66 is newer
than CLI version 2.4.1. No retry or further Orca invocation followed.
No Orca parity is claimed for five-plates, cat or dense-painted.

## Reproduction

The original fixture generator writes synthetic meshes and annotations directly. Reference outputs
and private projects stay outside the repository:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
python3 scripts/make-painting-oracle.py /tmp/mica-painting-oracle
out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer \
  --datadir out/ref/datadir \
  --printer-profile 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print-profile '0.20mm SPEED @XLIS 0.4' --material-profile 'Prusament PLA @XLIS' \
  --center 180,180 --no-wipe-tower --no-binary-gcode --export-gcode \
  -o /tmp/mica-painting-oracle/colour-ref.gcode /tmp/mica-painting-oracle/colour.3mf
./target/release/mica slice /tmp/mica-painting-oracle/colour.3mf \
  --printer 'Original Prusa XL - 5T Input Shaper 0.4 nozzle' \
  --print '0.20mm SPEED @XLIS 0.4' --filament 'Prusament PLA @XLIS' \
  --center 180,180 --set wipe_tower=0 -o /tmp/mica-painting-oracle/colour-mica.gcode
python3 scripts/compare-painting.py /tmp/mica-painting-oracle/colour-ref.gcode \
  /tmp/mica-painting-oracle/colour-mica.gcode
```

Use the CORE One HF0.4 reference/default Mica profiles for `support`, `enforcer`, `blocker`, `seam`
and `fuzzy`. Pass `--seams` to the comparison script for the cylinder. Comparisons measure deposited
filament after retraction repayment, by role and zero-based tool, and report every failed ±3% check.

`cargo run --offline --release -p mica-profiles --example slice_painting -- project.3mf` is a
read-only U1 smoke helper. It centers and slices every printable instance individually, releases
finished path batches, and reports geometric filament totals as JSON. This tests all plates' objects
without claiming support for a multi-plate G-code schedule. `MICA_PAINTING_OBJECT` selects one
zero-based object for diagnosis.

## Validation

`cargo fmt --all --check`, `cargo clippy --offline --workspace --all-targets -- -D warnings`,
`cargo test --offline --workspace`, the offline release CLI build, and
`CARGO_NET_OFFLINE=true cargo xtask provenance` passed. The final workspace run completed 517 tests
with zero failures and two existing ignored tests (manual thumbnail rendering and the broader
exact-G-code acceptance oracle). Provenance checked 239 Rust files, including 149 ported/adapted files.

The 12 painting regressions cover every split pattern, extended states and malformed trees,
orientation repair, sparse support projection, seam precedence, material width/depth and cap
propagation, alternating beams, classic/Arachne fuzzy hooks, modifier/tool precedence, batch
invariance, normal/organic support contacts, and preservation of the original model silhouette.
Importer tests cover both dialects, volume ranges, component mirrors and literal-hash archive paths.

## Scope and review

The decoder, slab projection, seam query, material boundary partition, cap propagation and beam
generation are ports/adaptations of the pinned PrusaSlicer sources registered in `PROVENANCE.md`.
The interlocking code retains its Ultimaker attribution. Import also follows pinned Orca's attribute
names. Fixtures, comparison tooling and integration tests are original; no third-party assets were
added.

Material segmentation currently processes each model volume separately, then uses the existing
ordered region booleans. It does not merge all volumes into one segmentation surface as Prusa does.
Beam generation likewise operates on painted colours within a volume, not arbitrary adjacent volumes.
Cap propagation uses the object's and volume's resolved settings; modifiers and height ranges do
not independently recalculate its material shell depth. The fuzzy hook uses facet proximity rather
than Prusa's separate fuzzy polygon segmentation. Colour projection and simplification use Mica's
indexed geometry and Clipper2; upstream corner snapping and every numerical tie rule are not identical.
These choices require close review on overlapping volumes, narrow colour islands and sloping skins.

Painting editing, 3MF re-export, virtual extruders, interface shells between materials, adaptive layer
profiles and multi-plate scheduling are outside this change. Existing tree-routing limitations and
Orca/Mica shell, bridge and adhesion differences also apply. Review split-tree winding, modifier/tool
precedence, colour-boundary cleanup, material cap propagation and beam air filtering closely.
