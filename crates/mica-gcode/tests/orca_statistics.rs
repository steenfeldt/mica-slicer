// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: DoExport::update_print_stats_and_format_filament_stats, GCode::_do_export
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Changes: End-to-end footer contract on Mica's nonconsecutive-tool jobs, with and without a tower; explicit motion totals checked by the independent validator.

use mica_core::{
    config::Config,
    mesh::TriangleMesh,
    print::LayerSource,
    print_job::{PreparedPrint, PrintObject, ToolAssignments},
    rules::Rules,
    settings::PrintSettings,
};
use mica_gcode::{
    export::export_gcode,
    validate::{Profile, validate_text},
};
use std::io::Cursor;

fn values(line: &str) -> Vec<f64> {
    line.split_once(" = ")
        .unwrap()
        .1
        .split(", ")
        .map(|v| v.parse().unwrap())
        .collect()
}

#[test]
fn exported_orca_statistics_match_slot_usage_with_and_without_tower() {
    for tower in [false, true] {
        let mut c = Config::defaults();
        Rules::OrcaSlicer.set(&mut c);
        for (k, v) in [
            ("gcode_flavor", "klipper"),
            ("nozzle_diameter", "0.4,0.4,0.4,0.4"),
            ("filament_diameter", "1.75,1.75,1.75,1.75"),
            ("filament_density", "1.24,1.1,1.24,1.32"),
            ("filament_cost", "60,100,60,0"),
            ("first_layer_temperature", "220,220,220,220"),
            ("temperature", "220,220,220,220"),
            ("use_relative_e_distances", "1"),
            ("layer_height", "0.2"),
            ("first_layer_height", "0.2"),
            ("perimeter_generator", "classic"),
            ("wipe_tower_x", "20"),
            ("wipe_tower_y", "150"),
            ("wipe_tower_width", "35"),
            ("single_extruder_multi_material_priming", "0"),
            ("retract_length_toolchange", "2,2,2,2"),
            (
                "toolchange_gcode",
                "M109 S220 T{next_extruder}\nT{next_extruder}\nSM_PRINT_PREEXTRUDE_FILAMENT",
            ),
            (
                "start_gcode",
                "M83\nM190 S60\nM109 S220 T1\nT1\nG1 E15 F600",
            ),
        ] {
            c.set_raw(k, v).unwrap();
        }
        c.set_raw("wipe_tower", if tower { "1" } else { "0" })
            .unwrap();
        let objects = [1, 3]
            .into_iter()
            .enumerate()
            .map(|(id, tool)| {
                let mut mesh = TriangleMesh::load_stl(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../tests/models/cube20.stl"
                ))
                .unwrap();
                for v in &mut mesh.vertices {
                    v[2] *= 0.02;
                }
                mesh.place_on_bed(70. + id as f32 * 40., 70.);
                PrintObject::prepare(
                    format!("cube{id}"),
                    id,
                    0,
                    mesh,
                    ToolAssignments([Some(tool), Some(tool), Some(tool), None, None]),
                    &c,
                )
                .unwrap()
            })
            .collect();
        let job = PreparedPrint::new(objects, &c).unwrap();
        assert_eq!(job.wipe_tower.is_some(), tower);
        let result =
            export_gcode(&job, &c, &PrintSettings::from_config(&c).unwrap(), "stats").unwrap();
        let g = result.gcode;
        let report = validate_text(Cursor::new(&g), &Profile::from_config(&c).unwrap());
        assert_eq!(report.errors, 0, "{:?}", report.diagnostics);
        let block = &g[g.find("; filament used [mm] =").unwrap()..];
        let lines: Vec<_> = block
            .split("; CONFIG_BLOCK_START")
            .next()
            .unwrap()
            .trim_end()
            .lines()
            .collect();
        let tags: Vec<_> = lines
            .iter()
            .map(|l| l.split_once(" = ").unwrap().0)
            .collect();
        assert_eq!(
            tags,
            [
                "; filament used [mm]",
                "; filament used [cm3]",
                "; filament used [g]",
                "; filament cost",
                "; total filament used [g]",
                "; total filament cost",
                "; total filament change",
                "; total layers count",
                "; estimated printing time (normal mode)",
                "; estimated first layer printing time (normal mode)",
            ]
        );
        let mm = values(lines[0]);
        assert_eq!(mm.len(), 4);
        assert_eq!((mm[0], mm[2]), (0., 0.));
        assert!(mm[1] > 15. && mm[3] > 0.);
        for (&actual, measured) in mm.iter().zip(&report.filament_mm) {
            assert!((actual - measured).abs() <= 0.0051);
        }
        let grams = values(lines[2]);
        assert!(grams[1] > 0. && grams[3] > 0.);
        assert_eq!(values(lines[3])[3], 0.);
        assert_eq!(values(lines[6])[0] as usize, job.ordering.changes);
        assert_eq!(values(lines[7])[0] as usize, job.layer_count());
    }
}
