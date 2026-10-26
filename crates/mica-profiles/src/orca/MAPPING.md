# Orca → Mica mapping evidence

All rows refer to `PrintConfigDef::init_fff_params` / `init_common_params` in
`orca@8500fcdcca src/libslic3r/PrintConfig.cpp` and
`prusaslicer@b028299c77 src/libslic3r/PrintConfig.cpp`. The columns identify
the exact option definitions, their labels and units. Enum symbol tables and
Orca `handle_legacy` / `handle_legacy_composite` were checked separately.
No name-only automatic fallback is used. Unlisted/new options stay extras-only.

“identical” describes meaning and units, including lossless JSON/INI serialization.
“converted” requires a value/base/enum/selection conversion. A row can return
extras-only for a particular mode or rejected for an invalid value.

| Orca key | Canonical key | Class | Definition evidence / conversion |
|---|---|---|---|
| `initial_layer_infill_speed` | `first_layer_infill_speed` | identical | First-layer infill feedrate, mm/s. |
| `internal_bridge_speed` | `internal_bridge_speed` | engine | Independent internal bridge feedrate; percentages resolve against bridge_speed. |
| `min_width_top_surface` | `min_width_top_surface` | engine | Filter width relative to inner wall width; Orca 0.85-wall extension. |
| `minimum_sparse_infill_area` | `minimum_sparse_infill_area` | engine | Promote small sparse islands after external-surface expansion, in mm². |
| `only_one_wall_first_layer` | `only_one_perimeter_first_layer` | identical | Single first-layer wall for Arachne. |
| `only_one_wall_top` | `top_one_perimeter_type` | converted | Boolean selects top or none; BambuStudio's `top_one_wall_type = topmost` (which Orca's `handle_legacy` reads as on) selects topmost, and Mica writes it next to the switch so Topmost surface only round-trips. Arachne consumes the top-surface filter. |
| `thick_internal_bridges` | `thick_internal_bridges` | engine | Internal bridge thickness independent of external bridges. |
| `support_type` | `support_material_auto` (+ `support_material_style`) | converted | `is_auto`: `*(auto)` detects overhangs, `*(manual)` supports only enforcers (Prusa "enforcers only"); the style follows `SupportParameters` below. |
| `support_style` | `support_material_style` | converted | Orca `SupportParameters` (orca `src/libslic3r/Support/SupportParameters.hpp`): tree types use organic for `default`/`organic`/grid/snug and Mica's classic tree for `tree_slim`/`tree_strong`/`tree_hybrid`; normal types use grid for `default` and every tree style, else grid/snug. |
| `tree_support_branch_angle_organic` | `support_tree_angle` | identical | Organic branch angle, degrees (`TreeSupportMeshGroupSettings`, orca `src/libslic3r/Support/TreeSupportCommon.hpp`). |
| `tree_support_angle_slow` | `support_tree_angle_slow` | identical | Preferred branch angle, degrees. |
| `tree_support_branch_diameter_organic` | `support_tree_branch_diameter` | identical | Organic branch diameter, mm. |
| `tree_support_branch_diameter_angle` | `support_tree_branch_diameter_angle` | identical | Branch diameter growth angle, degrees. |
| `tree_support_branch_distance_organic` | `support_tree_branch_distance` | identical | Organic tip spacing, mm. |
| `tree_support_tip_diameter` | `support_tree_tip_diameter` | identical | Tip diameter, mm (also the enforcer overhang offset). |
| `tree_support_top_rate` | `support_tree_top_rate` | identical | Branch density under the roof, percent. |
| `tree_support_wall_count` | `support_tree_branch_diameter_double_wall` | converted | Orca `SupportParameters`: 0 doubles walls of islands larger than a 5 mm circle, 1 never, 2+ everywhere (0.001 mm threshold). Also sets `support_material_with_sheath` (wall count > 0), which Orca applies to normal supports. |
| `tree_support_branch_angle`, `tree_support_branch_diameter`, `tree_support_branch_distance` | `support_tree_angle`, `support_tree_branch_diameter`, `support_tree_branch_distance` | mapped when the effective style is Orca's classic tree (`tree_slim`/`tree_strong`/`tree_hybrid` → Mica `tree`); the `*_organic` keys map only otherwise. Sparse keys without `support_type` keep the organic values. Orca's classic generator itself (BBS `TreeSupport`) is not ported; Mica's tree style is Prusa's. |
| `filter_out_gap_fill` | `filter_out_gap_fill` | identical | Implemented; see [batch A behavior and oracle results](../../../../docs/orca-options-a.md). |
| `initial_layer_min_bead_width` | `initial_layer_min_bead_width` | identical | Implemented; see [batch A behavior and oracle results](../../../../docs/orca-options-a.md). |
| `small_area_infill_flow_compensation_model` | `small_area_infill_flow_compensation_model` | identical | Implemented; see [batch A behavior and oracle results](../../../../docs/orca-options-a.md). |
| `small_area_infill_flow_compensation` | `small_area_infill_flow_compensation` | identical | Implemented; see [batch A behavior and oracle results](../../../../docs/orca-options-a.md). |
| `small_perimeter_threshold` | `small_perimeter_threshold` | identical | Implemented; see [batch A behavior and oracle results](../../../../docs/orca-options-a.md). |
| `xy_hole_compensation` | `xy_hole_compensation` | identical | Implemented; see [batch A behavior and oracle results](../../../../docs/orca-options-a.md). |
| `bed_custom_model` | `bed_custom_model` | identical | Orca: Bed custom model []; Prusa: Bed custom model []. Same physical quantity and units; vector/scalar shape is checked. |
| `bed_custom_texture` | `bed_custom_texture` | identical | Orca: Bed custom texture []; Prusa: Bed custom texture []. Same physical quantity and units; vector/scalar shape is checked. |
| `before_layer_change_gcode` | `before_layer_gcode` | identical | Orca: Before layer change G-code []; Prusa: Before layer change G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `bottom_shell_layers` | `bottom_solid_layers` | identical | Orca: Bottom shell layers [layers]; Prusa: Bottom Layers []. Same physical quantity and units; vector/scalar shape is checked. |
| `bottom_shell_thickness` | `bottom_solid_min_thickness` | identical | Orca: Bottom shell thickness [mm]; Prusa: Bottom Layers [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `bottom_surface_pattern` | `bottom_fill_pattern` | converted | Orca: Bottom surface pattern []; Prusa: Bottom fill pattern []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `bridge_acceleration` | `bridge_acceleration` | converted | Orca: Bridge [mm/s² or %]; Prusa: Bridge [mm/s²]. Resolve percentage using Orca ratio_over before mapping (different canonical base/type). |
| `bridge_angle` | `bridge_angle` | identical | Orca: External bridge infill direction [°]; Prusa: Bridging angle [°]. Same physical quantity and units; vector/scalar shape is checked. |
| `bridge_flow` | `bridge_flow_ratio` | identical | Orca: Bridge flow ratio []; Prusa: Bridge flow ratio []. Same physical quantity and units; vector/scalar shape is checked. |
| `bridge_speed` | `bridge_speed` | identical | Orca: External [mm/s]; Prusa: Bridges [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `brim_object_gap` | `brim_separation` | identical | Orca: Brim-object gap [mm]; Prusa: Brim separation gap [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `brim_type` | `brim_type` | converted | All seven Orca modes retained; painted uses the object's hand-placed ears (`brim_ear_points.txt`). |
| `brim_width` | `brim_width` | identical | Orca: Brim width [mm]; Prusa: Brim width [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `change_filament_gcode` | `toolchange_gcode` | identical | Orca: Change filament G-code []; Prusa: Tool change G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `close_fan_the_first_x_layers` | `disable_fan_first_layers` | identical | Orca: No cooling for the first [layers]; Prusa: Disable fan for the first [layers]. Same physical quantity and units; vector/scalar shape is checked. |
| `cool_plate_temp` | `bed_temperature` | converted | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `cool_plate_temp_initial_layer` | `first_layer_bed_temperature` | converted | Orca: First layer [\u2103]; Prusa: First layer [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `cooling_tube_length` | `cooling_tube_length` | identical | Orca: Cooling tube length [mm]; Prusa: Cooling tube length [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `cooling_tube_retraction` | `cooling_tube_retraction` | identical | Orca: Cooling tube position [mm]; Prusa: Cooling tube position [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `default_acceleration` | `default_acceleration` | identical | Orca: Normal printing [mm/s²]; Prusa: Default [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `deretraction_speed` | `deretract_speed` | identical | Orca: De-retraction Speed [mm/s]; Prusa: Deretraction Speed [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `detect_thin_wall` | `thin_walls` | identical | Orca: Detect thin walls []; Prusa: Detect thin walls []. Same physical quantity and units; vector/scalar shape is checked. |
| `draft_shield` | `draft_shield` | converted | Orca: Draft shield []; Prusa: Draft shield []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `elefant_foot_compensation` | `elefant_foot_compensation` | identical | Orca: Elephant foot compensation [mm]; Prusa: Elephant foot compensation [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `enable_arc_fitting` | `arc_fitting` | converted | `0` → `disabled`, `1` → `emit_center`; U1 inherits `1`. Orca emits I/J arcs, including Klipper. |
| `ensure_vertical_shell_thickness` | `ensure_vertical_shell_thickness` | converted | As both slicers read their old booleans (`1`: Orca `ensure_all` / Prusa `enabled`, `0`: `ensure_moderate` / `partial`), `none` → `disabled`; Orca's `ensure_critical_only` (never written by Mica) reads as `partial` (lossy). |
| `enable_prime_tower` | `wipe_tower` | identical | Orca: Enable []; Prusa: Enable []. Same physical quantity and units; vector/scalar shape is checked. |
| `enable_support` | `support_material` | identical | Orca: Enable support []; Prusa: Generate support material []. Same physical quantity and units; vector/scalar shape is checked. |
| `enforce_support_layers` | `support_material_enforce_layers` | identical | Orca:  [layers]; Prusa: Enforce support for the first [layers]. Same physical quantity and units; vector/scalar shape is checked. |
| `eng_plate_temp` | `bed_temperature` | converted | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `eng_plate_temp_initial_layer` | `first_layer_bed_temperature` | converted | Orca: First layer [\u2103]; Prusa: First layer [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `extra_loading_move` | `extra_loading_move` | identical | Orca: Extra loading distance [mm]; Prusa: Extra loading distance [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `extra_perimeters_on_overhangs` | `extra_perimeters_on_overhangs` | identical | Orca: Extra perimeters on overhangs []; Prusa: Extra perimeters on overhangs (Experimental) []. Same physical quantity and units; vector/scalar shape is checked. |
| `extruder_clearance_radius` | `extruder_clearance_radius` | identical | Orca: Radius [mm]; Prusa: Radius [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `extruder_colour` | `extruder_colour` | identical | Orca: Extruder Color []; Prusa: Extruder Color []. Same physical quantity and units; vector/scalar shape is checked. |
| `extruder_offset` | `extruder_offset` | identical | Orca: Extruder offset [mm]; Prusa: Extruder offset [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `fan_cooling_layer_time` | `fan_below_layer_time` | identical | Orca: Layer time [s]; Prusa: Enable fan if layer print time is below [approximate seconds]. Same physical quantity and units; vector/scalar shape is checked. |
| `fan_max_speed` | `max_fan_speed` | identical | Orca: Fan speed [%]; Prusa: Max [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `fan_min_speed` | `min_fan_speed` | identical | Orca: Fan speed [%]; Prusa: Min [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_colour` | `filament_colour` | identical | Orca: Color []; Prusa: Color []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_cooling_final_speed` | `filament_cooling_final_speed` | identical | Orca: Speed of the last cooling move [mm/s]; Prusa: Speed of the last cooling move [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_cooling_initial_speed` | `filament_cooling_initial_speed` | identical | Orca: Speed of the first cooling move [mm/s]; Prusa: Speed of the first cooling move [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_cooling_moves` | `filament_cooling_moves` | identical | Orca: Number of cooling moves []; Prusa: Number of cooling moves []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_cost` | `filament_cost` | identical | Orca: Price [money/kg]; Prusa: Cost [money/kg]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_density` | `filament_density` | identical | Orca: Density [g/cm³]; Prusa: Density [g/cm³]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_deretraction_speed` | `deretract_speed` | converted | Orca:  []; Prusa: Deretraction Speed [mm/s]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_diameter` | `filament_diameter` | identical | Orca: Diameter [mm]; Prusa: Diameter [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_end_gcode` | `end_filament_gcode` | identical | Orca: End G-code []; Prusa: End G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_flow_ratio` | `extrusion_multiplier` | identical | Orca: Flow ratio []; Prusa: Extrusion multiplier []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_loading_speed` | `filament_loading_speed` | identical | Orca: Loading speed [mm/s]; Prusa: Loading speed [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_loading_speed_start` | `filament_loading_speed_start` | identical | Orca: Loading speed at the start [mm/s]; Prusa: Loading speed at the start [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_max_volumetric_speed` | `filament_max_volumetric_speed` | identical | Orca: Max volumetric speed [mm³/s]; Prusa: Max volumetric speed [mm³/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_minimal_purge_on_wipe_tower` | `filament_minimal_purge_on_wipe_tower` | identical | Orca: Minimal purge on wipe tower [mm³]; Prusa: Minimal purge on wipe tower [mm³]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_multitool_ramming` | `filament_multitool_ramming` | identical | Orca: Enable ramming for multi-tool setups []; Prusa: Enable ramming for multitool setups []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_multitool_ramming_flow` | `filament_multitool_ramming_flow` | identical | Orca: Multi-tool ramming flow [mm³/s]; Prusa: Multitool ramming flow [mm³/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_multitool_ramming_volume` | `filament_multitool_ramming_volume` | identical | Orca: Multi-tool ramming volume [mm³]; Prusa: Multitool ramming volume [mm³]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_notes` | `filament_notes` | identical | Orca: Filament notes []; Prusa: Filament notes []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_ramming_parameters` | `filament_ramming_parameters` | identical | Orca: Ramming parameters []; Prusa: Ramming parameters []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_retract_before_wipe` | `retract_before_wipe` | converted | Orca:  []; Prusa: Retract amount before wipe [%]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retract_lift_above` | `retract_lift_above` | converted | Orca:  []; Prusa: Above Z [mm]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retract_lift_below` | `retract_lift_below` | converted | Orca:  []; Prusa: Below Z [mm]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retract_restart_extra` | `retract_restart_extra` | converted | Orca:  []; Prusa: Deretraction extra length [mm]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retract_when_changing_layer` | `retract_layer_change` | converted | Orca:  []; Prusa: Retract on layer change []. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retraction_length` | `retract_length` | converted | Orca:  []; Prusa: Retraction length [mm (zero to disable)]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retraction_minimum_travel` | `retract_before_travel` | converted | Orca:  []; Prusa: Minimum travel after retraction [mm]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_retraction_speed` | `retract_speed` | converted | Orca:  []; Prusa: Retraction Speed [mm/s]. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_shrink` | `filament_shrinkage_compensation_xy` | converted | Orca: Shrinkage (XY) [%]; Prusa: Shrinkage compensation XY [%]. Orca retained-size percent -> Prusa shrink percent: 100 - value. |
| `filament_shrinkage_compensation_z` | `filament_shrinkage_compensation_z` | converted | Orca: Shrinkage (Z) [%]; Prusa: Shrinkage compensation Z [%]. Orca retained-size percent -> Prusa shrink percent: 100 - value. |
| `filament_soluble` | `filament_soluble` | identical | Orca: Soluble material []; Prusa: Soluble material []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_stamping_distance` | `filament_stamping_distance` | identical | Orca: Stamping distance measured from the center of the cooling tube []; Prusa: Stamping distance measured from the center of the cooling tube [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_stamping_loading_speed` | `filament_stamping_loading_speed` | identical | Orca: Stamping loading speed []; Prusa: Stamping loading speed [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_start_gcode` | `start_filament_gcode` | identical | Orca: Start G-code []; Prusa: Start G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_toolchange_delay` | `filament_toolchange_delay` | identical | Orca: Delay after unloading [s]; Prusa: Delay after unloading [s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_type` | `filament_type` | identical | Orca: Type []; Prusa: Filament type []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_unloading_speed` | `filament_unloading_speed` | identical | Orca: Unloading speed [mm/s]; Prusa: Unloading speed [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_unloading_speed_start` | `filament_unloading_speed_start` | identical | Orca: Unloading speed at the start [mm/s]; Prusa: Unloading speed at the start [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_vendor` | `filament_vendor` | identical | Orca: Vendor []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `filament_wipe` | `wipe` | converted | Orca:  []; Prusa: Wipe while retracting []. Apply non-nil filament values over corresponding per-tool printer values. |
| `filament_z_hop` | `retract_lift` | converted | Orca:  []; Prusa: Lift height [mm]. Apply non-nil filament values over corresponding per-tool printer values. |
| `full_fan_speed_layer` | `full_fan_speed_layer` | identical | Orca: Full fan speed at layer [layer]; Prusa: Full fan speed at layer []. Same physical quantity and units; vector/scalar shape is checked. |
| `fuzzy_skin` | `fuzzy_skin` | converted | Orca: Fuzzy Skin []; Prusa: Fuzzy Skin []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `fuzzy_skin_point_distance` | `fuzzy_skin_point_dist` | identical | Orca: Fuzzy skin point distance [mm]; Prusa: Fuzzy skin point distance [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `fuzzy_skin_thickness` | `fuzzy_skin_thickness` | identical | Orca: Fuzzy skin thickness [mm]; Prusa: Fuzzy skin thickness [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `gap_infill_speed` | `gap_fill_speed` | identical | Orca: Gap infill [mm/s]; Prusa: Gap fill [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `gcode_comments` | `gcode_comments` | identical | Orca: Verbose G-code []; Prusa: Verbose G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `gcode_flavor` | `gcode_flavor` | converted | Orca: G-code flavor []; Prusa: G-code flavor []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `gcode_label_objects` | `gcode_label_objects` | converted | Orca: Label objects []; Prusa: Label objects []. With `exclude_object`: Exclude objects on -> firmware (M486 / EXCLUDE_OBJECT), else Label objects -> octoprint, else disabled (`mica_core::merged`). |
| `exclude_object` | `gcode_label_objects` | converted | Orca: Exclude objects. Merged into Label objects (see `gcode_label_objects`). |
| `high_current_on_filament_swap` | `high_current_on_filament_swap` | identical | Orca: High extruder current on filament swap []; Prusa: High extruder current on filament swap []. Same physical quantity and units; vector/scalar shape is checked. |
| `hot_plate_temp` | `bed_temperature` | converted | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `hot_plate_temp_initial_layer` | `first_layer_bed_temperature` | converted | Orca: First layer [\u2103]; Prusa: First layer [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `idle_temperature` | `idle_temperature` | identical | Orca: Idle temperature [\u2103]; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `infill_anchor` | `infill_anchor` | identical | Orca: Sparse infill anchor length [mm or %]; Prusa: Length of the infill anchor [mm or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `infill_anchor_max` | `infill_anchor_max` | identical | Orca: Maximum length of the infill anchor []; Prusa: Maximum length of the infill anchor []. Same physical quantity and units; vector/scalar shape is checked. |
| `infill_direction` | `fill_angle` | identical | Orca: Sparse infill direction [°]; Prusa: Fill angle [°]. Same physical quantity and units; vector/scalar shape is checked. |
| `infill_wall_overlap` | `infill_overlap` | identical | Orca: Infill/Wall overlap [%]; Prusa: Infill/perimeters overlap [mm or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `initial_layer_acceleration` | `first_layer_acceleration` | identical | Orca: First layer [mm/s²]; Prusa: First layer [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `initial_layer_line_width` | `first_layer_extrusion_width` | converted | Orca: First layer [mm or %]; Prusa: First layer [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `initial_layer_print_height` | `first_layer_height` | identical | Orca: First layer height [mm]; Prusa: First layer height [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `initial_layer_speed` | `first_layer_speed` | identical | Orca: First layer [mm/s]; Prusa: First layer speed [mm/s or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `inner_wall_acceleration` | `perimeter_acceleration` | identical | Orca: Inner wall [mm/s²]; Prusa: Perimeters [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `inner_wall_line_width` | `perimeter_extrusion_width` | converted | Orca: Inner wall [mm or %]; Prusa: Perimeters [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `inner_wall_speed` | `perimeter_speed` | identical | Orca: Inner wall [mm/s]; Prusa: Perimeters [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `interface_shells` | `interface_shells` | identical | Orca: Interface shells []; Prusa: Interface shells []. Same physical quantity and units; vector/scalar shape is checked. |
| `internal_solid_infill_acceleration` | `solid_infill_acceleration` | converted | Orca: Internal solid infill [mm/s² or %]; Prusa: Solid infill [mm/s²]. Resolve percentage using Orca ratio_over before mapping (different canonical base/type). |
| `internal_solid_infill_line_width` | `solid_infill_extrusion_width` | converted | Orca: Internal solid infill [mm or %]; Prusa: Solid infill [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `internal_solid_infill_speed` | `solid_infill_speed` | identical | Orca: Internal solid infill [mm/s]; Prusa: Solid infill [mm/s or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `ironing_flow` | `ironing_flowrate` | identical | Orca: Ironing flow [%]; Prusa: Flow rate [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `ironing_spacing` | `ironing_spacing` | identical | Orca: Ironing line spacing [mm]; Prusa: Spacing between ironing passes [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `ironing_speed` | `ironing_speed` | identical | Orca: Ironing speed [mm/s]; Prusa: Ironing [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `ironing_type` | `ironing_type` | converted | Orca: Ironing Type []; Prusa: Ironing Type []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `layer_change_gcode` | `layer_gcode` | identical | Orca: Layer change G-code []; Prusa: After layer change G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `layer_height` | `layer_height` | identical | Orca: Layer height [mm]; Prusa: Layer height [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `line_width` | `extrusion_width` | converted | Orca: Default [mm or %]; Prusa: Default extrusion width [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `machine_end_gcode` | `end_gcode` | identical | Orca: End G-code []; Prusa: End G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_e` | `machine_max_acceleration_e` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_extruding` | `machine_max_acceleration_extruding` | identical | Orca:  [mm/s²]; Prusa:  [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_retracting` | `machine_max_acceleration_retracting` | identical | Orca:  [mm/s²]; Prusa:  [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_travel` | `machine_max_acceleration_travel` | identical | Orca:  [mm/s²]; Prusa:  [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_x` | `machine_max_acceleration_x` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_y` | `machine_max_acceleration_y` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_acceleration_z` | `machine_max_acceleration_z` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_jerk_e` | `machine_max_jerk_e` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_jerk_x` | `machine_max_jerk_x` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_jerk_y` | `machine_max_jerk_y` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_jerk_z` | `machine_max_jerk_z` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_speed_e` | `machine_max_feedrate_e` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_speed_x` | `machine_max_feedrate_x` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_speed_y` | `machine_max_feedrate_y` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_max_speed_z` | `machine_max_feedrate_z` | identical | Orca:  []; Prusa:  []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_min_extruding_rate` | `machine_min_extruding_rate` | identical | Orca:  [mm/s]; Prusa:  [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_min_travel_rate` | `machine_min_travel_rate` | identical | Orca:  [mm/s]; Prusa:  [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_pause_gcode` | `pause_print_gcode` | identical | Orca: Pause G-code []; Prusa: Pause Print G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `machine_start_gcode` | `start_gcode` | identical | Orca: Start G-code []; Prusa: Start G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `max_layer_height` | `max_layer_height` | identical | Orca: Max [mm]; Prusa: Max [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `max_travel_detour_distance` | `avoid_crossing_perimeters_max_detour` | identical | Orca: Avoid crossing walls - Max detour length [mm or %]; Prusa: Avoid crossing perimeters - Max detour length [mm or % (zero to disable)]. Same physical quantity and units; vector/scalar shape is checked. |
| `min_bead_width` | `min_bead_width` | converted | Orca: Minimum wall width [%]; Prusa: Minimum perimeter width [mm or %]. Orca percent type to Prusa float-or-percent; retain percent marker. |
| `min_feature_size` | `min_feature_size` | converted | Orca: Minimum feature size [%]; Prusa: Minimum feature size [mm or %]. Orca percent type to Prusa float-or-percent; retain percent marker. |
| `min_layer_height` | `min_layer_height` | identical | Orca: Min [mm]; Prusa: Min [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `min_skirt_length` | `min_skirt_length` | identical | Orca: Skirt minimum extrusion length [mm]; Prusa: Minimal filament extrusion length [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `notes` | `notes` | identical | Orca: Configuration notes []; Prusa: Configuration notes []. Same physical quantity and units; vector/scalar shape is checked. |
| `nozzle_diameter` | `nozzle_diameter` | identical | Orca: Nozzle diameter [mm]; Prusa: Nozzle diameter [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `nozzle_temperature` | `temperature` | identical | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Same physical quantity and units; vector/scalar shape is checked. |
| `nozzle_temperature_initial_layer` | `first_layer_temperature` | identical | Orca: First layer [\u2103]; Prusa: First layer [°C]. Same physical quantity and units; vector/scalar shape is checked. |
| `ooze_prevention` | `ooze_prevention` | identical | Orca: Enable []; Prusa: Enable []. Same physical quantity and units; vector/scalar shape is checked. |
| `outer_wall_acceleration` | `external_perimeter_acceleration` | identical | Orca: Outer wall [mm/s²]; Prusa: External perimeters [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `outer_wall_line_width` | `external_perimeter_extrusion_width` | converted | Orca: Outer wall [mm or %]; Prusa: External perimeters [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `outer_wall_speed` | `external_perimeter_speed` | identical | Orca: Outer wall [mm/s]; Prusa: External perimeters [mm/s or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `parking_pos_retraction` | `parking_pos_retraction` | identical | Orca: Filament parking position [mm]; Prusa: Filament parking position [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `post_process` | `post_process` | identical | Orca: Post-processing Scripts []; Prusa: Post-processing scripts []. Same physical quantity and units; vector/scalar shape is checked. |
| `prime_tower_brim_width` | `wipe_tower_brim_width` | identical | Orca: Brim width [mm]; Prusa: Wipe tower brim width [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `prime_tower_width` | `wipe_tower_width` | identical | Orca: Width [mm]; Prusa: Width [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `print_sequence` | `complete_objects` | converted | Orca: Print sequence []; Prusa: Complete individual objects []. by layer -> false; by object -> true. |
| `printable_area` | `bed_shape` | identical | Orca: Printable area []; Prusa: Bed shape []. Same physical quantity and units; vector/scalar shape is checked. |
| `printable_height` | `max_print_height` | identical | Orca: Printable height [mm]; Prusa: Max print height [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `printer_model` | `printer_model` | identical | Orca: Printer type []; Prusa: Printer type []. Same physical quantity and units; vector/scalar shape is checked. |
| `printer_notes` | `printer_notes` | identical | Orca: Printer notes []; Prusa: Printer notes []. Same physical quantity and units; vector/scalar shape is checked. |
| `printer_technology` | `printer_technology` | converted | Orca: Printer technology []; Prusa: Printer technology []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `printer_variant` | `printer_variant` | identical | Orca: Printer variant []; Prusa: Printer variant []. Same physical quantity and units; vector/scalar shape is checked. |
| `raft_contact_distance` | `raft_contact_distance` | identical | Orca: Raft contact Z distance [mm]; Prusa: Raft contact Z distance [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `raft_expansion` | `raft_expansion` | identical | Orca: Raft expansion [mm]; Prusa: Raft expansion [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `raft_first_layer_density` | `raft_first_layer_density` | identical | Orca: First layer density [%]; Prusa: First layer density [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `raft_first_layer_expansion` | `raft_first_layer_expansion` | identical | Orca: First layer expansion [mm]; Prusa: First layer expansion [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `raft_layers` | `raft_layers` | identical | Orca: Raft layers [layers]; Prusa: Raft layers [layers]. Same physical quantity and units; vector/scalar shape is checked. |
| `reduce_crossing_wall` | `avoid_crossing_perimeters` | identical | Orca: Avoid crossing walls []; Prusa: Avoid crossing perimeters []. Same physical quantity and units; vector/scalar shape is checked. |
| `reduce_fan_stop_start_freq` | `fan_always_on` | identical | Orca: Keep fan always on []; Prusa: Keep fan always on []. Same physical quantity and units; vector/scalar shape is checked. |
| `resolution` | `resolution` | identical | Orca: Resolution [mm]; Prusa: Slice resolution [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_before_wipe` | `retract_before_wipe` | identical | Orca: Retract amount before wipe [%]; Prusa: Retract amount before wipe [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_length_toolchange` | `retract_length_toolchange` | identical | Orca: Length [mm]; Prusa: Length [mm (zero to disable)]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_lift_above` | `retract_lift_above` | identical | Orca: Only lift Z above [mm]; Prusa: Above Z [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_lift_below` | `retract_lift_below` | identical | Orca: Only lift Z below [mm]; Prusa: Below Z [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_restart_extra` | `retract_restart_extra` | identical | Orca: Extra length on restart [mm]; Prusa: Deretraction extra length [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_restart_extra_toolchange` | `retract_restart_extra_toolchange` | identical | Orca: Extra length on restart [mm]; Prusa: Extra length on restart [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `retract_when_changing_layer` | `retract_layer_change` | identical | Orca: Retract when change layer []; Prusa: Retract on layer change []. Same physical quantity and units; vector/scalar shape is checked. |
| `retraction_length` | `retract_length` | identical | Orca: Length [mm]; Prusa: Retraction length [mm (zero to disable)]. Same physical quantity and units; vector/scalar shape is checked. |
| `retraction_minimum_travel` | `retract_before_travel` | identical | Orca: Travel distance threshold [mm]; Prusa: Minimum travel after retraction [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `retraction_speed` | `retract_speed` | identical | Orca: Retraction Speed [mm/s]; Prusa: Retraction Speed [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `seam_gap` | `seam_gap_distance` | identical | Nozzle-relative percent or absolute millimetres; resolved by the seam engine. |
| `seam_position` | `seam_position` | converted | Orca: Seam position []; Prusa: Seam position []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `single_extruder_multi_material` | `single_extruder_multi_material` | identical | Orca: Single Extruder Multi Material []; Prusa: Single Extruder Multi Material []. Same physical quantity and units; vector/scalar shape is checked. |
| `single_extruder_multi_material_priming` | `single_extruder_multi_material_priming` | identical | Orca: Prime all printing extruders []; Prusa: Prime all printing extruders []. Same physical quantity and units; vector/scalar shape is checked. |
| `skirt_distance` | `skirt_distance` | identical | Orca: Skirt distance [mm]; Prusa: Distance from brim/object [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `skirt_height` | `skirt_height` | identical | Orca: Skirt height [layers]; Prusa: Skirt height [layers]. Same physical quantity and units; vector/scalar shape is checked. |
| `skirt_loops` | `skirts` | identical | Orca: Skirt loops []; Prusa: Loops (minimum) []. Same physical quantity and units; vector/scalar shape is checked. |
| `slice_closing_radius` | `slice_closing_radius` | identical | Orca: Slice gap closing radius [mm]; Prusa: Slice gap closing radius [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `slicing_mode` | `slicing_mode` | converted | Orca: Slicing Mode []; Prusa: Slicing Mode []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `slow_down_for_layer_cooling` | `cooling` | identical | Orca: Slow printing down for better layer cooling []; Prusa: Enable auto cooling []. Same physical quantity and units; vector/scalar shape is checked. |
| `slow_down_layer_time` | `slowdown_below_layer_time` | identical | Orca: Layer time [s]; Prusa: Slow down if layer print time is below [approximate seconds]. Same physical quantity and units; vector/scalar shape is checked. |
| `slow_down_min_speed` | `min_print_speed` | identical | Orca: Min print speed [mm/s]; Prusa: Min print speed [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `small_perimeter_speed` | `small_perimeter_speed` | converted | Orca: Small perimeters [mm/s or %]; Prusa: Small perimeters [mm/s or %]. Resolve percentage using Orca ratio_over before mapping (different canonical base/type). |
| `sparse_infill_acceleration` | `infill_acceleration` | converted | Orca: Sparse infill [mm/s² or %]; Prusa: Infill [mm/s²]. Resolve percentage using Orca ratio_over before mapping (different canonical base/type). |
| `sparse_infill_density` | `fill_density` | identical | Orca: Sparse infill density [%]; Prusa: Fill density [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `sparse_infill_line_width` | `infill_extrusion_width` | converted | Orca: Sparse infill [mm or %]; Prusa: Infill [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `sparse_infill_pattern` | `fill_pattern` | converted | Orca: Sparse infill pattern []; Prusa: Fill pattern []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `sparse_infill_speed` | `infill_speed` | identical | Orca: Sparse infill [mm/s]; Prusa: Infill [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `spiral_mode` | `spiral_vase` | identical | Orca: Spiral vase []; Prusa: Spiral vase []. Same physical quantity and units; vector/scalar shape is checked. |
| `staggered_inner_seams` | `staggered_inner_seams` | identical | Orca: Staggered inner seams []; Prusa: Staggered inner seams []. Same physical quantity and units; vector/scalar shape is checked. |
| `standby_temperature_delta` | `standby_temperature_delta` | identical | Orca: Temperature variation [\u2206\u2103]; Prusa: Temperature variation [∆°C]. Same physical quantity and units; vector/scalar shape is checked. |
| `supertack_plate_temp` | `bed_temperature` | converted | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `supertack_plate_temp_initial_layer` | `first_layer_bed_temperature` | converted | Orca: First layer [\u2103]; Prusa: First layer [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `support_angle` | `support_material_angle` | identical | Orca: Pattern angle [°]; Prusa: Pattern angle [°]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_base_pattern_spacing` | `support_material_spacing` | identical | Orca: Base pattern spacing [mm]; Prusa: Pattern spacing [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_bottom_z_distance` | `support_material_bottom_contact_distance` | identical | Orca: Bottom Z distance [mm]; Prusa: Bottom contact Z distance [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_filament` | `support_material_extruder` | identical | Orca: Support/raft base []; Prusa: Support material/raft/skirt extruder []. Same physical quantity and units; vector/scalar shape is checked. |
| `support_interface_bottom_layers` | `support_material_bottom_interface_layers` | identical | Orca: Bottom interface layers [layers]; Prusa: Bottom interface layers [layers]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_interface_filament` | `support_material_interface_extruder` | identical | Orca: Support/raft interface []; Prusa: Support material/raft interface extruder []. Same physical quantity and units; vector/scalar shape is checked. |
| `support_interface_spacing` | `support_material_interface_spacing` | identical | Orca: Top interface spacing [mm]; Prusa: Interface pattern spacing [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_interface_speed` | `support_material_interface_speed` | identical | Orca: Support interface [mm/s]; Prusa: Support material interface [mm/s or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_interface_top_layers` | `support_material_interface_layers` | identical | Orca: Top interface layers [layers]; Prusa: Top interface layers [layers]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_line_width` | `support_material_extrusion_width` | converted | Orca: Support [mm or %]; Prusa: Support material [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `support_object_xy_distance` | `support_material_xy_spacing` | identical | Orca: Support/object XY distance [mm]; Prusa: XY separation between an object and its support [mm or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_on_build_plate_only` | `support_material_buildplate_only` | identical | Orca: On build plate only []; Prusa: Support on build plate only []. Same physical quantity and units; vector/scalar shape is checked. |
| `support_speed` | `support_material_speed` | identical | Orca: Support [mm/s]; Prusa: Support material [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_threshold_angle` | `support_material_threshold` | identical | Orca: Threshold angle [°]; Prusa: Overhang threshold [°]. Same physical quantity and units; vector/scalar shape is checked. |
| `support_top_z_distance` | `support_material_contact_distance` | identical | Orca: Top Z distance [mm]; Prusa: Top contact Z distance [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `template_custom_gcode` | `template_custom_gcode` | identical | Orca: Custom G-code []; Prusa: Custom G-code []. Same physical quantity and units; vector/scalar shape is checked. |
| `textured_cool_plate_temp` | `bed_temperature` | converted | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `textured_cool_plate_temp_initial_layer` | `first_layer_bed_temperature` | converted | Orca: First layer [\u2103]; Prusa: First layer [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `textured_plate_temp` | `bed_temperature` | converted | Orca: Other layers [\u2103]; Prusa: Other layers [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `textured_plate_temp_initial_layer` | `first_layer_bed_temperature` | converted | Orca: First layer [\u2103]; Prusa: First layer [°C]. Select only the temperature vector for curr_bed_type; other plates stay extras-only. |
| `thick_bridges` | `thick_bridges` | identical | Orca: Thick external bridges []; Prusa: Thick bridges []. Same physical quantity and units; vector/scalar shape is checked. |
| `thumbnails` | `thumbnails` | identical | Both define a String list of WxH/FORMAT thumbnail requests. Mica renders PNG and QOI; other formats fail at export. |
| `thumbnails_format` | `thumbnails_format` | identical | Both define the legacy format enum, default PNG. Mica honors it for sizes without extensions; unsupported formats fail at export. |
| `top_shell_layers` | `top_solid_layers` | identical | Orca: Top shell layers [layers]; Prusa: Top Layers []. Same physical quantity and units; vector/scalar shape is checked. |
| `top_shell_thickness` | `top_solid_min_thickness` | identical | Orca: Top shell thickness [mm]; Prusa: Top Layers [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `top_surface_acceleration` | `top_solid_infill_acceleration` | identical | Orca: Top surface [mm/s²]; Prusa: Top solid infill [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `top_surface_line_width` | `top_infill_extrusion_width` | converted | Orca: Top surface [mm or %]; Prusa: Top solid infill [mm or %]. Orca percent is of nozzle diameter; Prusa width percent is of layer height. Resolve to mm. |
| `top_surface_pattern` | `top_fill_pattern` | converted | Orca: Top surface pattern []; Prusa: Top fill pattern []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `top_surface_speed` | `top_solid_infill_speed` | identical | Orca: Top surface [mm/s]; Prusa: Top solid infill [mm/s or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `travel_acceleration` | `travel_acceleration` | identical | Orca: Travel [mm/s²]; Prusa: Travel [mm/s²]. Same physical quantity and units; vector/scalar shape is checked. |
| `travel_speed` | `travel_speed` | identical | Orca: Travel [mm/s]; Prusa: Travel [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `travel_speed_z` | `travel_speed_z` | identical | Orca:  [mm/s]; Prusa: Z travel [mm/s]. Same physical quantity and units; vector/scalar shape is checked. |
| `use_firmware_retraction` | `use_firmware_retraction` | identical | Orca: Use firmware retraction []; Prusa: Use firmware retraction []. Same physical quantity and units; vector/scalar shape is checked. |
| `use_relative_e_distances` | `use_relative_e_distances` | identical | Orca: Use relative E distances []; Prusa: Use relative E distances []. Same physical quantity and units; vector/scalar shape is checked. |
| `wall_distribution_count` | `wall_distribution_count` | identical | Orca: Wall distribution count []; Prusa: Perimeter distribution count []. Same physical quantity and units; vector/scalar shape is checked. |
| `wall_generator` | `perimeter_generator` | converted | Orca: Wall generator []; Prusa: Perimeter generator []. Explicit enum vocabulary; unsupported Orca modes stay extras-only. |
| `wall_loops` | `perimeters` | identical | Orca: Wall loops []; Prusa: Perimeters [(minimum)]. Same physical quantity and units; vector/scalar shape is checked. |
| `wall_transition_angle` | `wall_transition_angle` | identical | Orca: Wall transitioning threshold angle [°]; Prusa: Perimeter transitioning threshold angle [°]. Same physical quantity and units; vector/scalar shape is checked. |
| `wall_transition_filter_deviation` | `wall_transition_filter_deviation` | identical | Orca: Wall transitioning filter margin [%]; Prusa: Perimeter transitioning filter margin [mm or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `wall_transition_length` | `wall_transition_length` | identical | Orca: Wall transition length [%]; Prusa: Perimeter transition length [mm or %]. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe` | `wipe` | identical | Orca: Wipe while retracting []; Prusa: Wipe while retracting []. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe_tower_bridging` | `wipe_tower_bridging` | identical | Orca: Maximal bridging distance [mm]; Prusa: Maximal bridging distance [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe_tower_cone_angle` | `wipe_tower_cone_angle` | identical | Orca: Stabilization cone apex angle [°]; Prusa: Stabilization cone apex angle [°]. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe_tower_extra_flow` | `wipe_tower_extra_flow` | identical | Orca: Extra flow for purging [%]; Prusa: Extra flow for purging [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe_tower_extra_spacing` | `wipe_tower_extra_spacing` | identical | Orca: Wipe tower purge lines spacing [%]; Prusa: Wipe tower purge lines spacing [%]. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe_tower_filament` | `wipe_tower_extruder` | identical | Orca: Wipe tower []; Prusa: Wipe tower extruder []. Same physical quantity and units; vector/scalar shape is checked. |
| `wipe_tower_no_sparse_layers` | `wipe_tower_no_sparse_layers` | identical | Orca: No sparse layers (beta) []; Prusa: No sparse layers (EXPERIMENTAL) []. Same physical quantity and units; vector/scalar shape is checked. |
| `xy_contour_compensation` | `xy_contour_compensation` | identical | Independent Orca contour offset; does not modify the Prusa XY size setting. |
| `z_hop` | `retract_lift` | identical | Orca: Z-hop height [mm]; Prusa: Lift height [mm]. Same physical quantity and units; vector/scalar shape is checked. |
| `z_offset` | `z_offset` | identical | Orca: Z offset [mm]; Prusa: Z offset [mm]. Same physical quantity and units; vector/scalar shape is checked. |

All other schema keys (including Snapmaker mixed filament / flow options) have
no implemented equivalent. They remain available verbatim and are reported as
extras-only. See the complete key inventory in the two `*_schema.json` files.

Widths with percent and automatic (zero) width/nozzle ambiguities are rejected
rather than inventing a base. `small_perimeter_speed` keeps the speed quantity,
but Orca’s configurable small-perimeter threshold is an extras-only feature.
Support algorithms, ironing algorithms and path planning can differ even when
an individual numeric option has the same definition.

## Extras-only inventory (Orca 2.4.2)

These have no reviewed mapping in this importer. Some have related Prusa concepts,
but translating them would change meaning or requires a separate feature port.
The report distinguishes these valid values from malformed values.

| Orca key | Class | Reason |
|---|---|---|
| `accel_to_decel_enable` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `accel_to_decel_factor` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `activate_air_filtration` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `activate_air_filtration_during_print` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `activate_air_filtration_on_completion` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `activate_chamber_temp_control` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `adaptive_bed_mesh_margin` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `adaptive_pressure_advance` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `adaptive_pressure_advance_bridges` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `adaptive_pressure_advance_model` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `adaptive_pressure_advance_overhangs` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `additional_cooling_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `additional_fan_full_speed_layer` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `align_infill_direction_to_model` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `alternate_extra_wall` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `auxiliary_fan` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bbl_calib_mark_logo` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bbl_use_printhost` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bed_exclude_area` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bed_mesh_max` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bed_mesh_min` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bed_mesh_probe_distance` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bed_temperature_formula` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `best_object_pos` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bottom_solid_infill_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `bottom_surface_density` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bottom_surface_filament_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `bridge_line_width` | `bridge_line_width` | Active for classic overhang walls (Orca `LayerRegion::bridging_flow` width); see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `bridge_no_support` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `brim_ears` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `brim_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `calib_flowrate_topinfill_special_order` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `chamber_minimal_temperature` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `chamber_temperature` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `change_extrusion_role_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `close_additional_fan_first_x_layers` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `combine_brims` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `compatible_machine_expression_group` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `compatible_printers` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `compatible_printers_condition` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `compatible_prints` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `compatible_prints_condition` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `compatible_process_expression_group` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `complete_print_exhaust_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `counterbore_hole_bridging` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `curr_bed_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_bed_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_filament_colour` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_filament_profile` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_junction_deviation` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_nozzle_volume_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `default_print_profile` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `detect_narrow_internal_solid_infill` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `detect_overhang_wall` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `different_settings_to_system` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `disable_m73` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `dont_filter_internal_bridges` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `dont_slow_down_outer_wall` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `during_print_exhaust_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `elefant_foot_compensation_layers` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `elefant_foot_layers_density` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `emit_machine_limits_to_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `enable_filament_dynamic_map` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_filament_ramming` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_long_retraction_when_cut` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_overhang_bridge_fan` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_overhang_speed` | extras-only | Kept as Orca text; with Orca slicing rules the G-code export applies Orca's overhang speed estimator (`ExtrusionQualityEstimator`, `crates/mica-gcode/src/orca_overhang.rs`). |
| `enable_power_loss_recovery` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_pressure_advance` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `enable_tower_interface_cooldown_during_tower` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_tower_interface_features` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `enable_wrapping_detection` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extra_solid_infills` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_ams_count` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_clearance_height_to_lid` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_clearance_height_to_rod` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_printable_area` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_printable_height` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extruder_variant_list` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `extrusion_rate_smoothing_external_perimeter_only` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fan_kickstart` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fan_speedup_overhangs` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fan_speedup_time` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `filament_adaptive_volumetric_speed` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_adhesiveness_category` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_change_extrusion_role_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `filament_change_length` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_colour_type` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_cooling_before_tower` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_extruder_variant` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_flush_temp` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_flush_volumetric_speed` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_ids` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_ironing_flow` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_ironing_inset` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_ironing_spacing` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_ironing_speed` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_is_support` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_long_retractions_when_cut` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_map` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_map_mode` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_multi_colour` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_printable` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_retract_lift_enforce` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_retraction_distances_when_cut` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_self_index` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_settings_id` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_tower_interface_pre_extrusion_dist` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_tower_interface_pre_extrusion_length` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_tower_interface_print_temp` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_tower_interface_purge_volume` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_tower_ironing_area` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_wipe_distance` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `filament_z_hop_types` | extras-only | Orca material metadata or behavior has no reviewed canonical translation. |
| `file_start_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `filename_format` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fill_multiline` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `first_layer_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `first_layer_print_sequence` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `first_layer_sequence_choice` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `first_x_layer_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flashforge_serial_number` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flush_into_infill` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flush_into_objects` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flush_into_support` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flush_multiplier` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flush_volumes_matrix` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `flush_volumes_vector` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_first_layer` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_layers_between_ripple_offset` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_mode` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_noise_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_octaves` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_persistence` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_ripple_offset` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_ripples_per_layer` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `fuzzy_skin_scale` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `gap_fill_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `gap_fill_target` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `gcode_add_line_number` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `grab_length` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `gyroid_optimized` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `has_filament_switcher` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `has_scarf_joint_seam` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `head_wrap_detect_zone` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `hole_to_polyhole` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `hole_to_polyhole_threshold` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `hole_to_polyhole_twisted` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `host_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `independent_support_layer_height` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `infill_combination` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `infill_combination_max_layer_height` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `infill_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `infill_lock_depth` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `infill_overhang_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `inherits` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `inherits_group` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `initial_layer_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `initial_layer_travel_acceleration` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `initial_layer_travel_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `initial_layer_travel_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `inner_wall_filament_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `inner_wall_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `inner_wall_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `input_shaping_damp_x` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `input_shaping_damp_y` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `input_shaping_emit` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `input_shaping_freq_x` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `input_shaping_freq_y` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `input_shaping_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `interlocking_beam` | mapped | `interlocking_beam`; painted material segmentation and interlocking beams. |
| `interlocking_beam_layer_count` | mapped | `interlocking_beam_layer_count`; painted material segmentation and interlocking beams. |
| `interlocking_beam_width` | mapped | `interlocking_beam_width`; painted material segmentation and interlocking beams. |
| `interlocking_boundary_avoidance` | mapped | `interlocking_boundary_avoidance`; painted material segmentation and interlocking beams. |
| `interlocking_depth` | mapped | `interlocking_depth`; painted material segmentation and interlocking beams. |
| `interlocking_orientation` | mapped | `interlocking_orientation`; painted material segmentation and interlocking beams. |
| `internal_bridge_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `internal_bridge_flow` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `internal_solid_filament_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `internal_solid_infill_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `internal_solid_infill_pattern` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `ironing_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `ironing_angle_fixed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `ironing_expansion` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `ironing_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `ironing_inset` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `ironing_pattern` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `is_infill_first` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `lateral_lattice_angle_1` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `lateral_lattice_angle_2` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `lightning_overhang_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `lightning_prune_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `lightning_straightening_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `long_retractions_when_cut` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `long_retractions_when_ec` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `machine_load_filament_time` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `machine_max_junction_deviation` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `machine_tool_change_time` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `machine_unload_filament_time` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `make_overhang_printable` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `make_overhang_printable_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `make_overhang_printable_hole_size` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `manual_filament_change` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `master_extruder_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `max_bridge_length` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `max_resonance_avoidance_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `max_volumetric_extrusion_rate_slope` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `max_volumetric_extrusion_rate_slope_segment_length` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `min_length_factor` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `min_resonance_avoidance_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `mmu_segmented_region_interlocking_depth` | mapped | `mmu_segmented_region_interlocking_depth`; painted material segmentation and interlocking beams. |
| `mmu_segmented_region_max_width` | mapped | `mmu_segmented_region_max_width`; painted material segmentation and interlocking beams. |
| `nozzle_flush_dataset` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_height` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_hrc` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_temperature_range_high` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_temperature_range_low` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_volume` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `nozzle_volume_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `other_layers_print_sequence` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `other_layers_print_sequence_nums` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `other_layers_sequence_choice` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `outer_wall_filament_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `outer_wall_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `outer_wall_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `overhang_1_4_speed` | extras-only | Kept as Orca text; with Orca slicing rules the G-code export applies Orca's overhang speed estimator (`ExtrusionQualityEstimator`, `crates/mica-gcode/src/orca_overhang.rs`). |
| `overhang_2_4_speed` | extras-only | Kept as Orca text; with Orca slicing rules the G-code export applies Orca's overhang speed estimator (`ExtrusionQualityEstimator`, `crates/mica-gcode/src/orca_overhang.rs`). |
| `overhang_3_4_speed` | extras-only | Kept as Orca text; with Orca slicing rules the G-code export applies Orca's overhang speed estimator (`ExtrusionQualityEstimator`, `crates/mica-gcode/src/orca_overhang.rs`). |
| `overhang_4_4_speed` | extras-only | Kept as Orca text; with Orca slicing rules the G-code export applies Orca's overhang speed estimator (`ExtrusionQualityEstimator`, `crates/mica-gcode/src/orca_overhang.rs`). |
| `overhang_fan_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `overhang_fan_threshold` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `overhang_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `parallel_printheads_bed_exclude_areas` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `parallel_printheads_count` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `part_cooling_fan_min_pwm` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `pellet_flow_coefficient` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `pellet_modded_printer` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `physical_extruder_map` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `precise_outer_wall` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `precise_z_height` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `preferred_orientation` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `preheat_steps` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `preheat_time` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `preset_name` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `preset_names` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `pressure_advance` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `prime_tower_enable_framework` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `prime_tower_flat_ironing` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `prime_tower_infill_gap` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `prime_tower_skip_points` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `prime_volume` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_compatible_printers` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_extruder_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_extruder_variant` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `print_host` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_host_webui` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_order` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `print_settings_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printer_agent` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printer_extruder_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printer_extruder_variant` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printer_settings_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printer_structure` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_apikey` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_authorization_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_cafile` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_password` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_port` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_ssl_ignore_revoke` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printhost_user` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `printing_by_object_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `process_change_extrusion_role_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `purge_in_prime_tower` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `reduce_infill_retraction` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `relative_bridge_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `required_nozzle_HRC` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `resonance_avoidance` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `retract_lift_enforce` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `retraction_distances_when_cut` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `retraction_distances_when_ec` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `role_based_wipe_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `scan_first_layer` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `set_other_flow_ratios` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `silent_mode` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `single_loop_draft_shield` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `skeleton_infill_density` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `skeleton_infill_line_width` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `skin_infill_density` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `skin_infill_depth` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `skin_infill_line_width` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `slow_down_layers` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `slowdown_for_curled_perimeters` | extras-only | Kept as Orca text; with Orca slicing rules the overhang speed estimator also slows walls over the lower layer's curled external walls (`crates/mica-gcode/src/orca_overhang.rs`). |
| `solid_infill_direction` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `solid_infill_rotate_template` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `sparse_infill_filament_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `sparse_infill_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `sparse_infill_rotate_template` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `spiral_finishing_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `spiral_mode_max_xy_smoothing` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `spiral_mode_smooth` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `spiral_starting_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `start_end_points` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `support_air_filtration` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_base_pattern` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_bottom_interface_spacing` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_chamber_temp_control` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_critical_regions_only` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_expansion` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_flow_ratio` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_interface_flow_ratio` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_interface_loop_pattern` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_interface_pattern` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_ironing` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_ironing_flow` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_ironing_pattern` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_ironing_spacing` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_material_interface_fan_speed` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_multi_bed_types` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_object_first_layer_gap` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_object_skip_flush` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_parallel_printheads` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `support_threshold_overlap` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `temperature_vitrification` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `time_cost` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `time_lapse_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `timelapse_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `tool_change_on_wipe_tower` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `top_bottom_infill_wall_overlap` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `top_solid_infill_flow_ratio` | extras-only | Orca flow or pressure control behavior is not implemented in the canonical engine. |
| `top_surface_density` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `top_surface_filament_id` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `top_surface_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `travel_jerk` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `travel_slope` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `tree_support_auto_brim` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `tree_support_brim_width` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `tree_support_with_infill` | extras-only | Orca support geometry/modes require a separate algorithm mapping. |
| `upward_compatible_machine` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `use_3mf` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `volumetric_speed_coefficients` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wall_direction` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wall_maximum_deviation` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wall_maximum_resolution` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_before_external_loop` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_distance` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_on_loops` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_extra_rib_length` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_fillet_wall` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_max_purge_speed` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_rib_width` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_rotation_angle` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_wall_type` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_x` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wipe_tower_y` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wiping_volumes_extruders` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wrapping_detection_gcode` | extras-only | Orca printer capability/template context has no direct canonical slot. |
| `wrapping_detection_layers` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `wrapping_exclude_area` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `zaa_dont_alternate_fill_direction` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `zaa_enabled` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `zaa_min_z` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |
| `zaa_minimize_perimeter_height` | extras-only | No implemented equivalent in Mica’s Prusa option schema. |

Skirt/brim engine settings (`engine` in mapping.tsv) keep Orca's typed source schema:
`brim_ears_max_angle`, `brim_ears_detection_length`, `brim_use_efc_outline`, `skirt_type`,
`skirt_speed`, and `skirt_start_angle`. They are consumed by `AdhesionSettings`.
`brim_type` preserves all seven modes; `painted` produces an explicit preparation error until
painting exists. `brim_object_gap` maps to `brim_separation`; `skirt_loops` maps to `skirts`.

Batch A activates the six source options above and separates the contour offset from Prusa XY size compensation. Source defaults are imported; Mica neutral defaults apply only without an Orca profile.
Batch B source-named options are typed and resolved without injecting Orca defaults into Prusa profiles.
| Orca source key | Canonical | Behaviour |
|---|---|---|
| `bridge_density` | `bridge_density` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `enable_extra_bridge_layer` | `enable_extra_bridge_layer` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `internal_bridge_angle` | `internal_bridge_angle` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `internal_bridge_density` | `internal_bridge_density` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `overhang_reverse` | `overhang_reverse` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `overhang_reverse_internal_only` | `overhang_reverse_internal_only` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `overhang_reverse_threshold` | `overhang_reverse_threshold` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `scarf_angle_threshold` | `scarf_angle_threshold` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `scarf_joint_flow_ratio` | `scarf_joint_flow_ratio` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `scarf_joint_speed` | `scarf_joint_speed` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `scarf_overhang_threshold` | `scarf_overhang_threshold` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `support_remove_small_overhang` | `support_remove_small_overhang` | Active for normal supports: Orca's overhang clusters (sharp tails, cantilevers, small-cluster removal); see [normal supports](../../../../docs/normal-supports.md). |
| `support_interface_not_for_body` | `support_interface_not_for_body` | Active: a Default support base under another interface filament avoids the interface filament on support-only layers (`ToolOrdering::collect_extruders`); see [support filaments](../../../../docs/support-filaments.md). |
| `seam_slope_conditional` | `seam_slope_conditional` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `seam_slope_entire_loop` | `seam_slope_entire_loop` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `seam_slope_inner_walls` | `seam_slope_inner_walls` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `seam_slope_min_length` | `seam_slope_min_length` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `seam_slope_start_height` | `seam_slope_start_height` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `seam_slope_steps` | `seam_slope_steps` | Active; see [batch B scope and oracles](../../../../docs/orca-options-b.md). |
| `seam_slope_type` | `seam_slope_type` | Active; merged with PrusaSlicer's `scarf_seam_*` (`mica_core::merged`, docs/setting-names.md). |
| `z_hop_types` | `z_hop_types` | Active under OrcaSlicer rules (Orca's travel lift); editable in Printer settings, see [slicing rules](../../../../docs/slicing-rules.md). |
| `wall_sequence` | `wall_sequence` | Active; merged with PrusaSlicer's `external_perimeters_first` (`mica_core::merged`, docs/setting-names.md). |
| `infill_shift_step` | `infill_shift_step` | Active for the Cross Zag sparse pattern (`mica_core::fill::ZagSettings`, `fill/zag.rs`). |
| `symmetric_infill_y_axis` | `symmetric_infill_y_axis` | Active for the Zig Zag and Cross Zag sparse patterns (`mica_core::fill::ZagSettings`). |
