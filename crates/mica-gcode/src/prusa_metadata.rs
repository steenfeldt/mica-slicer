// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/Utils.hpp :: get_time_dhms
// Source: prusaslicer@b028299c77 src/libslic3r/GCode.cpp :: GCodeGenerator::_do_export (binary_data metadata)
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::post_process (print/printer metadata)
// Source: prusaslicer@b028299c77 src/libslic3r/GCode/LabelObjects.cpp :: LabelObjects::all_objects_header_singleline_json
// Changes: Built from Mica's config and planned normal/silent/first-layer statistics; accepts rendered
// Changes: thumbnail blocks. max_layer_z includes z_offset under PrusaSlicer rules (m_max_layer_z).
// Changes: Shared full_config serializer filters foreign options and host credentials in text and binary metadata under both rules.

//! Prusa metadata: the `.bgcode` metadata blocks and the `objects_info` object outline.

use mica_core::config::Config;
use mica_core::geometry::{Point, unscale};
use mica_core::mesh::TriangleMesh;

use crate::bgcode::{BgcodeMetadata, Metadata, Thumbnail};
use crate::writer::MoveStats;

/// Keys PrusaSlicer copies from the full config into the printer metadata block.
const PRINTER_KEYS: &[&str] = &[
    "printer_model",
    "filament_type",
    "filament_abrasive",
    "nozzle_diameter",
    "nozzle_high_flow",
    "bed_temperature",
    "brim_width",
    "fill_density",
    "layer_height",
    "temperature",
    "ironing",
    "support_material",
    "extruder_colour",
];

/// `get_time_dhms`: "1d 2h 3m 4s", leading zero units omitted.
/// The `max_layer_z` of the binary metadata for a print whose highest layer is at
/// `max_print_z`: PrusaSlicer's `m_max_layer_z` includes `z_offset` (its print_z does).
pub fn max_layer_z(config: &Config, max_print_z: f64) -> f64 {
    let offset = if mica_core::rules::Rules::of(config).is_orca() {
        0.
    } else {
        config
            .float("z_offset")
            .filter(|v| v.is_finite())
            .unwrap_or(0.)
    };
    max_print_z + offset
}

