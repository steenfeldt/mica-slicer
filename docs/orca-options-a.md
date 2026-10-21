# Orca options A: dimensions and small features

Implemented against `orca@8500fcdcca`; runtime comparisons use the installed OrcaSlicer **2.4.1**.
No dependencies added. All options are typed, imported from Orca/Snapmaker profiles, editable in the GUI
and available through CLI `--set key=value`. The GUI tables were regenerated from pinned upstream labels
and tooltips with `cargo xtask gen-option-text`.

## API and configuration

`PrintSettings::small_features` contains `small_features::SmallFeatureSettings`.
`SmallFeatureSettings::from_config` resolves and validates the options; `FlowCompensation::new` accepts
custom `(length_mm, multiplier)` points and `multiplier` evaluates Orca's PCHIP curve.
`small_features::compensate` exposes the independent polygon operation.

| Key | Mica / Prusa default | Orca profile behavior |
|---|---|---|
| `xy_hole_compensation` | 0 mm | Positive expands holes; negative contracts them |
| `xy_contour_compensation` | 0 mm | Positive expands the outside; negative contracts it |
| `small_perimeter_threshold` | 6.5 mm | Source default 0 disables it; limit is **2π × radius**, despite the tooltip saying length |
| `small_perimeter_speed` | Existing Prusa setting | Percentages use outer-wall speed for Orca; zero means 50% of that speed |
| `initial_layer_min_bead_width` | 0%, meaning use shared `min_bead_width` | Source default 85%; percentage of the nozzle diameter, used only for layer 0 Arachne walls |
| `filter_out_gap_fill` | 0 mm, disabled | Reject continuous gap paths shorter than the threshold, before width splitting |
| `small_area_infill_flow_compensation` | false | Opt in to the length/flow model |
| `small_area_infill_flow_compensation_model` | Orca's ten-point curve | String vector of `length,multiplier` pairs; CLI spelling uses semicolons between pairs |

Example, after the usual U1 profile arguments:

```sh
mica slice model.stl ... -o /tmp/part.gcode \
  --set xy_hole_compensation=0.1 --set xy_contour_compensation=-0.05 \
  --set small_perimeter_threshold=4 --set small_perimeter_speed=25% \
  --set initial_layer_min_bead_width=125% --set filter_out_gap_fill=1 \
  --set small_area_infill_flow_compensation=1
# Optional replacement curve:
# --set 'small_area_infill_flow_compensation_model=0,0;1,0.75;3,1'
```

Prusa's independent `xy_size_compensation` remains available and is applied first. The Orca contour
mapping no longer aliases it. Orca compensation is object-scoped and disabled for material-painted
objects when more than one filament slot exists; Prusa XY compensation keeps its existing behavior.
Compensation uses the merged layer silhouette, then grows/clips region masks with first-region priority,
so internal material boundaries do not become compensated contours or holes. A 5 µm Douglas–Peucker
cleanup before/after the Orca offsets prevents tiny slice-closing artifacts from becoming miter spikes.
This cleanup never runs when the two Orca dimension controls are zero.

Unclipped perimeter length is carried through seam placement, variable-width runs and overhang/cooling
splits. Orca loop thresholds do not slow open thin-wall paths; imported zero speed means automatic
speed despite the upstream UI minimum. Modifier grouping retains separate wall and solid-fill settings. Default Prusa speed selection retains its existing length calculation and output. First-layer bead
minimum changes the beading strategy without changing subsequent layers or classic walls.

The flow model validates finite, increasing length/flow points, first length 0, last factor 1, and at least
two points. It uses Orca's secant endpoint slopes and weighted harmonic interior slopes. Length 0 and
lengths beyond the model return full flow. It operates on eligible solid/top roles and the selected
rectilinear/aligned/monotonic/monotonic-lines patterns. Each fitted source line or arc gets one factor,
retained through cooling subdivision. Sparse infill, walls, bridges, gap fill and ironing are excluded.
The writer accounts for the actual compensated extrusion, including per-tool totals.

## Source verification and scope

- `PrintObjectSlice.cpp::PrintObject::_shrink_contour_holes` and `PrintObject::slice`: independent
  contour/hole operations, positive/negative passes and painted-material exclusion.
- `GCode.cpp::GCode::extrude_loop` and `libslic3r.h::SMALL_PERIMETER_LENGTH`: radius threshold,
  outer-speed percentage base, auto speed, and exclusion of bridge/overhang roles.
- `Arachne/WallToolPaths.cpp::make_paths_params`: separate first-layer minimum bead width.
- `PerimeterGenerator.cpp::PerimeterGenerator::process_classic` and
  `Fill/FillBase.cpp::Fill::_create_gap_fill`: length rejection before variable-width conversion.
  The older `FillMonotonicLineWGapFill::fill_surface_extrusion` block in `FillRectilinear.cpp` cited by
  the survey is commented out at the pin; its gap logic has moved to `FillBase`.
