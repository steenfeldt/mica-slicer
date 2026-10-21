# Orca 2.4.2 versus Snapmaker Orca 2.4.0 (U1)

This is a report, not an asserted compatibility baseline. Re-run with:

```sh
cargo test --offline -p mica-profiles --test orca_bundle report_orca242 -- --nocapture
```

Selection: Snapmaker U1 (0.4 nozzle), standard 0.20 process; four copies of
Generic PLA (Snapmaker) or Generic PLA @System (OrcaFilamentLibrary).

267 differing keys; 444 identical shared keys. Orca adds 115 keys; Snapmaker has 42 unique keys.

Notable changes: Orca uses tri-hexagon infill (canonical stars), retraction
0.8 mm at 40 mm/s versus 1.5 mm at 30 mm/s, updated printer limits and G-code,
and shared Generic PLA data from OrcaFilamentLibrary. Snapmaker retains its
process flow vectors and mixed-material options. Large templates below show
their serialized byte lengths; runtime reports include a text preview.

| Key | Snapmaker | Orca |
|---|---|---|
| `accel_to_decel_enable` | `["1"]` | `"1"` |
| `accel_to_decel_factor` | `["50%"]` | `"50%"` |
| `activate_air_filtration_during_print` | absent | `["1", "1", "1", "1"]` |
| `activate_air_filtration_on_completion` | absent | `["1", "1", "1", "1"]` |
| `additional_fan_full_speed_layer` | absent | `["0", "0", "0", "0"]` |
| `bed_mesh_max` | `"99999,99999"` | `"267,267"` |
| `bed_mesh_min` | `"-99999,-99999"` | `"3,3"` |
| `bed_temperature_formula` | absent | `"by_highest_temp"` |
| `bottom_surface_filament_id` | absent | `"0"` |
| `bridge_acceleration` | `["50%"]` | `"50%"` |
| `bridge_density` | `"80%"` | `"100%"` |
| `bridge_flow` | `"0.8"` | `"1"` |
| `bridge_line_width` | absent | `"100%"` |
| `bridge_speed` | `["50"]` | `"50"` |
| `brim_flow_ratio` | absent | `"1"` |
| `brim_use_efc_outline` | absent | `"0"` |
| `chamber_minimal_temperature` | absent | `["0", "0", "0", "0"]` |
| `change_filament_gcode` | (1171 bytes; template/model text) | (1526 bytes; template/model text) |
| `close_additional_fan_first_x_layers` | absent | `["1", "1", "1", "1"]` |
| `combine_brims` | absent | `"0"` |
| `default_acceleration` | `["10000"]` | `"10000"` |
| `default_jerk` | `["0"]` | `"0"` |
| `default_junction_deviation` | `["0"]` | `"0"` |
| `default_nozzle_volume_type` | absent | `["Standard", "Standard", "Standard", "Standard"]` |
| `default_print_profile` | `"0.20 Standard @Snapmaker U1 (0.4 nozzle)"` | `"0.20mm Standard @Snapmaker"` |
| `delta_temperature` | `"0"` | absent |
| `deretraction_speed` | `["30", "30", "30", "30"]` | `["35", "35", "35", "35"]` |
| `dithering_local_z_direct_multicolor` | `"0"` | absent |
| `dithering_local_z_infill` | `"0"` | absent |
| `dithering_local_z_mode` | `"0"` | absent |
| `dithering_local_z_whole_objects` | `"0"` | absent |
| `dithering_step_painted_zones_only` | `"1"` | absent |
| `dithering_z_step_size` | `"0"` | absent |
| `elefant_foot_layers_density` | absent | `"100%"` |
| `enable_arc_fitting` | `"0"` | `"1"` |
| `enable_change_pressure_when_wiping` | `"1"` | absent |
| `enable_overhang_speed` | `["1"]` | `"1"` |
| `enable_power_loss_recovery` | absent | `"printer_configuration"` |
| `enable_tower_interface_cooldown_during_tower` | absent | `"0"` |
| `enable_tower_interface_features` | absent | `"0"` |
| `enable_wrapping_detection` | absent | `"0"` |
| `extruder_ams_count` | absent | `[]` |
| `extruder_printable_area` | absent | `[]` |
| `extruder_printable_height` | absent | `["0", "0", "0", "0"]` |
| `extruder_type` | absent | `["Direct Drive", "Direct Drive", "Direct Drive", "Direct Drive"]` |
| `extruder_variant_list` | absent | `["Direct Drive Standard"]` |
| `extrusion_rate_smoothing_external_perimeter_only` | `["0"]` | `"0"` |
| `filament_adaptive_volumetric_speed` | absent | `["0", "0", "0", "0"]` |
| `filament_adhesiveness_category` | absent | `["0", "0", "0", "0"]` |
| `filament_change_extrusion_role_gcode` | absent | `["", "", "", ""]` |
| `filament_change_length` | absent | `["10", "10", "10", "10"]` |
| `filament_colour_mode` | `["0"]` | absent |
| `filament_cooling_before_tower` | absent | `["10", "10", "10", "10"]` |
| `filament_end_gcode` | `["\n", "\n", "\n", "\n"]` | `["; filament end gcode\n", "; filament end gcode\n", "; filament end gcode\n", "; filament end gcode\n"]` |
| `filament_extruder_variant` | absent | `["Direct Drive Standard", "Direct Drive Standard", "Direct Drive Standard", "Direct Drive Standard"]` |
| `filament_flow_step_size` | `["1", "1", "1", "1"]` | absent |
| `filament_flow_support` | `["standard", "standard", "standard", "standard"]` | absent |
| `filament_flush_temp` | absent | `["0", "0", "0", "0"]` |
| `filament_flush_volumetric_speed` | absent | `["0", "0", "0", "0"]` |
| `filament_grouping_mode` | `"standard"` | absent |
| `filament_ids` | `["GFSL991", "GFSL991", "GFSL991", "GFSL991"]` | `["OGFL99", "OGFL99", "OGFL99", "OGFL99"]` |
| `filament_ironing_flow` | absent | `["nil", "nil", "nil", "nil"]` |
| `filament_ironing_inset` | absent | `["nil", "nil", "nil", "nil"]` |
| `filament_ironing_spacing` | absent | `["nil", "nil", "nil", "nil"]` |
| `filament_ironing_speed` | absent | `["nil", "nil", "nil", "nil"]` |
| `filament_is_high_temperature` | `["0", "0", "0", "0"]` | absent |
| `filament_map` | absent | `["1"]` |
| `filament_map_mode` | absent | `"Auto For Flush"` |
| `filament_multi_colors` | `[""]` | absent |
| `filament_multitool_ramming` | `["1", "1", "1", "1"]` | `["0", "0", "0", "0"]` |
| `filament_multitool_ramming_flow` | `["20", "20", "20", "20"]` | `["10", "10", "10", "10"]` |
| `filament_multitool_ramming_volume` | `["0.5", "0.5", "0.5", "0.5"]` | `["10", "10", "10", "10"]` |
| `filament_printable` | absent | `["3", "3", "3", "3"]` |
| `filament_ramming_parameters` | (604 bytes; template/model text) | (212 bytes; template/model text) |
| `filament_retract_length_toolchange` | `["10", "10", "10", "10"]` | absent |
| `filament_retract_lift_above` | `["nil", "nil", "nil", "nil"]` | `["0", "0", "0", "0"]` |
| `filament_retract_lift_below` | `["nil", "nil", "nil", "nil"]` | `["0", "0", "0", "0"]` |
| `filament_retract_lift_enforce` | `["nil", "nil", "nil", "nil"]` | `["All Surfaces", "All Surfaces", "All Surfaces", "All Surfaces"]` |
| `filament_retract_restart_extra_toolchange` | `["nil", "nil", "nil", "nil"]` | absent |
| `filament_scarf_gap` | absent | `["15%", "15%", "15%", "15%"]` |
| `filament_scarf_height` | absent | `["10%", "10%", "10%", "10%"]` |
| `filament_scarf_length` | absent | `["10", "10", "10", "10"]` |
| `filament_scarf_seam_type` | absent | `["none", "none", "none", "none"]` |
| `filament_settings_id` | `["Generic PLA", "Generic PLA", "Generic PLA", "Generic PLA"]` | `["Generic PLA @System", "Generic PLA @System", "Generic PLA @System", "Generic PLA @System"]` |
| `filament_start_gcode` | `["", "", "", ""]` | `["; Filament gcode\n", "; Filament gcode\n", "; Filament gcode\n", "; Filament gcode\n"]` |
| `filament_tower_interface_pre_extrusion_dist` | absent | `["10", "10", "10", "10"]` |
| `filament_tower_interface_pre_extrusion_length` | absent | `["0", "0", "0", "0"]` |
| `filament_tower_interface_print_temp` | absent | `["-1", "-1", "-1", "-1"]` |
| `filament_tower_interface_purge_volume` | absent | `["20", "20", "20", "20"]` |
| `filament_volume_type` | `["standard"]` | absent |
| `file_start_gcode` | absent | `""` |
| `first_layer_flow_ratio` | absent | `"1"` |
| `first_x_layer_fan_speed` | absent | `["0", "0", "0", "0"]` |
| `flashforge_serial_number` | absent | `""` |
| `flush_multiplier` | `"0.3"` | `["0.3"]` |
| `fuzzy_skin` | `"none"` | `"disabled_fuzzy"` |
| `fuzzy_skin_layers_between_ripple_offset` | absent | `"1"` |
| `fuzzy_skin_ripple_offset` | absent | `"50%"` |
| `fuzzy_skin_ripples_per_layer` | absent | `"15"` |
| `gap_fill_flow_ratio` | absent | `"1"` |
| `gap_fill_target` | `"topbottom"` | `"nowhere"` |
| `gap_infill_speed` | `["250"]` | `"250"` |
| `grab_length` | absent | `["0"]` |
| `graphic_effect_plate_temp` | `["65", "65", "65", "65"]` | absent |
| `graphic_effect_plate_temp_initial_layer` | `["65", "65", "65", "65"]` | absent |
| `gyroid_optimized` | absent | `"0"` |
| `hot_plate_temp` | `["65", "65", "65", "65"]` | `["55", "55", "55", "55"]` |
| `hot_plate_temp_initial_layer` | `["65", "65", "65", "65"]` | `["55", "55", "55", "55"]` |
| `independent_support_layer_height` | `"0"` | `"1"` |
| `infill_jerk` | `["9"]` | `"9"` |
| `initial_layer_acceleration` | `["500"]` | `"500"` |
| `initial_layer_infill_speed` | `["105"]` | `"105"` |
| `initial_layer_jerk` | `["9"]` | `"9"` |
| `initial_layer_speed` | `["50"]` | `"50"` |
| `initial_layer_travel_acceleration` | absent | `"100%"` |
| `initial_layer_travel_jerk` | absent | `"100%"` |
| `initial_layer_travel_speed` | `["100%"]` | `"100%"` |
| `inner_wall_acceleration` | `["10000"]` | `"10000"` |
| `inner_wall_filament_id` | absent | `"0"` |
| `inner_wall_flow_ratio` | absent | `"1"` |
| `inner_wall_jerk` | `["9"]` | `"9"` |
| `inner_wall_speed` | `["300", "600"]` | `"300"` |
| `input_shaping_damp_x` | absent | `"0.1"` |
| `input_shaping_damp_y` | absent | `"0.1"` |
| `input_shaping_emit` | absent | `"0"` |
| `input_shaping_freq_x` | absent | `"0"` |
| `input_shaping_freq_y` | absent | `"0"` |
| `input_shaping_type` | absent | `"Default"` |
| `internal_bridge_speed` | `["150%"]` | `"150%"` |
| `internal_solid_filament_id` | absent | `"0"` |
| `internal_solid_infill_acceleration` | `["100%"]` | `"100%"` |
| `internal_solid_infill_flow_ratio` | absent | `"1"` |
| `internal_solid_infill_pattern` | `"rectilinear"` | `"monotonic"` |
| `internal_solid_infill_speed` | `["250", "600"]` | `"250"` |
| `ironing_angle` | `"-1"` | `"0"` |
| `ironing_angle_fixed` | absent | `"0"` |
| `ironing_expansion` | absent | `"0"` |
| `ironing_speed` | `["30"]` | `"30"` |
| `layer_change_gcode` | `";AFTER_LAYER_CHANGE\n;[layer_z]\nSET_PRINT_STATS_INFO TOTAL_LAYER={total_layer_count} CURRENT_LAYER={layer_num+1}"` | `";AFTER_LAYER_CHANGE\n;[layer_z]\nSET_PRINT_STATS_INFO TOTAL_LAYER={total_layer_count}\nSET_PRINT_STATS_INFO CURRENT_LAYER={layer_num+1}"` |
| `lightning_overhang_angle` | absent | `"45"` |
| `lightning_prune_angle` | absent | `"45"` |
| `lightning_straightening_angle` | absent | `"45"` |
| `local_z_wipe_tower_purge_lines` | `"3"` | absent |
| `long_retractions_when_ec` | absent | `["0", "0", "0", "0"]` |
| `machine_end_gcode` | `"{if print_sequence == \"by object\"}\nG91\nG1 X2 Y2 Z1 F24000\nG90\nG1 Z{max_layer_z+2} F600\n{endif}\nPRINT_END\nTIMELAPSE_STOP"` | (449 bytes; template/model text) |
| `machine_max_acceleration_extruding` | `["20000", "20000"]` | `["25000", "25000"]` |
| `machine_max_acceleration_travel` | `["20000", "20000"]` | `["25000", "25000"]` |
| `machine_max_acceleration_x` | `["20000", "20000"]` | `["25000", "25000"]` |
| `machine_max_acceleration_y` | `["20000", "20000"]` | `["25000", "25000"]` |
| `machine_max_junction_deviation` | `["0", "0"]` | `["0.01"]` |
| `machine_max_speed_x` | `["500", "200"]` | `["1000", "200"]` |
| `machine_max_speed_y` | `["500", "200"]` | `["1000", "200"]` |
| `machine_start_gcode` | (6225 bytes; template/model text) | (2839 bytes; template/model text) |
| `master_extruder_id` | absent | `"1"` |
| `max_resonance_avoidance_speed` | `"120"` | `"90"` |
| `max_volumetric_extrusion_rate_slope` | `["0"]` | `"0"` |
| `max_volumetric_extrusion_rate_slope_segment_length` | `["3"]` | `"3"` |
| `min_resonance_avoidance_speed` | `"70"` | `"40"` |
| `min_width_top_surface` | `"200%"` | `"300%"` |
| `mixed_color_layer_height_a` | `"0"` | absent |
| `mixed_color_layer_height_b` | `"0"` | absent |
| `mixed_filament_advanced_dithering` | `"0"` | absent |
| `mixed_filament_component_bias_enabled` | `"0"` | absent |
| `mixed_filament_definitions` | `""` | absent |
| `mixed_filament_gradient_mode` | `"0"` | absent |
| `mixed_filament_height_lower_bound` | `"0.04"` | absent |
| `mixed_filament_height_upper_bound` | `"0.16"` | absent |
| `mixed_filament_pointillism_line_gap` | `"0"` | absent |
| `mixed_filament_pointillism_pixel_size` | `"0"` | absent |
| `mixed_filament_region_collapse` | `"1"` | absent |
| `mixed_filament_surface_indentation` | `"0"` | absent |
| `nozzle_flush_dataset` | absent | `["0", "0", "0", "0"]` |
| `nozzle_type` | `"hardened_steel"` | `["stainless_steel", "stainless_steel", "stainless_steel", "stainless_steel"]` |
| `nozzle_volume` | `"143"` | `["143", "143", "143", "143"]` |
| `nozzle_volume_type` | `["standard", "standard", "standard", "standard"]` | `["Standard"]` |
| `outer_wall_acceleration` | `["5000"]` | `"5000"` |
| `outer_wall_filament_id` | absent | `"0"` |
| `outer_wall_flow_ratio` | absent | `"1"` |
| `outer_wall_jerk` | `["9"]` | `"9"` |
| `outer_wall_speed` | `["200", "500"]` | `"200"` |
| `overhang_1_4_speed` | `["0"]` | `"0"` |
| `overhang_2_4_speed` | `["50"]` | `"50"` |
| `overhang_3_4_speed` | `["30"]` | `"30"` |
| `overhang_4_4_speed` | `["10"]` | `"10"` |
| `overhang_flow_ratio` | absent | `"1"` |
| `parallel_printheads_bed_exclude_areas` | absent | `[]` |
| `parallel_printheads_count` | absent | `"1"` |
| `part_cooling_fan_min_pwm` | absent | `"0"` |
| `physical_extruder_map` | absent | `["0"]` |
| `precise_outer_wall` | `"0"` | `"1"` |
| `prime_tower_brim_chamfer` | `"1"` | absent |
| `prime_tower_brim_chamfer_max_width` | `"4"` | absent |
| `prime_tower_enable_framework` | absent | `"0"` |
| `prime_tower_flat_ironing` | absent | `"0"` |
| `prime_tower_infill_gap` | absent | `"150%"` |
| `prime_tower_skip_points` | absent | `"1"` |
| `prime_tower_width` | `"30"` | `"35"` |
| `prime_volume` | `"45"` | `"30"` |
| `print_extruder_id` | absent | `["1"]` |
| `print_extruder_variant` | absent | `["Direct Drive Standard"]` |
| `print_settings_id` | `"0.20mm Standard @Snapmaker U1 (0.4 nozzle)"` | `"0.20 Standard @Snapmaker U1 (0.4 nozzle)"` |
| `printer_agent` | absent | `"snapmaker"` |
| `printer_extruder_id` | absent | `["1"]` |
| `printer_extruder_variant` | absent | `["Direct Drive Standard"]` |
| `printer_flow_support` | `["standard", "high_flow"]` | absent |
| `process_change_extrusion_role_gcode` | absent | `""` |
| `process_flow_support` | `["standard", "high_flow"]` | absent |
| `ramming_line_width_ratio` | `"2"` | absent |
| `relative_bridge_angle` | absent | `"0"` |
| `resonance_avoidance` | `"0"` | `"1"` |
| `retraction_distances_when_ec` | absent | `["10", "10", "10", "10"]` |
| `retraction_length` | `["1.5", "1.5", "1.5", "1.5"]` | `["0.8", "0.8", "0.8", "0.8"]` |
| `retraction_speed` | `["30", "30", "30", "30"]` | `["40", "40", "40", "40"]` |
| `seam_gap` | `"15%"` | `"10%"` |
| `set_other_flow_ratios` | absent | `"0"` |
| `slow_down_layer_time` | `["8", "8", "8", "8"]` | `["4", "4", "4", "4"]` |
| `slow_down_layers` | `["0"]` | `"0"` |
| `slow_down_min_speed` | `["20", "20", "20", "20"]` | `["10", "10", "10", "10"]` |
| `slowdown_for_curled_perimeters` | `["0"]` | `"0"` |
| `small_perimeter_speed` | `["50%"]` | `"50%"` |
| `small_perimeter_threshold` | `["0"]` | `"0"` |
| `solid_infill_filament` | `"1"` | absent |
| `sparse_infill_acceleration` | `["100%"]` | `"100%"` |
| `sparse_infill_filament` | `"1"` | absent |
| `sparse_infill_filament_id` | absent | `"0"` |
| `sparse_infill_flow_ratio` | absent | `"1"` |
| `sparse_infill_pattern` | `"grid"` | `"crosshatch"` |
| `sparse_infill_speed` | `["270", "600"]` | `"270"` |
| `supertack_plate_temp` | `["35", "35", "35", "35"]` | `["45", "45", "45", "45"]` |
| `supertack_plate_temp_initial_layer` | `["35", "35", "35", "35"]` | `["45", "45", "45", "45"]` |
| `support_flow_ratio` | absent | `"1"` |
| `support_interface_flow_ratio` | absent | `"1"` |
| `support_interface_speed` | `["80"]` | `"80"` |
| `support_object_skip_flush` | absent | `"0"` |
| `support_parallel_printheads` | absent | `"0"` |
| `support_speed` | `["150"]` | `"150"` |
| `support_type` | `"tree(auto)"` | `"normal(auto)"` |
| `textured_plate_temp` | `["65", "65", "65", "65"]` | `["55", "55", "55", "55"]` |
| `textured_plate_temp_initial_layer` | `["65", "65", "65", "65"]` | `["55", "55", "55", "55"]` |
| `tool_change_on_wipe_tower` | absent | `"0"` |
| `top_surface_acceleration` | `["2000"]` | `"2000"` |
| `top_surface_filament_id` | absent | `"0"` |
| `top_surface_jerk` | `["9"]` | `"9"` |
| `top_surface_speed` | `["200"]` | `"200"` |
| `travel_acceleration` | `["10000"]` | `"10000"` |
| `travel_jerk` | `["12"]` | `"12"` |
| `travel_speed` | `["500"]` | `"500"` |
| `tree_support_adaptive_layer_height` | `"1"` | absent |
| `use_3mf` | absent | `"0"` |
| `volumetric_speed_coefficients` | absent | `["", "", "", ""]` |
| `wall_direction` | `"auto"` | `"ccw"` |
| `wall_filament` | `"1"` | absent |
| `wall_generator` | `"classic"` | `"arachne"` |
| `wall_maximum_deviation` | absent | `"0.025"` |
| `wall_maximum_resolution` | absent | `"0.5"` |
| `wipe_speed` | `"50"` | `"80%"` |
| `wipe_tower_extra_rib_length` | `"8"` | `"0"` |
| `wipe_tower_extra_spacing` | `"120%"` | `"100%"` |
| `wipe_tower_type` | absent | `"type2"` |
| `wipe_tower_wall_gap` | `"1"` | absent |
| `wrapping_detection_gcode` | absent | `""` |
| `wrapping_detection_layers` | absent | `"20"` |
| `wrapping_exclude_area` | absent | `[]` |
| `zaa_dont_alternate_fill_direction` | absent | `"0"` |
| `zaa_enabled` | absent | `"0"` |
| `zaa_min_z` | absent | `"0.05"` |
| `zaa_minimize_perimeter_height` | absent | `"35"` |
