# Regions, object settings and support volumes

Mica applies sparse settings from Prusa and Orca/Snapmaker 3MF projects automatically. The CLI flags
are unchanged. CLI-selected printer, process and filament profiles remain the base configuration;
embedded project presets are retained by the importer but are not loaded as profiles.

## API and configuration precedence

`mica_core::regions::{RegionModel, Volume, HeightRange}` describes an instance's volumes in bed
coordinates with sparse canonical `Config` patches. `PrintObject::prepare_regions` prepares one
instance from its combined model mesh, region model, base configuration and object tool assignments.
The combined mesh supplies bounds and mesh-dependent infill/seam context; slice booleans use the
individual volumes. `regions::apply_config` applies patches, including the common `extruder` fallback.

Precedence is base profiles → object → height range → model part → intersecting modifiers in archive
order. A modifier inherits the configuration of each region it intersects, so overlapping modifiers
accumulate settings and the later value wins. `extruder` resets perimeter, sparse and solid tool
defaults; explicit role tool keys in the same patch take precedence. Existing one-based CLI tool
selection remains supported. Orca metadata uses `mica_profiles::orca::mapping::map_metadata`, which
shares the preset key translations but starts from an empty configuration. It must not inject preset
defaults into a sparse object/part patch.

`layer_height`, `first_layer_height`, `infill_every_layers` and `solid_infill_every_layers` are resolved
at object scope. Range `layer_height` controls the layer schedule, while pattern spacing retains the
object's nominal height. Part/modifier overrides do not change the object's combination interval.
Explicit height ranges are finite, nonnegative, sorted and nonoverlapping. Region settings are selected
at the slicing plane, using half-open `[min_z, max_z)` ranges. Piecewise layer heights follow the upstream
minimum-height sampling rule and retain the fixed first layer. Adaptive layer-height profile files are
still reported as ignored.

The importer reads both `Metadata/Prusa_Slicer_layer_config_ranges.xml` and
`Metadata/layer_config_ranges.xml`. Their one-based IDs are model indices, not resource IDs: Prusa uses
mesh-resource order, Orca first-build order. Regression coverage includes reversed build order.

`PreparedObject::{region, region_count, layer_regions, has_regions}` exposes indexed settings and
geometry. `LayerRegion` contains a region index, slices and classified fill surfaces; it does not clone
settings on every layer. `PrintRegion` owns the resolved configuration and physical-tool settings once.
`PathAssignment.region` carries the index through `PreparedPrint` batching and G-code export, including
region speeds and physical-tool temperature/retraction settings. Support and adhesion paths use the
object configuration.

## Geometry and supports

Later model volumes clip earlier material; negative volumes subtract from material already present.
Modifiers split existing slices without creating material. Equivalent settings are interned. Empty
layers above the remaining model are removed after subtraction. Positive XY compensation is applied
before negative clipping; shrinking and first-layer compensation trim the combined object outline.

Compatible wall regions share perimeter generation, choosing the greatest density for gap handling.
Fill areas are then clipped back to each region. Top/bottom and vertical-shell detection use the whole
object's neighbouring slices and shared fill holes, avoiding false top/bottom caps at region changes.
Fill grouping is per surface type, tool, pattern, flow and speed settings: a sparse-density change does
not split compatible solid skins. Equal-angle bridge areas and equal-thickness combined infill merge.
Lightning retains its region-specific prepared forest. Paths are generated in bounded layer batches.
Only one volume's slices and one region's temporary shell arrays are prepared at a time.

`infill_every_layers` intersects sparse areas across fixed groups, limits their total thickness to the
nozzle diameter, clears the lower layers and emits the combined area at the last layer's full thickness.
Periodic solid infill above sparse infill is classified as internal bridging. Automatic infill
combination is not implemented.

Support enforcers/blockers are sliced on the object grid and passed as `SupportInput.masks`. Blockers
remove automatic contacts; enforcers add unsupported contacts after blocker and bridge exclusion.
Enforcers work with manual support mode, but do not enable supports by themselves. Normal, tree and
organic supports share this contact detection. Normal enforced columns retain the upstream exception
allowing a model landing when build-plate-only is selected. Bottom contacts intersect exposed top
surfaces rather than vertical walls, preventing false contact slabs inside narrow support columns.

## Oracle results, 2026-10-16

Original synthetic fixtures are generated by `scripts/make-regions-oracle.py`. Reference projects,
G-code, logs and comparisons stay in a system temporary directory; no private project or reference
output is in the repository. `scripts/compare-regions.py` measures deposited filament after
retraction repayment, by role and 5 mm slicing-plane band. It freezes the first layer-height tag:
later `HEIGHT` tags can describe combined infill or bridge extrusion thickness.

PrusaSlicer 2.9.6 used CORE One HF0.4, `0.20mm SPEED @COREONE HF0.4`, Prusament PLA and text output.
Mica used its corresponding default profiles. Percentages are Mica/reference minus one.

| Fixture | Sparse infill | Other role results | 5 mm bands |
|---|---:|---|---|
| Plain 20 mm cube | +0.000% | Within 0.004% | All within ±2% |
| Middle 10 mm modifier, 100% infill | −0.072% | Within 0.004% | All within ±2% |
| Range 5–10 mm, 4 walls and gyroid | +0.126% | Within 0.050% | All within ±2% |
| Negative cylinder, radius 3 mm | +1.457% | Within 0.090% | All within ±2% |
| Object: 0.1 mm, 3 walls | **+2.466%** | Within 0.038% | Sparse +2.426–2.477% |
| 0.1 mm, combine 3, periodic solid 20 | +1.588% | Within 0.003% | All within ±2% |
| Supported roof, normal snug | +0.000% | Support −0.190%; interface +0.114% | See small solid exception below |
| Left-half support blocker | +0.000% | Support −1.630%; interface +0.404% | Same solid exception |
| Right-half manual enforcer | +0.000% | Support −1.669%; interface +0.448% | Same solid exception |