- `GCode/SmallAreaInfillFlowCompensator.cpp`, `GCode/PchipInterpolatorHelper.cpp` and
  `GCode.cpp::GCode::_needSAFC`, `GCode::_extrude`: curve validation/interpolation and emission.
  The actual exporter evaluates each source **line/arc**, rather than the entire connected fill path.
  Alexander Þór's upstream attribution is retained.

Review the following limits closely:

- Polygon math remains Mica's Clipper2 implementation, with the explicit 5 µm cleanup above. The
  dimensional oracle below passes; this is not a wholesale port of Orca's slice-volume machinery.
- Gap filtering covers classic wall gap fill and the existing Ensuring fill's reconstructed gaps.
  Mica does not acquire Orca's general `FillBase` surface-gap generation in this batch, and does not
  filter ordinary narrow concentric solid fill as if it were gap fill. Classic Benchy gap-role totals
  still differ substantially from Orca; those results are reported, not accepted as parity.
- Mica folds bottom solid fill into `SolidInfill`; it does not add Orca's separate bottom-surface role.
  First-layer bottom-pattern eligibility is handled, but later bottom surfaces share the internal-solid
  pattern gate. The oracle uses the standard monotonic bottom/rectilinear internal patterns.
- Bead percentages use Mica's resolved nozzle for the active tool; unlike upstream's global minimum
  across all nozzles, this is not a mixed-nozzle-diameter extension. The U1 fixture uses four 0.4 mm tools.
- No changes to the general cooling, resonance-avoidance, shell or bridge policies are included. Post
  speed comparisons disable cooling slowdown and resonance avoidance to isolate this option.

## Reproduction