pub fn time_dhms(secs: f64) -> String {
    // Upstream truncates seconds for durations >= one minute, rounding only sub-minute values.
    let secs = secs.max(0.0) as f32;
    if secs < 60.0 {
        return format!("{}s", secs.round() as u64);
    }
    let mut s = secs.trunc() as u64;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    s %= 60;
    if d > 0 {
        format!("{d}d {h}h {m}m {s}s")
    } else if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Convex hull (counter-clockwise) of the mesh's XY projection, starting at the top-right point,
/// which is how PrusaSlicer's object polygon for the cube comes out.
fn convex_hull(mesh: &TriangleMesh) -> Vec<(f64, f64)> {
    let mut pts: Vec<Point> = mesh
        .vertices
        .iter()
        .map(|v| Point::from_mm(f64::from(v[0]), f64::from(v[1])))
        .collect();
    pts.sort_by_key(|p| (p.x, p.y));
    pts.dedup();
    if pts.len() < 3 {
        return pts.iter().map(|p| (unscale(p.x), unscale(p.y))).collect();
    }
    let cross = |o: Point, a: Point, b: Point| {
        (a.x - o.x) as i128 * (b.y - o.y) as i128 - (a.y - o.y) as i128 * (b.x - o.x) as i128
    };
    let mut hull: Vec<Point> = Vec::with_capacity(2 * pts.len());
    for pass in [false, true] {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Point>> = if pass {
            Box::new(pts.iter().rev())
        } else {
            Box::new(pts.iter())
        };
        for &p in iter {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    // Start at the point with the largest x (then largest y), as PrusaSlicer's polygons do for boxes.
    let first = (0..hull.len())
        .max_by_key(|&i| (hull[i].x, hull[i].y))
        .unwrap_or(0);
    hull.rotate_left(first);
    hull.iter().map(|p| (unscale(p.x), unscale(p.y))).collect()
}

/// `{"objects":[{"name":…,"polygon":[[x,y],…]}]}` as PrusaSlicer writes it.
pub fn objects_info(name: &str, mesh: &TriangleMesh) -> String {
    let polygon: Vec<String> = convex_hull(mesh)
        .iter()
        .map(|(x, y)| format!("[{x:.3},{y:.3}]"))
        .collect();
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "{{\"objects\":[{{\"name\":\"{escaped}\",\"polygon\":[{}]}}]}}",
        polygon.join(",")
    )
}

/// Filament statistics in PrusaSlicer's units.
struct Filament {
    mm: f64,
    cm3: f64,
    g: f64,
    cost: f64,
}

fn filament(config: &Config, stats: MoveStats) -> Filament {
    let d = config.float("filament_diameter").unwrap_or(1.75);
    let cm3 = stats.filament_mm * std::f64::consts::PI * (d / 2.0).powi(2) / 1000.0;
    let g = cm3 * config.float("filament_density").unwrap_or(0.0);
    let cost = g / 1000.0 * config.float("filament_cost").unwrap_or(0.0);
    Filament {
        mm: stats.filament_mm,
        cm3,
        g,
        cost,
    }
}

/// The four metadata blocks PrusaSlicer writes into a `.bgcode` file.
pub fn bgcode_metadata(
    config: &Config,
    stats: MoveStats,
    max_layer_z: f64,
    objects_info: Option<&str>,
    produced_on: &str,
    thumbnails: Vec<Thumbnail>,
) -> BgcodeMetadata {
    let f = filament(config, stats);
    let n = |v: f64| format!("{v:.2}");
    let time = time_dhms(stats.time_s);

    let slicer = crate::full_config::entries(config);

    let mut printer: Vec<(String, String)> = PRINTER_KEYS
        .iter()
        .filter_map(|k| {
            slicer
                .binary_search_by(|(key, _)| key.as_str().cmp(k))
                .ok()
                .map(|i| slicer[i].clone())
        })
        .collect();
    printer.push(("max_layer_z".into(), n(max_layer_z)));
    if let Some(info) = objects_info {
        printer.push(("objects_info".into(), info.to_string()));
    }
    printer.extend([
        ("filament used [mm]".into(), n(f.mm)),
        ("filament used [g]".into(), n(f.g)),
        ("filament cost".into(), n(f.cost)),
        ("filament used [cm3]".into(), n(f.cm3)),
        ("total filament used for wipe tower [g]".into(), n(0.0)),
        ("estimated printing time (normal mode)".into(), time.clone()),
    ]);
    let mut print = vec![
        ("filament used [mm]".into(), n(f.mm)),
        ("filament used [cm3]".into(), n(f.cm3)),
        ("filament used [g]".into(), n(f.g)),
        ("filament cost".into(), n(f.cost)),
        ("total filament used [g]".into(), n(f.g)),
        ("total filament cost".into(), n(f.cost)),
        ("total filament used for wipe tower [g]".into(), n(0.0)),
        ("estimated printing time (normal mode)".into(), time),
    ];
    print.push((
        "estimated first layer printing time (normal mode)".into(),
        time_dhms(stats.first_layer_time_s),
    ));
    if let Some(silent) = stats.silent_time_s {
        let entry = (
            "estimated printing time (silent mode)".into(),
            time_dhms(silent),
        );
        printer.push(entry.clone());
        print.push(entry);
        print.push((
            "estimated first layer printing time (silent mode)".into(),
            time_dhms(stats.silent_first_layer_time_s.unwrap_or(0.0)),
        ));
    }
    BgcodeMetadata {
        file: Some(Metadata::Ini(vec![
            (
                "Producer".into(),
                format!("Mica Slicer {}", mica_core::VERSION),
            ),
            ("Produced on".into(), produced_on.to_string()),
        ])),
        printer: Metadata::Ini(printer),
        thumbnails,
        print: Metadata::Ini(print),
        slicer: vec![Metadata::Ini(slicer)],
    }
}

/// "2026-10-15 at 19:55:07 UTC", PrusaSlicer's `utc_timestamp` format.
pub fn utc_timestamp(unix_secs: u64) -> String {
    let (days, rem) = (unix_secs / 86400, unix_secs % 86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} at {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_and_timestamp_formats() {
        assert_eq!(time_dhms(923.0), "15m 23s");
        assert_eq!(time_dhms(35.0), "35s");
        assert_eq!(time_dhms(59.6), "60s");
        assert_eq!(time_dhms(60.6), "1m 0s");
        assert_eq!(time_dhms(3723.0), "1h 2m 3s");
        assert_eq!(utc_timestamp(1_792_093_007), "2026-10-15 at 19:36:47 UTC");
        assert_eq!(utc_timestamp(0), "1970-01-01 at 00:00:00 UTC");
    }

    #[test]
    fn cube_objects_info_matches_prusaslicer() {
        let mut m = TriangleMesh::load_stl(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/cube20.stl"
        ))
        .unwrap();
        m.place_on_bed(125.0, 110.0);
        assert_eq!(
            objects_info("cube20.stl", &m),
            r#"{"objects":[{"name":"cube20.stl","polygon":[[135.000,120.000],[115.000,120.000],[115.000,100.000],[135.000,100.000]]}]}"#
        );
    }
}