The 0.1 mm object's sparse excess also occurs on the legacy STL path with global settings, at exactly
+2.466%; it is an existing grid connection/anchor discrepancy. Each roof fixture's lower stem contains
1.2801 mm reference solid filament; Mica uses 1.2064 mm (−5.758%) in the 0–5 mm band, despite total solid
error of only −0.083%. These results do **not** satisfy a blanket ±2% role-and-range acceptance criterion.
No tolerance was widened to hide them. Tree/organic volume controls have geometric regression tests;
these numerical support oracles exercise normal snug support.

The first generated Prusa height-range fixture omitted `layer_height`; the reference CLI crashed while
reading it. The upstream reader requires that option for slicing. The generator now writes it explicitly,
and the corrected fixture and all subsequent Prusa runs succeeded.

### Real U1 projects

The measurements below are the original baseline. Later U1 parity work
restores bridge support across the holder's speed-only modifier, but still reports excessive bridge
width and does not claim holder parity. It also identifies and normalizes the saved reference's
filament settings, which differ from Snapmaker PLA.

The private `holder.3mf` (`MICA_PRIVATE_MODELS`) contains a thin `Generic-Cube` modifier setting six
wall/fill speeds to 50 mm/s. Mica sliced the original project successfully (525 layers, about 101 MiB peak resident memory
measured with `getrusage`, 25.78 seconds for the final release run). OrcaSlicer 2.4.1
sliced a temporary copy using the explicit four-slot machine/process/filament vectors documented in
[multitool.md](multitool.md), arrangement disabled and no tower. Geometry, object and part overrides were
preserved; only temporary global presets/plate mapping were normalized. No originals were modified.

| Holder role | Difference |
|---|---:|
| External perimeter | −2.002% |
| Perimeter | −2.587% |
| Sparse infill | −1.403% |
| Solid infill, including bottom surfaces | −3.121% |
| Top solid infill | +11.429% |
| Internal bridge | −57.033% |
| Brim | −9.360% |

The holder does not meet Orca role parity. Existing Orca/Mica shell, bridge, wall and adhesion differences
remain; applying the modifier's speed values alone does not resolve them.

In the private five-plate project (`cat.3mf`), objects 10, 13, 16 and 19 on plate 3 override
`enable_support=0`. Mica successfully sliced a temporary
subset containing those four instances translated to plate-local coordinates (21 layers). Orca rejected
that subset with “Loading of a model file failed” (exit 250); reference attempts stopped after this one
failure, so no comparison is claimed for it. Full multi-plate scheduling remains outside this change.
The inspected dense-painted and five-plates projects contain tool assignments but no additional process-setting
modifiers relevant to regions.

### Reproduction

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
python3 scripts/make-regions-oracle.py /tmp/mica-regions-oracle
out/ref/PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer \
  --datadir out/ref/datadir \
  --printer-profile 'Prusa CORE One HF0.4 nozzle' \
  --print-profile '0.20mm SPEED @COREONE HF0.4' \
  --material-profile 'Prusament PLA @COREONE HF0.4' --center 125,110 \
  --no-binary-gcode --export-gcode -o /tmp/mica-regions-oracle/modifier-ref.gcode \
  /tmp/mica-regions-oracle/modifier.3mf
./target/release/mica slice /tmp/mica-regions-oracle/modifier.3mf \
  -o /tmp/mica-regions-oracle/modifier-mica.gcode
python3 scripts/compare-regions.py /tmp/mica-regions-oracle/modifier-ref.gcode \
  /tmp/mica-regions-oracle/modifier-mica.gcode
```

## Validation

`cargo fmt --all --check`, `cargo clippy --offline --workspace --all-targets -- -D warnings`,
`cargo test --offline --workspace`, `CARGO_NET_OFFLINE=true cargo xtask provenance`, and the offline
release CLI build passed. The workspace run completed 404 tests with zero failures and one ignored
test. Provenance checked 176 Rust files, including 134 ported/adapted files.

The new regressions cover configuration inheritance, modifier boundaries, compatible solid skins,
ordered booleans, explicit range heights and index dialects, fixed-interval infill, normal/organic
support masks, normal enforced model landings, batched region/tool assignments and G-code speed/tool
settings. Existing workspace geometry, support, profile and multi-tool tests also pass.

## Scope and review

The adapted modules register pinned Prusa `PrintApply`, `PrintObjectSlice`, `PrintObject`, `Layer`,
`Fill`, `Slicing` and support logic, plus Prusa/Orca range XML import, in `PROVENANCE.md`. The
synthetic fixture generator, comparison script, sparse CLI adapter and integration tests are original.

Facet painting and material segmentation are now implemented in [painting.md](painting.md).
Virtual extruders, adaptive height profiles, automatic infill combination,
interface shells between materials, configuration migration and multi-plate scheduling are excluded.
Seam planning still uses the object's plan; region scarf parameters do not get independent seam plans.
Tree routing retains the existing simplified search and build-plate landing policy. Region flows with
different role nozzle diameters and soluble support interfaces are not validated; all oracle nozzles
are 0.4 mm. Supports and adhesion use object settings, not parameter-modifier overrides.

Review configuration precedence and scope, compatibility keys used to merge walls/fills, whole-object
shell propagation, range index mapping, support contact projection and region/tool export assignments
closely. The remaining numerical oracle differences above are open acceptance issues, not passing parity.