The original fixtures and the oracle command generator use only Python's standard library. The plate
is a watertight 48 × 16 × 3 mm mesh with 3/5/8 mm through holes; the posts are 3/5/8 mm cylinders.
Circular meshes have 192 facets. Reference G-code, flattened settings, logs and generated meshes stay
in the system temporary directory.

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --offline --release -p mica-cli
ref=$(mktemp -d /tmp/mica-orcaopts-a.XXXXXX)
python3 tests/oracles/orca_options_a.py "$ref" --include-off
sh "$ref/commands.sh"
python3 tests/oracles/orca_options_a_metrics.py "$ref" --check > "$ref/measurements.json"
```

The script loads only the Orca filament library and Snapmaker vendor, resolving that vendor's base names
before flattening. It emits every exact CLI command, including four explicit `--load-filaments` slots.
`--run-orca` runs the Orca commands directly and stops on the first failed attempt; the shell script also
stops on failure. No Orca run crashed.

The five on-cases are:

| Case | Model | Non-default controls |
|---|---|---|
| xy | Hole plate | hole +0.1 mm, contour −0.05 mm, elephant foot off, classic, arcs off |
| posts | Small cylinders | threshold 4 mm, speed 25%, classic, cooling slowdown and resonance avoidance off |
| bead | Benchy | first-layer bead minimum 125%, Arachne |
| gap | Benchy | gap threshold 1 mm, classic |
| flow | Benchy | compensation on, default curve, arcs off |

Hole boundary estimates use outer-wall centerline X/Y extents minus the declared 0.42 mm bead width,
at Z=1.05 mm (above elephant-foot effects). These are G-code measurements, not physical print tests.

| Nominal hole | Orca X / Y (mm) | Mica X / Y (mm) | Largest difference |
|---|---|---|---|
| 3 mm | 3.194 / 3.194 | 3.194 / 3.194 | 0.000 mm |
| 5 mm | 5.195 / 5.195 | 5.200 / 5.195 | 0.005 mm |
| 8 mm | 8.196 / 8.198 | 8.196 / 8.197 | 0.001 mm |

Both slicers emit **50 mm/s** inner/outer post walls at Z=1.05 mm with the isolated speed controls.

## Filament by role

Values are positive extrusion on XY moves, in mm of 1.75 mm filament. Retract/unretract-only moves
are excluded; the startup purge is reported separately as Custom. Orca reports Bottom surface separately;
compare its **bottom + internal solid** sum with Mica internal solid. Missing roles are zero. These tables
are measurements, not a claim of complete per-role parity.

### xy

| Role | Orca (mm) | Mica (mm) |
|---|---:|---:|
| Bottom surface | 53.164 | 0.000 |
| Custom | 15.000 | 15.000 |
| Inner wall | 88.990 | 96.060 |
| Internal Bridge | 60.911 | 61.889 |
| Internal solid infill | 222.055 | 279.771 |
| Outer wall | 86.510 | 86.873 |
| Sparse infill | 59.692 | 59.444 |
| Top surface | 50.614 | 43.800 |

### posts

| Role | Orca (mm) | Mica (mm) |
|---|---:|---:|
| Bottom surface | 3.738 | 0.000 |
| Custom | 15.000 | 15.000 |
| Inner wall | 49.407 | 51.670 |
| Internal Bridge | 3.684 | 3.762 |
| Internal solid infill | 42.859 | 52.037 |
| Outer wall | 57.303 | 57.238 |
| Sparse infill | 18.179 | 18.376 |
| Top surface | 5.049 | 3.596 |

### bead

| Role | Orca (mm) | Mica (mm) |
|---|---:|---:|
| Bottom surface | 20.862 | 0.000 |
| Bridge | 47.867 | 48.695 |
| Brim | 0.960 | 1.199 |
| Custom | 15.000 | 15.000 |
| Inner wall | 1146.113 | 1152.164 |
| Internal Bridge | 165.517 | 149.298 |
| Internal solid infill | 702.473 | 725.792 |
| Outer wall | 1006.971 | 1006.362 |
| Overhang wall | 5.946 | 5.926 |
| Sparse infill | 564.031 | 571.740 |
| Top surface | 105.519 | 104.811 |

### gap

| Role | Orca (mm) | Mica (mm) |
|---|---:|---:|
| Bottom surface | 20.028 | 0.000 |
| Bridge | 46.955 | 48.592 |
| Brim | 0.960 | 1.199 |
| Custom | 15.000 | 15.000 |
| Gap infill | 226.986 | 155.997 |
| Inner wall | 982.926 | 1037.575 |
| Internal Bridge | 162.289 | 145.164 |
| Internal solid infill | 621.640 | 720.413 |
| Outer wall | 1011.986 | 1016.673 |
| Overhang wall | 5.556 | 0.000 |
| Sparse infill | 555.511 | 578.659 |
| Top surface | 105.243 | 111.572 |

### flow

| Role | Orca (mm) | Mica (mm) |
|---|---:|---:|
| Bottom surface | 18.135 | 0.000 |
| Bridge | 47.772 | 48.694 |
| Brim | 0.960 | 1.199 |
| Custom | 15.000 | 15.000 |
| Inner wall | 1146.925 | 1152.211 |
| Internal Bridge | 164.955 | 149.299 |
| Internal solid infill | 633.068 | 653.025 |
| Outer wall | 1006.436 | 1006.253 |
| Overhang wall | 5.949 | 5.927 |
| Sparse infill | 563.892 | 571.720 |
| Top surface | 99.650 | 98.982 |

## Option on/off checks

Additional Benchy runs changed only the named option, using the same per-case settings.

| Effect | Orca off → on (mm filament) | Mica off → on (mm filament) |
|---|---:|---:|
| Gap role, threshold 0 → 1 mm | 235.943 → 226.986 | 156.773 → 155.997 |
| Solid + bottom, flow off → on | 722.429 → 651.203 | 725.840 → 653.025 |
| Top surface, flow off → on | 105.525 → 99.650 | 104.811 → 98.982 |

For the flow toggle, walls, bridges, sparse infill, brim and startup extrusion are unchanged in
each slicer. The different gap totals and the remaining bridge/shell allocations need separate
parity work; the filter does not supply missing surface gap paths.

## Validation and regressions

- `cargo fmt --all` and the final format check: pass.
- `cargo clippy --offline --workspace --all-targets -- -D warnings`: pass.
- `cargo test --offline --workspace`: pass (with the pinned Prusa profiles). Unit tests cover the circular offset regression,
  nonlinear PCHIP values, arc/cooling subdivision, first-layer-only beading, signed offsets, small-loop
  speed/auto/percentage handling, mapping in both Orca dialects, painted-material exclusion and independent modifier settings.
- `cargo xtask provenance`: pass (288 files, 168 ported/adapted).
- `cargo xtask corpus --quick`: all 11 cases pass, each with 0 validation errors.
- `mica validate`: 0 errors for all five on-cases, both off-cases, and the four Prusa baseline outputs.
  Profile arguments must match the output's printer. Existing bridge-height warnings remain. The flow-on
  output has 6,617 warnings, predominantly expected flow-consistency warnings because the validator's
  geometric bead check assumes nominal flow; it does not interpret this new empirical model.
- `cmp`: **byte-identical text G-code** before/after for cube20 and 3DBenchy on both CORE One HF0.4 SPEED and
  XL **5T Input Shaper 0.4**, 0.20mm SPEED, Prusament PLA. These CLI comparisons used installed Prusa
  profiles 2.5.11. The separate 2T startup template references absent tool slots; the successful XL
  comparison uses the 5T profile. No startup-template work is included here.

The reference comparison, per-role tables and commands are review artifacts. Physical print validation,
complete Orca surface-gap generation and exact bottom-role pattern distinction remain outside this batch.
