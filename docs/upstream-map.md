# Upstream map

Where each feature's logic lives upstream, which upstream to port it from, and what it costs.
Surveyed 2026-10-15 against the commits pinned in [`../upstream/UPSTREAM.lock`](../upstream/UPSTREAM.lock).
Paths are relative to each upstream repository root; every path was checked to exist at the pinned commit.

Ids: **ps** = `prusaslicer` (2.9.6), **orca** = `orca` (2.4.2), **sm** = `snapmaker-orca` (2.4.0),
**bg** = `libbgcode`. Sizes are upstream C++ lines (`wc -l`), a rough measure of porting effort.
Difficulty: **S** days · **M** a week or two · **L** several weeks · **XL** a month or more.

## Ground rules from the survey

1. **PrusaSlicer is the skeleton; Orca supplies features.** Prusa's code is consistently cleaner: small
   modules, mostly pure functions, explicit parameter structs. Orca (via Bambu Studio) has more
   user-facing features but reads config through raw pointers deep in algorithms, keeps state in objects,
   and carries dead code. Port structure from ps, then cherry-pick orca features onto it.
2. **Take only profiles and small U1 tweaks from Snapmaker's fork.** sm sits on an *older* Orca base
   (`SLIC3R_VERSION 01.10.01.50` vs orca `02.06.00.51`), and its own additions (mixed filament, local Z,
   pointillism) bloat core files: its `PrintObjectSlice.cpp` is 5,660 lines vs orca's 1,598.
3. **The U1 profile comes from orca, not sm.** Upstream Orca ships a *newer* U1 profile (tool-change
   G-code dated 2026-06-07 vs 2025-12-13, by-object Z handling, higher machine limits).
4. **Port the data model from ps.** `ExtrusionPath` + `ExtrusionAttributes` (with optional overhang
   attributes) and a bit-flag `ExtrusionRole`. Orca bakes `overhang_degree`, `fitting_result` and 3D
   polylines into its geometry types, which leaks into every split, reverse and append.

## Architecture consequences

These change Mica's design, not just effort:

- **A sliding window of layers, not strictly one layer at a time.** Top/bottom shell discovery,
  surface typing (layer above and below), internal bridges, seams and curled-edge detection all look at
  neighbouring layers; supports look at the whole column. Each such stage is its own pass over a window
  of layers. Upstream keeps *every* layer with all intermediate data alive for the whole slice, plus
  every G-code move for the preview afterwards. Those are the two big memory consumers to avoid.
  Surface typing and shells (`mica-core/src/shells.rs`) are whole-object passes that keep only each
  layer's typed infill areas, about the size of the outlines; walls and toolpaths are then made in batches
  (the walls are computed twice, once for the infill areas and once for the toolpaths).
- **Two-pass output.** Generate G-code, then post-process it. Prusa XL tool preheating (`M104.1`), `M73`
  progress lines and time estimates all come from a time simulation of the finished G-code
  (ps `GCodeProcessor::post_process`). The generator stays streaming; the post-pass streams too.
- **Polygon library must carry per-vertex data.** Arachne carries line width through clipping
  (ClipperLib_Z), and offsets must match Clipper's join types and miter limits or output drifts.
  Upstream uses an old, modified Clipper 1 (6.4.2) everywhere; Clipper2 is new ground, so polygon
  results need regression tests against Clipper 1 output. Candidate: `clipper2-rust` (pure Rust, BSL-1.0);
  verify its Z/user-data support in an M0 spike.
- **A placeholder-language interpreter is required early.** Real printer profiles run custom G-code
  (start, end, tool change, layer change) through ps/orca's macro language, and that code writes state
  *back* to the generator (`position`, `e_retracted`, `zhop`). Both the U1 and XL profiles depend on it.
- **One canonical settings schema with alias tables.** The same setting has different names in Prusa
  and Orca (`perimeters` vs `wall_loops`, `retract_lift` vs `z_hop`, `complete_objects` vs
  `print_sequence`). Mica picks one name per setting and maps both at import.
