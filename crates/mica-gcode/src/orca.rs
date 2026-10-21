// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::_do_export, GCode::set_object_info, sanitize_instance_name, GCode::process_layer, GCode::_extrude, GCode::travel_to, GCode::PlaceholderParserIntegration
// Source: orca@8500fcdcca src/libslic3r/Print.cpp :: Print::_make_skirt, Print::finalize_first_layer_convex_hull
// Source: orca@8500fcdcca src/libslic3r/GCode/GCodeProcessor.cpp :: GCodeProcessor::run_post_process
// Source: orca@8500fcdcca src/libslic3r/Extruder.cpp :: Extruder::used_filament
// Changes: Runtime placeholders and labels for one object and T0; mesh projection supplies object hull.
// Changes: Use the prepared first-layer hull, including actual support, skirt and brim geometry.
// Source: snapmaker-orca@b1831e5dcb src/libslic3r/GCode.cpp :: GCode::_do_export
// Changes: Snapmaker chamber_cooling_mode from the softening temperature of T0.

use mica_core::config::{Config, ConfigValue, Value};
use mica_core::geometry::clip::intersection_ex;
use mica_core::geometry::{ExPolygon, Point, unscale};
use mica_core::mesh::TriangleMesh;
use mica_core::print::LayerSource;
use mica_core::slice::{DEFAULT_CLOSING_RADIUS, slice_mesh};
use mica_core::toolpath::Role;
use mica_core::{flow::Flow, settings::PrintSettings};
use mica_template::{Context, Var};

use crate::writer::decimal;

/// Convex hull of the mesh's XY projection, counterclockwise and beginning at the lowest XY vertex.
/// Unlike a bounding rectangle this preserves diagonal edges and overhang footprints.
pub(crate) fn object_hull(mesh: &TriangleMesh) -> Vec<Point> {
    hull(
        mesh.vertices
            .iter()
            .map(|v| Point::from_mm(f64::from(v[0]), f64::from(v[1])))
            .collect(),
    )
}

fn hull(mut points: Vec<Point>) -> Vec<Point> {
    points.sort_by_key(|p| (p.x, p.y));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |a: Point, b: Point, c: Point| {
        (b.x - a.x) as i128 * (c.y - a.y) as i128 - (b.y - a.y) as i128 * (c.x - a.x) as i128
    };
    let half = |iter: Vec<Point>| {
        let mut out: Vec<Point> = Vec::new();
        for p in iter {
            while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0 {
                out.pop();
            }
            out.push(p);
        }
        out.pop();
        out
    };
    let mut out = half(points.clone());
    out.extend(half(points.into_iter().rev().collect()));
    out
}

pub(crate) fn object_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if " !@#$%^&*()=+[]{};:\",'".contains(c) || c.is_control() {
            if !out.ends_with('_') {
                out.push('_');
            }
        } else {
            out.push(c);
        }
    }
    format!("{}_id_0_copy_0", out.trim_matches('_'))
}