- **Thumbnails use a CPU renderer.** Upstream renders them with the GUI's OpenGL canvas. Mica now
  renders the placed mesh in Rust with the same orthographic camera; see [`thumbnails.md`](thumbnails.md).

## Rust equivalents for upstream dependencies

| Upstream | Used for | Rust |
|---|---|---|
| Clipper 6.4.2 (modified) | Polygon booleans and offsets | `clipper2-rust` (to verify in M0) |
| Boost.Polygon Voronoi | Medial axis (thin walls), Arachne | `boostvoronoi` (pure-Rust port, BSL-1.0) |
| Eigen | Vectors, matrices, transforms | `glam` or `nalgebra` |
| TBB | Parallel loops, pipelines | `rayon` + channels |
| admesh | STL load and repair | Port (ps `bundled_deps/admesh`) |
| expat, miniz | 3MF XML and zip | `quick-xml`, `zip` |
| nlohmann json, boost property_tree | Orca JSON / Prusa INI profiles | `serde_json`, own INI parser |
| boost::spirit | Placeholder language grammar | Hand-written parser |
| zlib, heatshrink | bgcode compression | `flate2`, port heatshrink (ISC) |

Not needed for core FDM: CGAL, libigl, OpenVDB, mcut (mesh booleans, cut, hollowing), OCCT (STEP),
OpenCV, draco, libseqarrange + Z3 (Prusa's sequential-print arranger).

---

## Core pipeline and formats

| Feature | Port from | Key upstream files | Size | Diff. |
|---|---|---|---|---|
| STL load + repair | ps | `src/libslic3r/Format/STL.cpp`, `src/libslic3r/TriangleMesh.cpp` (`trianglemesh_repair_on_import`), `bundled_deps/admesh/admesh/*` | ~4.5k | S–M |
| Mesh → layer outlines | ps (orca's copy reads easier) | `src/libslic3r/TriangleMeshSlicer.cpp` (`slice_mesh_ex`, `make_loops`, `make_expolygons`), `fix_slicing_errors` in `PrintObjectSlice.cpp` | ~2.7k | M |
| Pipeline / steps | ps, rebuilt for streaming | `src/libslic3r/Print.cpp` (`Print::process`), `PrintObject.cpp`, `PrintBase.hpp` | ~7k | L |
| Regions + modifiers + height ranges | ps | `src/libslic3r/PrintApply.cpp` (`generate_print_object_regions`), `PrintObjectSlice.cpp` (`slices_to_regions`), `Print.hpp` (`PrintObjectRegions`) | ~4k | L |
| Painting storage + MMU segmentation | ps | `src/libslic3r/TriangleSelector.cpp`, `MultiMaterialSegmentation.cpp` | ~5k | L |
| Settings reference | orca names, ps design | `src/libslic3r/PrintConfig.cpp` (orca 871 options, ps 594), `Config.hpp` | reference | M |
| Orca/U1 JSON profiles | **orca** | `resources/profiles/Snapmaker.json`, `resources/profiles/Snapmaker/{machine,process,filament}/` (single `inherits`) | data | S |
| Prusa INI profiles | ps | `resources/profiles/PrusaResearch.ini`: XL models ~L215, Core One models ~L10; multiple inheritance and `compatible_printers_condition` | data | M |
| 3MF, Orca dialect | orca | `src/libslic3r/Format/bbs_3mf.cpp`: `Metadata/model_settings.config`, `project_settings.config`, production-extension object files | ~9.5k | L |
| 3MF, Prusa dialect | ps | `src/libslic3r/Format/3mf.cpp`: `Metadata/Slic3r_PE.config`, `Slic3r_PE_model.config` | ~4.5k | M |
| Polygon API | ps API on Clipper2 | `src/libslic3r/ClipperUtils.cpp` (`union_ex`, `diff_ex`, `offset2_ex`, `opening`/`closing`) | ~2.2k | M |
| Adaptive / variable layer height | ps | `src/libslic3r/SlicingAdaptive.cpp`, `Slicing.cpp` | ~1.1k | S |

Compatibility: Orca reads Prusa 3MF fully. Prusa reads Orca geometry but drops modifiers and per-object
settings. Mica imports both.

## Walls, surfaces, infill, quality

| Feature | Port from | Key upstream files | Size | Diff. |
|---|---|---|---|---|
| Fixed-width walls, gap fill | ps + orca wall order | ps `src/libslic3r/PerimeterGenerator.cpp` (`process_classic`, `traverse_loops_classic`) | ~650 | M |
| Thin walls (medial axis) | ps | `src/libslic3r/Geometry/MedialAxis.cpp`, `Geometry/Voronoi.cpp` | ~1.3k | M |
| Variable-width walls (Arachne) | ps | `src/libslic3r/Arachne/*` (SkeletalTrapezoidation, WallToolPaths, BeadingStrategy, PerimeterOrder), `process_arachne` | ~9k | XL |
| Seams | ps (2.8+ rewrite) | `src/libslic3r/GCode/Seam*.cpp` (Placer, Aligned, Rear, Random, Perimeters, Geometry, Scarf), `GCode/ModelVisibility.cpp` | ~4.5k | L |
| Overhangs (speed, fan, curl) | ps | `src/libslic3r/GCode/ExtrusionProcessor.{hpp,cpp}` (`calculate_overhang_speed`), `PrintObject.cpp` (`calculate_overhanging_perimeters`), `SupportSpotsGenerator.cpp` (`estimate_malformations` only) | ~1.5k | M–L |
| Extra perimeters on overhangs | ps | `PerimeterGenerator.cpp` (`generate_extra_perimeters_over_overhangs`) | ~400 | M |
| Bridges + internal bridges | ps + orca options | `src/libslic3r/BridgeDetector.cpp`, `LayerRegion.cpp` (`process_external_surfaces`, ported with bridge directions), `Algorithm/RegionExpansion.cpp` (ported), `PrintObject::bridge_over_infill` (ported; [oracle and scope](bridges.md)) | ~2.3k | L |
| Top/bottom + vertical shells | ps | `PrintObject.cpp`: `detect_surfaces_type`, `discover_vertical_shells`, `discover_horizontal_shells` (ported, `shells.rs`), `combine_infill` (not yet) | ~1.3k | L |
| Infill base + anchoring | ps | `src/libslic3r/Fill/FillBase.cpp` (`connect_infill`), `Fill/Fill.cpp` (`make_fills`) | ~1.3k | L |
| Rectilinear/monotonic/grid/cubic | ps | `src/libslic3r/Fill/FillRectilinear.cpp` | ~3.1k | L |
| Concentric, gyroid, honeycomb | ps | `Fill/FillConcentric.cpp`, `FillGyroid.cpp`, `FillHoneycomb.cpp` | ~500 | S |
| Lightning, adaptive cubic | ps | `Fill/FillLightning.cpp` + `Fill/Lightning/*`, `Fill/FillAdaptive.cpp` (ported with documented differences; [oracle and scope](infill-finishing.md)) | ~3.3k | M–L |
| Orca-only patterns (TPMS, cross-hatch, …) | orca | `Fill/FillTpmsD.cpp`, `FillTpmsFK.cpp`, `FillCrossHatch.cpp`, extras in `FillRectilinear.cpp` | ~1.5k | M |
| Ironing | ps + orca | `Layer::make_ironing` in `Fill/Fill.cpp` (ported; [oracle and scope](infill-finishing.md)) | ~230 | S |
| Fuzzy skin | ps, then orca noise modes | `src/libslic3r/Feature/FuzzySkin/FuzzySkin.cpp` | 256 / 839 | S–M |
| Arc fitting | ps | `src/libslic3r/Geometry/ArcWelder.cpp`, `GCode/SmoothPath.cpp` | ~1.8k | M |
| Brim + skirt | ps, orca ears later | `src/libslic3r/Brim.cpp`, `Print::_make_skirt` | ~900 | M |

**Orca features to cherry-pick** (mostly S): inner-outer-inner wall order, precise outer wall,
only one wall on top surfaces (`split_top_surfaces`), reverse walls on overhangs, configurable small
perimeter threshold, hole-to-polyhole, small-area infill flow compensation
(`GCode/SmallAreaInfillFlowCompensator.cpp`), conical "make overhang printable", gap-fill filtering.

## Supports

All from **ps**. Orca's support code is Prusa's with Bambu changes on top, and Orca's two tree engines
call into each other, so neither can be ported alone.

| Feature | Key upstream files (ps) | Size | Diff. |
|---|---|---|---|
| Shared support back end (layers, interfaces, raft, toolpaths) | `src/libslic3r/Support/SupportCommon.cpp`, `SupportParameters.cpp`, `SupportLayer.hpp` | ~2.6k | L |
| Normal (grid/snug) supports | `src/libslic3r/Support/SupportMaterial.cpp` (`PrintObjectSupportMaterial`) | ~2.8k | L |
| Enforcers, blockers, painted supports | `PrintObjectSlice.cpp` (`slice_support_volumes`), `PrintObject::project_and_append_custom_facets` | ~300 | M |
| Support/interface on another tool | `SupportParameters.cpp`, `GCode/ToolOrdering.cpp`, `GCode/ExtrusionOrder.cpp` | ~400 | M |
| Tree supports | `src/libslic3r/Support/TreeSupport.cpp`, `TreeModelVolumes.cpp`, `TreeSupportCommon.cpp` | ~6.2k | XL |
| Organic supports | `src/libslic3r/Support/OrganicSupport.cpp` (re-slices a generated branch mesh) | ~1.5k | L |

**Order:** shared back end (gives rafts) → normal snug → normal grid → enforcers/blockers →
separate support tool → tree → organic. Test oracle: ps `tests/fff_print/test_support_material.cpp`.
Tree-support files carry a CuraEngine/Thomas Rahm attribution header (AGPLv3); keep it in the port.

## G-code, multi-tool, output

| Feature | Port from | Key upstream files | Size | Diff. |
|---|---|---|---|---|
| Generator core (travel, retraction, wipe, layer change) | ps | `src/libslic3r/GCode.cpp` (`process_layer`, `_extrude`, `travel_to`, `set_extruder`), `GCode/GCodeWriter.cpp`, `GCode/Travels.cpp`, `GCode/Wipe.cpp`, `GCode/ExtrusionOrder.cpp` | ~8k | XL |
| Avoid crossing perimeters | ps | `src/libslic3r/GCode/AvoidCrossingPerimeters.cpp` | ~1.7k | L |
| Cooling, fan, layer-time slowdown | ps | `src/libslic3r/GCode/CoolingBuffer.cpp` | ~1.4k | M |
| Pressure equalizer, spiral vase | ps | `GCode/PressureEqualizer.cpp`, `GCode/SpiralVase.cpp` | ~1k | M |
| Klipper flavor | orca | orca `src/libslic3r/GCodeWriter.cpp` (`SET_VELOCITY_LIMIT`, `SET_PRESSURE_ADVANCE`) | small | S |
| Prusa flavor | ps | `GCode/GCodeWriter.cpp` (`M204 P/T`, `M205`), machine limits | small | S |
| Object labels (cancel object) | ps | `src/libslic3r/GCode/LabelObjects.cpp` (`M486` and `EXCLUDE_OBJECT`) | ~300 | S |
| Placeholder language | ps | `src/libslic3r/PlaceholderParser.cpp` + variables set in `GCode.cpp` | ~2.5k | L |
| Tool ordering | ps | `src/libslic3r/GCode/ToolOrdering.cpp` | ~1.1k | M |
| Prime/wipe tower | ps | `src/libslic3r/GCode/WipeTower.cpp`, `GCode/WipeTowerIntegration.cpp` | ~2k | L |
| Tool preheat (post-pass) | orca generalization of ps | ps `GCodeProcessor::post_process` (`M104.1` backtrace, XL only); orca makes `preheat_time`/`preheat_steps` configurable for any multi-tool printer (U1) | ~800 | M |
| U1 code tweaks | sm | `src/libslic3r/GCode/WipeTower2.cpp` (`is_snapmaker_u1`: `M220 B/R`, PA reset), preheat delta temperature | small | S |
| Time estimate + M73 progress | ps + orca Klipper commands | ps `src/libslic3r/GCode/GCodeProcessor.cpp` (planner, `process_G1`, `process_G2_G3`, M-code handlers, `post_process`); orca `process_SET_VELOCITY_LIMIT` | ~3.5k | L |
| Binary G-code | ps + bg | bg `src/LibBGCode/core/*`, `binarize/*` (incl. MeatPack), spec `doc/specifications.md`; ps `GCode.cpp` metadata assembly | ~2.5k | M |
| Thumbnails + metadata comments | ps | `src/libslic3r/GCode/Thumbnails.cpp`, `PrintStatistics` in `Print.cpp` | ~400 | S |
| Sequential printing, pause, color change | ps emission, orca clearance check | ps `GCode.cpp` (`complete_objects` loop), `CustomGCode.cpp`; orca `Print::sequential_print_clearance_valid` | ~1k | M |
| Upload | ps PrusaLink, orca Moonraker | ps `src/slic3r/Utils/OctoPrint.cpp` (`PrusaLink`), orca `src/slic3r/Utils/Moonraker.cpp` | ~1.5k | S–M |

**Printer facts the survey turned up**

- **U1:** Klipper; 4 tools; ooze prevention with 30–60 s preheat; tool change is
  `M109 S… T<n>` → `T<n>` → `SM_PRINT_PREEXTRUDE_FILAMENT`, with Klipper macros doing the pick and park;
  thumbnails 48×48 and 300×300 PNG; uploads over plain Moonraker HTTP.
- **XL:** `binary_gcode = 1`; wipe tower on; per-filament idle temperature; preheat via `M104.1`
  (enabled only when `printer_notes` contains `PRINTER_MODEL_XL`); `M486` object labels; thumbnails
  16×16, 313×173 and 480×240 QOI plus 380×285 PNG.
- **Core One:** `binary_gcode = 1`; profile variants in `PrusaResearch.ini` include COREONE, COREONEL,
  COREONEMMU3 and COREONEOAK.
- The U1's `change_filament_gcode` is adapted from the XL's `toolchange_gcode`, so one placeholder
  interpreter and one tool-change model cover both.

## Total volume

Roughly **110–120k lines of upstream C++** sit behind the full parity checklist (excluding the GUI).
Rust ports are usually more compact, and not all of it is needed at once. Per milestone:

| Milestone | Main upstream pieces | Approx. upstream lines |
|---|---|---|
| M0 cube | STL load, mesh slicer, polygon API, simple walls/infill, minimal writer | ~8k (much simplified) |
| M1 Benchy ×3 | settings + profile import, placeholder language, top/bottom, infill base + 3 patterns, generator core, Klipper + Prusa flavors, cooling | ~25k |
| M2 quality | overhangs, extra overhang perimeters, bridges, seams, arc fitting, avoid crossing perimeters | ~12k |
| M3 multi-tool | tool ordering, wipe tower, preheat post-pass, time estimate, labels, thumbnails, bgcode | ~11k |
| M4 supports | shared back end, normal, enforcers/blockers, tree, organic | ~14k |
| M5 modifiers | regions, painting, MMU segmentation, 3MF both dialects | ~20k |
| M6 GUI | written fresh (wgpu); upload from ps/orca | ~1.5k ported + new GUI |
| M7 rest | Arachne, Lightning/adaptive/Orca infills, ironing, fuzzy skin, brim ears, sequential printing, Orca extras | ~18k |