pub(crate) fn object_definition(name: &str, hull: &[Point]) -> String {
    let min_x = hull.iter().map(|p| p.x).min().unwrap();
    let max_x = hull.iter().map(|p| p.x).max().unwrap();
    let min_y = hull.iter().map(|p| p.y).min().unwrap();
    let max_y = hull.iter().map(|p| p.y).max().unwrap();
    let polygon = hull
        .iter()
        .chain(hull.first())
        .map(|p| {
            format!(
                "[{},{}]",
                decimal(unscale(p.x), 6),
                decimal(unscale(p.y), 6)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "EXCLUDE_OBJECT_DEFINE NAME={name} CENTER={},{} POLYGON=[{polygon}]",
        decimal(unscale(min_x + max_x) / 2.0, 6),
        decimal(unscale(min_y + max_y) / 2.0, 6)
    )
}

fn point(c: &Config, key: &str, default: (f64, f64)) -> (f64, f64) {
    match c.get(key) {
        Some(ConfigValue::Scalar(Value::Point(x, y))) => (*x, *y),
        _ => default,
    }
}

/// Orca's variables for the six template call sites, specialized to the supported single-tool print.
/// Config variables (including curr_bed_type and nozzle temperatures) already have source types.
pub(crate) fn set_print_vars(
    ctx: &mut Context,
    c: &Config,
    obj: &dyn LayerSource,
    name: &str,
    mesh: Option<&TriangleMesh>,
    settings: &PrintSettings,
) {
    let n = c.vector_len("nozzle_diameter").max(1);
    for key in [
        "first_tools",
        "first_filaments",
        "first_non_support_tools",
        "first_non_support_filaments",
    ] {
        ctx.set(key, Var::ints((0..n).map(|i| if i == 0 { 0 } else { -1 })));
    }
    for key in [
        "initial_no_support_tool",
        "initial_no_support_extruder",
        "initial_no_support_hotend",
        "current_hotend",
        "most_used_physical_extruder_id",
    ] {
        ctx.set(key, Var::int(0));
    }
    for (key, source) in [
        (
            "retraction_distance_when_cut",
            "retraction_distances_when_cut",
        ),
        (
            "retraction_distance_when_ec",
            "retraction_distances_when_ec",
        ),
    ] {
        ctx.set(key, Var::float(c.float(source).unwrap_or(0.0)));
    }
    for (key, source) in [
        ("long_retraction_when_cut", "long_retractions_when_cut"),
        ("long_retraction_when_ec", "long_retractions_when_ec"),
    ] {
        ctx.set(key, Var::bool(c.bool(source).unwrap_or(false)));
    }
    // Aliases deliberately override same-named Prusa default options.
    for (key, source) in [
        ("temperature", "nozzle_temperature"),
        (
            "first_layer_temperature",
            "nozzle_temperature_initial_layer",
        ),
        (
            "bed_temperature_initial_layer",
            "first_layer_bed_temperature",
        ),
    ] {
        if let Some(v) = ctx.get(source).cloned() {
            ctx.set(key, v);
        }
    }
    ctx.set(
        "bed_temperature_initial_layer_single",
        Var::int(c.int("first_layer_bed_temperature").unwrap_or(0)),
    );
    ctx.set("bed_temperature_initial_layer_vector", Var::string(""));
    ctx.set("bbl_bed_temperature_gcode", Var::bool(false));
    ctx.set(
        "max_print_height",
        Var::int(c.float("max_print_height").unwrap_or(0.0) as i64),
    );
    ctx.set("first_layer_height", Var::float(obj.layer_z(0).1));
    ctx.set(
        "max_print_z",
        Var::int(obj.layer_z(obj.layer_count() - 1).0.ceil() as i64),
    );
    ctx.set(
        "overall_chamber_temperature",
        Var::int(c.int("chamber_temperature").unwrap_or(0)),
    );
    ctx.set(
        "min_vitrification_temperature",
        Var::int(c.int("temperature_vitrification").unwrap_or(0)),
    );
    // Snapmaker Orca's chamber cooling mode (its U1 start G-code branches on it): strong
    // cooling for a low softening temperature (<= 50 °C), weak up to 70 °C, else keep warm;
    // high-temperature filaments never ask for cooling. Only that fork defines
    // `filament_is_high_temperature`, and only its templates read the variable.
    if let Some(high) = c.bool_at("filament_is_high_temperature", 0) {
        let vitrification = c.int_at("temperature_vitrification", 0).unwrap_or(100);
        let mode = match vitrification {
            _ if high => 0,
            v if v <= 50 => 2,
            v if v <= 70 => 1,
            _ => 0,
        };
        ctx.set("chamber_cooling_mode", Var::int(mode));
    }
    ctx.set(
        "max_additional_fan",
        Var::float(c.float("additional_cooling_fan_speed").unwrap_or(0.0)),
    );
    ctx.set(
        "has_tpu_in_first_layer",
        Var::bool(c.string("filament_type") == Some("TPU")),
    );
    ctx.set(
        "is_all_bbl_filament",
        Var::bool(c.string("filament_vendor") == Some("Bambu Lab")),
    );
    ctx.set("enable_high_low_temp_mix", Var::bool(false));
    ctx.set("model_name", Var::string(name));
    ctx.set("plate_number", Var::string("1"));
    ctx.set("plate_name", Var::string(""));
    ctx.set(
        "during_print_exhaust_fan_speed_num",
        Var::ints((0..n).map(|i| {
            (c.float_at("during_print_exhaust_fan_speed", i)
                .unwrap_or(0.0)
                * 255.0
                / 100.0) as i64
        })),
    );
    for (key, source, fallback) in [
        (
            "flush_volumetric_speeds",
            "filament_flush_volumetric_speed",
            "filament_max_volumetric_speed",
        ),
        (
            "flush_temperatures",
            "filament_flush_temp",
            "nozzle_temperature_range_high",
        ),
    ] {
        let values = (0..n)
            .map(|i| {
                let v = c.float_at(source, i).unwrap_or(0.0);
                if v == 0.0 {
                    c.float_at(fallback, i).unwrap_or(0.0)
                } else {
                    v
                }
            })
            .collect::<Vec<_>>();
        ctx.set(
            key,
            if key == "flush_temperatures" {
                Var::ints(values.into_iter().map(|v| v as i64))
            } else {
                Var::floats(values)
            },
        );
    }
    ctx.set(
        "outer_wall_volumetric_speed",
        Var::float(
            (settings.external_perimeter_speed
                * Flow::new(
                    settings.external_perimeter_extrusion_width,
                    settings.layer_height,
                )
                .mm3_per_mm())
            .min(settings.filament_max_volumetric_speed),
        ),
    );
    ctx.set("print_time_sec", Var::string("__MICA_PRINT_TIME_SEC__"));
    ctx.set(
        "used_filament_length",
        Var::string("__MICA_USED_FILAMENT_METERS__"),
    );
    ctx.set("in_head_wrap_detect_zone", Var::bool(false));
    ctx.set("hold_chamber_temp_for_flat_print", Var::bool(false));
    if let Some(bb) = obj.first_layer_bbox() {
        let (mut min, mut max) = (
            (unscale(bb.min.x), unscale(bb.min.y)),
            (unscale(bb.max.x), unscale(bb.max.y)),
        );
        ctx.set(
            "first_layer_center_no_wipe_tower",
            Var::floats(
                if let Some(b) = obj.print_job().and_then(|j| j.object_first_layer_bbox()) {
                    [
                        unscale(b.min.x + b.max.x) / 2.,
                        unscale(b.min.y + b.max.y) / 2.,
                    ]
                } else {
                    [(min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0]
                },
            ),
        );
        if let Some(mesh) = mesh {
            let slice = slice_mesh(
                mesh,
                &[obj.layer_z(0).0 - obj.layer_z(0).1 / 2.0],
                DEFAULT_CLOSING_RADIUS,
            );
            let area: f64 = slice[0].islands.iter().map(ExPolygon::area_mm2).sum();
            ctx.set(
                "hold_chamber_temp_for_flat_print",
                Var::bool(obj.layer_z(obj.layer_count() - 1).0 < 0.3 && area > 40000.0),
            );
            let convex = obj
                .first_layer_hull()
                .map(<[Point]>::to_vec)
                .unwrap_or_else(|| {
                    hull(
                        slice[0]
                            .islands
                            .iter()
                            .flat_map(|p| p.contour.iter().copied())
                            .collect(),
                    )
                });
            let mut footprint = vec![ExPolygon {
                contour: convex.clone(),
                holes: Vec::new(),
            }];
            if let Some(b) = obj
                .print_job()
                .and_then(|j| j.wipe_tower.as_ref())
                .and_then(|t| t.first_layer_bbox())
            {
                footprint.push(ExPolygon {
                    contour: vec![
                        b.min,
                        Point::new(b.max.x, b.min.y),
                        b.max,
                        Point::new(b.min.x, b.max.y),
                    ],
                    holes: Vec::new(),
                });
            }
            let convex = hull(
                footprint
                    .iter()
                    .flat_map(|p| p.contour.iter().copied())
                    .collect(),
            );
            ctx.set(
                "first_layer_print_convex_hull",
                Var::strings(convex.iter().map(|p| {
                    format!(
                        "[{}, {}]",
                        decimal(unscale(p.x), 6),
                        decimal(unscale(p.y), 6)
                    )
                })),
            );
            let zone = c
                .points("head_wrap_detect_zone")
                .unwrap_or_default()
                .into_iter()
                .map(|(x, y)| Point::from_mm(x, y))
                .collect::<Vec<_>>();
            if zone.len() >= 3 {
                let (lo, hi) = mesh.bounds();
                footprint.push(ExPolygon {
                    contour: vec![
                        Point::from_mm(lo[0] as f64, lo[1] as f64),
                        Point::from_mm(hi[0] as f64, lo[1] as f64),
                        Point::from_mm(hi[0] as f64, hi[1] as f64),
                        Point::from_mm(lo[0] as f64, hi[1] as f64),
                    ],
                    holes: Vec::new(),
                });
                ctx.set(
                    "in_head_wrap_detect_zone",
                    Var::bool(
                        !intersection_ex(
                            &footprint,
                            &[ExPolygon {
                                contour: zone,
                                holes: Vec::new(),
                            }],
                        )
                        .is_empty(),
                    ),
                );
            }
        }
        ctx.set("first_layer_print_min", Var::floats([min.0, min.1]));
        ctx.set("first_layer_print_max", Var::floats([max.0, max.1]));
        ctx.set(
            "first_layer_print_size",
            Var::floats([max.0 - min.0, max.1 - min.1]),
        );
        let mesh_min = point(c, "bed_mesh_min", min);
        let mesh_max = point(c, "bed_mesh_max", max);
        let margin = c.float("adaptive_bed_mesh_margin").unwrap_or(0.0);
        min = (
            mesh_min.0.max(min.0 - margin),
            mesh_min.1.max(min.1 - margin),
        );
        max = (
            mesh_max.0.min(max.0 + margin),
            mesh_max.1.min(max.1 + margin),
        );
        ctx.set("adaptive_bed_mesh_min", Var::floats([min.0, min.1]));
        ctx.set("adaptive_bed_mesh_max", Var::floats([max.0, max.1]));
        let dist = point(c, "bed_mesh_probe_distance", (50.0, 50.0));
        let x = (((max.0 - min.0) / dist.0.max(1.0)).ceil() as i64 + 1).max(3);
        let y = (((max.1 - min.1) / dist.1.max(1.0)).ceil() as i64 + 1).max(3);
        let lagrange = x * y <= 6;
        ctx.set(
            "bed_mesh_probe_count",
            Var::ints([
                if lagrange { x } else { x.max(4) },
                if lagrange { y } else { y.max(4) },
            ]),
        );
        ctx.set(
            "bed_mesh_algo",
            Var::string(if lagrange { "lagrange" } else { "bicubic" }),
        );
    }
}

pub(crate) fn jerk(c: &Config, role: Role, first: bool, travel: bool, short: bool) -> f64 {
    let default = c.float("default_jerk").unwrap_or(0.0);
    if default <= 0.0 {
        return 0.0;
    }
    if travel {
        let key = if first {
            "initial_layer_travel_jerk"
        } else if short && role == Role::ExternalPerimeter {
            "outer_wall_jerk"
        } else {
            "travel_jerk"
        };
        return c.float(key).unwrap_or(0.0);
    }
    if first && let Some(jerk) = c.float("initial_layer_jerk").filter(|v| *v > 0.0) {
        return jerk;
    }
    let key = match role {
        Role::ExternalPerimeter => "outer_wall_jerk",
        Role::Perimeter => "inner_wall_jerk",
        Role::TopSolidInfill | Role::Ironing => "top_surface_jerk",
        _ => "infill_jerk",
    };
    c.float(key).filter(|v| *v > 0.0).unwrap_or(default)
}

/// Orca's inline placeholders are strings until postprocessing supplies planned time and total
/// extrusion, in seconds and metres. Firmware macro internals are opaque to the estimator.
pub(crate) fn replace_statistics(gcode: String, stats: crate::writer::MoveStats) -> String {
    gcode
        .replace("__MICA_PRINT_TIME_SEC__", &format!("{:.2}", stats.time_s))
        .replace(
            "__MICA_USED_FILAMENT_METERS__",
            &format!("{:.2}", stats.filament_mm / 1000.0),
        )
}

/// Refresh Orca's writer-backed placeholders and read writable outputs back after each template.
/// Only T0 has motion state; the remaining physical tool slots stay idle.
pub(crate) fn run(
    name: &str,
    template: &str,
    ctx: &mut Context,
    writer: &mut crate::writer::Writer,
    c: &Config,
) -> Result<String, String> {
    let n = c.vector_len("nozzle_diameter").max(1);
    let (position, retracted, restart_extra, zhop) = writer.template_state();
    ctx.set_output("position", Var::floats(position));
    ctx.set_output(
        "e_retracted",
        Var::floats((0..n).map(|i| if i == 0 { retracted } else { 0.0 })),
    );
    ctx.set_output(
        "e_restart_extra",
        Var::floats((0..n).map(|i| if i == 0 { restart_extra } else { 0.0 })),
    );
    ctx.set("zhop", Var::float(zhop));
    let volume = (writer.stats.filament_mm + retracted)
        * std::f64::consts::PI
        * (c.float("filament_diameter").unwrap_or(1.75) / 2.0).powi(2);
    let weight = volume * c.float("filament_density").unwrap_or(0.0) * 0.001;
    ctx.set("extruded_volume", Var::floats([volume]));
    ctx.set("extruded_weight", Var::floats([weight]));
    ctx.set("extruded_volume_total", Var::float(volume));
    ctx.set("extruded_weight_total", Var::float(weight));
    let result = crate::templates::run(name, template, ctx)?;
    let output = |key: &str, len: usize| -> Result<Vec<f64>, String> {
        let values = ctx
            .output(key)
            .and_then(Var::as_vector)
            .filter(|v| v.len() == len)
            .ok_or_else(|| format!("{name}: {key} output must have {len} elements"))?;
        values
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("{name}: {key} must contain finite numbers"))
            })
            .collect()
    };
    let pos = output("position", 3)?;
    let e = output("e_retracted", n)?;
    let extra = output("e_restart_extra", n)?;
    if e.iter().any(|e| *e < 0.0) {
        return Err(format!("{name}: negative e_retracted"));
    }
    if e.iter()
        .skip(1)
        .chain(extra.iter().skip(1))
        .any(|v| *v != 0.0)
    {
        return Err(format!(
            "{name}: changing inactive tool state is not supported in T0-only output"
        ));
    }
    let pos = [pos[0], pos[1], pos[2]];
    writer.apply_template_state((pos != position).then_some(pos), e[0], extra[0]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusion_uses_projected_mesh_hull_not_a_rectangle() {
        let mesh = TriangleMesh::from_triangles(&[[
            [10.0, 20.0, 0.0],
            [30.0, 20.0, 5.0],
            [20.0, 40.0, 0.0],
        ]]);
        let polygon = object_hull(&mesh);
        assert_eq!(polygon.len(), 3);
        assert_eq!(object_name(" a (b), c.stl "), "a_b_c.stl_id_0_copy_0");
        assert_eq!(
            object_definition("triangle", &polygon),
            "EXCLUDE_OBJECT_DEFINE NAME=triangle CENTER=20,30 POLYGON=[[10,20],[30,20],[20,40],[10,20]]"
        );
    }
    #[test]
    fn template_writer_state_is_refreshed_and_validated() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4,0.4,0.4").unwrap();
        let mut ctx = Context::new();
        let mut writer = crate::writer::Writer::with_config(crate::Flavor::Klipper, &c);
        let text = run(
            "start",
            "{position[2]=2.5; e_retracted[0]=0.8;}G1 Z2.5",
            &mut ctx,
            &mut writer,
            &c,
        )
        .unwrap();
        assert_eq!(text, "G1 Z2.5");
        assert_eq!(writer.template_state(), ([0.0, 0.0, 2.5], 0.8, 0.0, 0.0));
        assert_eq!(
            run(
                "layer",
                "{position[2]}:{e_retracted[0]}",
                &mut ctx,
                &mut writer,
                &c
            )
            .unwrap(),
            "2.5:0.8"
        );
        assert!(
            run("end", "{e_retracted[1]=1;}", &mut ctx, &mut writer, &c)
                .unwrap_err()
                .contains("inactive tool")
        );
        assert!(run("end", "{position=(1,2);}", &mut ctx, &mut writer, &c).is_err());
    }
}
