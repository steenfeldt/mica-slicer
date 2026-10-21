// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Guard rails: inline, non-modal warnings about risky setting combinations, each with a
//! one-click fix where one exists. Pure functions over the effective configuration and the
//! plate's objects, so they are easy to test.

use crate::scene::Object;
use mica_core::config::Config;

#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub label: String,
    /// The settings the fix applies, as `(key, raw text)`.
    pub settings: Vec<(String, String)>,
}

impl Fix {
    fn one(label: impl Into<String>, key: &str, raw: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            settings: vec![(key.to_string(), raw.into())],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Guard {
    pub message: String,
    pub fix: Option<Fix>,
}

fn round_step(v: f64, step: f64) -> f64 {
    (v / step).floor() * step
}

/// Short text for a millimetre value (`0.3`, not `0.30000000000000004`).
fn mm(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A short object name for messages: the file stem.
fn short(name: &str) -> &str {
    std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
}

/// Turns relative extrusion on (the printer's setting): the fix of the absolute extrusion
/// notice and of the slice error it leads to.
pub fn relative_e_fix() -> Fix {
    Fix::one(
        "Turn on relative E distances",
        "use_relative_e_distances",
        "1",
    )
}

/// Checks the plate configuration against the objects on it.
pub fn guard_rails(config: &Config, objects: &[Object]) -> Vec<Guard> {
    let mut out = Vec::new();
    // A printer profile with absolute extrusion (several stock Orca printers ship one): the
    // slice will stop, so say so as soon as it is selected.
    if let Some(problem) = mica_core::safety::extrusion_mode_problem(config) {
        out.push(Guard {
            message: format!("{problem}."),
            fix: Some(relative_e_fix()),
        });
    }
    let nozzle = config.float("nozzle_diameter").unwrap_or(0.4);
    let layer = config.float("layer_height").unwrap_or(0.2);
    let first = config
        .abs_value("first_layer_height", layer)
        .unwrap_or(layer);
    if layer > nozzle * 0.75 + 1e-9 {
        let fix = round_step(nozzle * 0.75, 0.05).max(0.05);
        out.push(Guard {
            message: format!(
                "Layer height {layer:.2} mm is more than 75 % of the {nozzle:.2} mm nozzle; layers may not bond."
            ),
            fix: Some(Fix::one(format!("Set {fix:.2} mm"), "layer_height", mm(fix))),
        });
    } else if layer < nozzle * 0.1 - 1e-9 {
        let fix = round_step(nozzle * 0.25, 0.05).max(0.05);
        out.push(Guard {
            message: format!(
                "Layer height {layer:.2} mm is very thin for a {nozzle:.2} mm nozzle; expect slow, inconsistent extrusion."
            ),
            fix: Some(Fix::one(format!("Set {fix:.2} mm"), "layer_height", mm(fix))),
        });
    }
    if first > nozzle + 1e-9 {
        out.push(Guard {
            message: format!(
                "First layer height {first:.2} mm is taller than the nozzle; the first layer may not stick."
            ),
            fix: Some(Fix::one(
                format!("Set {layer:.2} mm"),
                "first_layer_height",
                mm(layer),
            )),
        });
    }
    if config.int("perimeters") == Some(0) {
        out.push(Guard {
            message: "Zero walls: the model will have no walls, only infill and surfaces.".into(),
            fix: Some(Fix::one("Set 2 walls", "perimeters", "2")),
        });
    }
    if config.int("top_solid_layers") == Some(0)
        && config.float("fill_density").unwrap_or(20.) < 100.
    {
        out.push(Guard {
            message: "No top layers with sparse infill: the top will be open.".into(),
            fix: Some(Fix::one("Set 4 top layers", "top_solid_layers", "4")),
        });
    }
    let brim = config.string("brim_type").unwrap_or("outer_only");
    if brim == "no_brim" {
        let tall_thin: Vec<&str> = objects
            .iter()
            .filter(|o| {
                let (min, max) = o.bounds();
                let size = max - min;
                let footprint = size.x.min(size.y);
                footprint > 0. && footprint < 30. && size.z > footprint * 4.
            })
            .map(|o| short(&o.name))
            .collect();
        if !tall_thin.is_empty() {
            out.push(Guard {
                message: format!(
                    "{} {} tall for {} footprint. A small brim helps it stay put.",
                    tall_thin.join(", "),
                    if tall_thin.len() == 1 { "is" } else { "are" },
                    if tall_thin.len() == 1 { "its" } else { "their" },
                ),
                fix: Some(Fix {
                    label: "Add 3 mm brim".into(),
                    settings: vec![
                        ("brim_type".into(), "outer_only".into()),
                        ("brim_width".into(), "3".into()),
                    ],
                }),
            });
        }
    }
    for tool in 0..config.vector_len("nozzle_diameter").max(1) {
        if config.float_at("temperature", tool) == Some(0.) {
            out.push(Guard {
                message: format!("Tool {}: nozzle temperature is 0 °C.", tool + 1),
                fix: None,
            });
        } else if let Some(message) = temperature_out_of_range(config, tool) {
            out.push(Guard { message, fix: None });
        }
    }
    support_filament_guards(config, objects, &mut out);
    // Settings the editor hides because Mica does not implement them: a profile value that
    // would change the G-code upstream is kept (and saved) but has no effect here.
    let unimplemented = crate::settings::unimplemented::in_use(config);
    if !unimplemented.is_empty() {
        out.push(Guard {
            message: format!(
                "Not supported by Mica (kept in the profile, but not applied): {}.",
                unimplemented.join("; ")
            ),
            fix: None,
        });
    }
    out
}

/// The support settings of the plate, or of one object with its own support overrides.
struct SupportSetup<'a> {
    /// `None`: the plate's settings (a fix can change them); else the object's short name.
    object: Option<&'a str>,
    enabled: bool,
    base: usize,
    interface: usize,
    top_distance: f64,
    synchronized: bool,
    organic: bool,
}

/// Supports printed with another filament: a tool the printer does not have or that has no
/// filament, a soluble interface that does not touch the model, a soluble support filament
/// that also prints the model, and PrusaSlicer's wipe tower rules for normal supports (which
/// otherwise stop the slice).
fn support_filament_guards(config: &Config, objects: &[Object], out: &mut Vec<Guard>) {
    let count = config.vector_len("nozzle_diameter").max(1);
    let orca = mica_core::rules::Rules::of(config).is_orca();
    let soluble =
        |tool: usize| tool > 0 && config.bool_at("filament_soluble", tool - 1) == Some(true);
    let names = config
        .raw("filament_settings_id")
        .and_then(|raw| mica_core::config::unescape_strings(&raw).ok())
        .unwrap_or_default();
    let plate = SupportSetup {
        object: None,
        enabled: config.bool("support_material").unwrap_or(false)
            || config.int("raft_layers").unwrap_or(0) > 0,
        base: config.int("support_material_extruder").unwrap_or(0).max(0) as usize,
        interface: config
            .int("support_material_interface_extruder")
            .unwrap_or(0)
            .max(0) as usize,
        top_distance: config
            .float("support_material_contact_distance")
            .unwrap_or(0.2),
        synchronized: config
            .bool("support_material_synchronize_layers")
            .unwrap_or(false),
        organic: config.string("support_material_style") == Some("organic"),
    };
    let keys = [
        "support_material",
        "raft_layers",
        "support_material_extruder",
        "support_material_interface_extruder",
        "support_material_contact_distance",
        "support_material_synchronize_layers",
        "support_material_style",
    ];
    let mut setups = Vec::new();
    let mut plate_used = objects.is_empty();
    for o in objects {
        if !keys.iter().any(|k| o.overrides.contains_key(*k)) {
            plate_used = true;
            continue;
        }
        let get = |k: &str| o.overrides.get(k).map(String::as_str);
        let num = |k: &str, d: f64| {
            get(k)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(d)
        };
        let flag = |k: &str, d: bool| get(k).map_or(d, |v| matches!(v.trim(), "1" | "true"));
        let p = &plate;
        let enabled = flag(
            "support_material",
            config.bool("support_material").unwrap_or(false),
        ) || num("raft_layers", config.int("raft_layers").unwrap_or(0) as f64) > 0.;
        let setup = SupportSetup {
            object: Some(short(&o.name)),
            enabled,
            base: num("support_material_extruder", p.base as f64).max(0.) as usize,
            interface: num("support_material_interface_extruder", p.interface as f64).max(0.)
                as usize,
            top_distance: num("support_material_contact_distance", p.top_distance),
            synchronized: flag("support_material_synchronize_layers", p.synchronized),
            organic: get("support_material_style").map_or(p.organic, |v| v == "organic"),
        };
        setups.push(setup);
    }
    if plate_used {
        setups.insert(0, plate);
    }
    let tower = !orca
        && config.bool("wipe_tower").unwrap_or(false)
        && !config.bool("spiral_vase").unwrap_or(false)
        && count > 1;
    for setup in setups.iter().filter(|s| s.enabled) {
        let whose = setup
            .object
            .map_or(String::new(), |name| format!("{name}: "));
        let plate_fix = |fix: Fix| setup.object.is_none().then_some(fix);
        // A tool the printer does not have, or one without a filament.
        for (tool, what, key) in [
            (setup.base, "support base", "support_material_extruder"),
            (
                setup.interface,
                "support interface",
                "support_material_interface_extruder",
            ),
        ] {
            if tool > count {
                out.push(Guard {
                    message: format!(
                        "{whose}The {what} prints with tool {tool}, but the printer has {count} tool{}.",
                        if count == 1 { "" } else { "s" }
                    ),
                    fix: plate_fix(Fix::one("Use Default", key, "0")),
                });
            } else if tool > 0 && names.get(tool - 1).is_some_and(|n| n.trim().is_empty()) {
                out.push(Guard {
                    message: format!(
                        "{whose}The {what} prints with tool {tool}, which has no filament assigned."
                    ),
                    fix: None,
                });
            }
        }
        // A soluble interface (or all-soluble supports) normally touches the model.
        let soluble_tool = if soluble(setup.interface) {
            Some(setup.interface)
        } else if soluble(setup.base) && (setup.interface == 0 || setup.interface == setup.base) {
            Some(setup.base)
        } else {
            None
        };
        let mut gap_fixed = false;
        if let Some(tool) = soluble_tool
            && setup.top_distance > 0.
        {
            gap_fixed = true;
            let mut settings = vec![(
                "support_material_contact_distance".to_string(),
                "0".to_string(),
            )];
            // PrusaSlicer's wipe tower needs synchronized layers for soluble normal supports.
            if tower && !setup.organic && !setup.synchronized {
                settings.push(("support_material_synchronize_layers".into(), "1".into()));
            }
            out.push(Guard {
                message: format!(
                    "{whose}Tool {tool} is soluble, but the supports keep a {} mm gap to the \
                     model: a soluble interface usually touches it (top contact distance 0).",
                    mm(setup.top_distance)
                ),
                fix: plate_fix(Fix {
                    label: "Set 0 mm".into(),
                    settings,
                }),
            });
        }
        // A soluble support filament that also prints the model dissolves the model too.
        for tool in [setup.base, setup.interface] {
            if !soluble(tool) {
                continue;
            }
            let models: Vec<&str> = objects
                .iter()
                .filter(|o| setup.object.is_none_or(|name| short(&o.name) == name))
                .filter(|o| o.tools.0[..3].iter().flatten().any(|&t| t + 1 == tool))
                .map(|o| short(&o.name))
                .collect();
            if !models.is_empty() {
                out.push(Guard {
                    message: format!(
                        "Tool {tool} is marked soluble and prints the supports, but it also prints \
                         {}: the model would dissolve with them. Check the filament or the tool.",
                        models.join(", ")
                    ),
                    fix: None,
                });
                break;
            }
        }
        // PrusaSlicer refuses these wipe tower combinations (Print::validate).
        if tower && !setup.organic {
            if setup.top_distance == 0. && !setup.synchronized {
                out.push(Guard {
                    message: format!(
                        "{whose}Soluble supports with the wipe tower need the support layers \
                         synchronized with the object layers; slicing stops otherwise."
                    ),
                    fix: plate_fix(Fix::one(
                        "Synchronize",
                        "support_material_synchronize_layers",
                        "1",
                    )),
                });
            } else if setup.top_distance > 0.
                && !gap_fixed
                && (setup.base != 0 || setup.interface != 0)
            {
                out.push(Guard {
                    message: format!(
                        "{whose}With the wipe tower, supports with a gap to the model must print \
                         with the current tool (support filaments Default); slicing stops otherwise."
                    ),
                    fix: plate_fix(Fix {
                        label: "Use Default".into(),
                        settings: vec![
                            ("support_material_extruder".into(), "0".into()),
                            ("support_material_interface_extruder".into(), "0".into()),
                        ],
                    }),
                });
            }
        }
    }
}

/// A tool's nozzle temperature (first layer or later) outside the range its filament
/// declares (`nozzle_temperature_range_low/high`), which `mica validate` reports as an
/// error in the sliced G-code.
pub fn temperature_out_of_range(config: &Config, tool: usize) -> Option<String> {
    // Orca's range keys are not PrusaSlicer options: they live among the extras as text.
    let bound = |key| {
        config
            .float_at(key, tool)
            .or_else(|| {
                let raw = config.extra(key)?;
                let values: Vec<&str> = raw.split(',').collect();
                values.get(tool).or(values.first())?.trim().parse().ok()
            })
            .filter(|v: &f64| v.is_finite() && *v > 0.)
    };
    let (low, high) = (
        bound("nozzle_temperature_range_low"),
        bound("nozzle_temperature_range_high"),
    );
    if low.is_none() && high.is_none() {
        return None;
    }
    let range = format!(
        "{}–{} °C",
        low.map_or("…".into(), |v| format!("{v:.0}")),
        high.map_or("…".into(), |v| format!("{v:.0}"))
    );
    for (key, what) in [
        ("first_layer_temperature", "first-layer nozzle temperature"),
        ("temperature", "nozzle temperature"),
    ] {
        let Some(t) = config.float_at(key, tool).filter(|t| *t > 0.) else {
            continue;
        };
        if low.is_some_and(|l| t < l) || high.is_some_and(|h| t > h) {
            return Some(format!(
                "Tool {}: {what} {t:.0} °C is outside the filament's range {range}.",
                tool + 1
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PrusaSlicer's defaults on a Marlin 2 printer with extra perimeters off and relative
    /// extrusion on, as every owner preset has: nothing Mica does not support.
    fn base() -> Config {
        let mut c = Config::defaults();
        c.set_raw("gcode_flavor", "marlin2").unwrap();
        c.set_raw("extra_perimeters", "0").unwrap();
        c.set_raw("use_relative_e_distances", "1").unwrap();
        c
    }

    #[test]
    fn layer_height_and_walls() {
        let mut c = base();
        c.set_raw("nozzle_diameter", "0.4").unwrap();
        c.set_raw("layer_height", "0.35").unwrap();
        c.set_raw("first_layer_height", "0.45").unwrap();
        c.set_raw("perimeters", "0").unwrap();
        c.set_raw("brim_type", "outer_only").unwrap();
        let g = guard_rails(&c, &[]);
        assert_eq!(g.len(), 3, "{g:?}");
        assert_eq!(g[0].fix.as_ref().unwrap().settings[0].1, "0.3");
        assert_eq!(
            g[1].fix.as_ref().unwrap().settings[0].0,
            "first_layer_height"
        );
        assert_eq!(g[2].fix.as_ref().unwrap().settings[0].1, "2");
        c.set_raw("layer_height", "0.2").unwrap();
        c.set_raw("first_layer_height", "0.2").unwrap();
        c.set_raw("perimeters", "2").unwrap();
        assert!(guard_rails(&c, &[]).is_empty());
        c.set_raw("layer_height", "0.02").unwrap();
        assert!(guard_rails(&c, &[])[0].message.contains("very thin"));
    }

    /// A printer with absolute extrusion (15 stock Orca printers ship one) gets a notice as
    /// soon as it is selected, with a fix that turns relative E distances on in the printer.
    #[test]
    fn absolute_extrusion_gets_a_notice_with_a_printer_fix() {
        let mut c = base();
        c.set_raw("use_relative_e_distances", "0").unwrap();
        let g = guard_rails(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(
            g[0].message
                .starts_with(mica_core::safety::ABSOLUTE_E_UNSUPPORTED)
        );
        let fix = g[0].fix.as_ref().unwrap();
        assert_eq!(
            fix.settings,
            vec![("use_relative_e_distances".to_string(), "1".to_string())]
        );
        // The fix changes the printer, not the print profile.
        assert!(crate::settings::on_printer_tab(&fix.settings[0].0));
        assert!(!crate::settings::on_printer_tab("layer_height"));
        c.set_raw("use_relative_e_distances", "1").unwrap();
        assert!(guard_rails(&c, &[]).is_empty());
    }

    #[test]
    fn tall_thin_part_without_brim() {
        let mut c = base();
        c.set_raw("brim_type", "no_brim").unwrap();
        let mesh = mica_core::mesh::TriangleMesh {
            vertices: vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [5., 5., 80.]],
            faces: vec![[0, 1, 2], [0, 1, 3], [1, 2, 3], [2, 0, 3]],
        };
        let object = Object::new(mica_core::input::InputObject::plain(
            "spire.stl",
            0,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([Some(0); 5]),
            Config::defaults(),
        ));
        let g = guard_rails(&c, std::slice::from_ref(&object));
        assert_eq!(g.len(), 1);
        assert_eq!(
            g[0].message,
            "spire is tall for its footprint. A small brim helps it stay put."
        );
        let fix = g[0].fix.as_ref().unwrap();
        assert_eq!(fix.label, "Add 3 mm brim");
        assert_eq!(fix.settings.len(), 2);
        assert_eq!(fix.settings[0], ("brim_type".into(), "outer_only".into()));
        assert_eq!(fix.settings[1], ("brim_width".into(), "3".into()));
        let mut squat = object;
        squat.scale.z = 0.1;
        assert!(guard_rails(&c, &[squat]).is_empty());
    }

    /// A nozzle temperature outside the filament's declared range slices into G-code that
    /// `mica validate` rejects (hot/cold extrusion): the guard says so before slicing.
    #[test]
    fn temperature_outside_the_filament_range() {
        let mut c = base();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        c.set_raw("temperature", "215,300").unwrap();
        c.set_raw("first_layer_temperature", "215,230").unwrap();
        c.set_raw("nozzle_temperature_range_low", "190,220")
            .unwrap();
        c.set_raw("nozzle_temperature_range_high", "240,250")
            .unwrap();
        assert_eq!(temperature_out_of_range(&c, 0), None);
        let message = temperature_out_of_range(&c, 1).unwrap();
        assert!(
            message.contains("Tool 2") && message.contains("300"),
            "{message}"
        );
        assert!(
            guard_rails(&c, &[])
                .iter()
                .any(|g| g.message.contains("outside the filament's range"))
        );
        c.set_raw("temperature", "215,210").unwrap();
        assert!(temperature_out_of_range(&c, 1).unwrap().contains("210"));
    }

    /// Supports in another filament: the guards and their fixes.
    #[test]
    fn support_filament_guards() {
        let mut c = base();
        for (k, v) in [
            ("nozzle_diameter", "0.4,0.4"),
            ("filament_soluble", "0,1"),
            ("support_material", "1"),
            ("support_material_style", "organic"),
            ("support_material_interface_extruder", "2"),
            ("support_material_contact_distance", "0.2"),
        ] {
            c.set_raw(k, v).unwrap();
        }
        let messages = |c: &Config, objects: &[Object]| -> Vec<Guard> {
            guard_rails(c, objects)
                .into_iter()
                .filter(|g| !g.message.starts_with("Not supported"))
                .collect()
        };
        // A soluble interface with a gap to the model: set the gap to 0.
        let g = messages(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(
            g[0].message.contains("Tool 2 is soluble"),
            "{}",
            g[0].message
        );
        let fix = g[0].fix.as_ref().unwrap();
        assert_eq!(
            fix.settings,
            [("support_material_contact_distance".into(), "0".into())]
        );
        // Normal supports on PrusaSlicer's wipe tower also need synchronized layers.
        c.set_raw("support_material_style", "grid").unwrap();
        c.set_raw("wipe_tower", "1").unwrap();
        let g = messages(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(g[0].fix.as_ref().unwrap().settings.len(), 2);
        c.set_raw("support_material_contact_distance", "0").unwrap();
        let g = messages(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(
            g[0].fix.as_ref().unwrap().settings,
            [("support_material_synchronize_layers".into(), "1".into())]
        );
        c.set_raw("support_material_synchronize_layers", "1")
            .unwrap();
        assert!(messages(&c, &[]).is_empty());
        // A non-soluble support tool with a gap and the tower: back to Default.
        c.set_raw("filament_soluble", "0,0").unwrap();
        c.set_raw("support_material_contact_distance", "0.2")
            .unwrap();
        let g = messages(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(g[0].fix.as_ref().unwrap().label, "Use Default");
        // Orca rules have no such tower rule.
        mica_core::rules::Rules::OrcaSlicer.set(&mut c);
        assert!(messages(&c, &[]).is_empty());
        // A tool the printer does not have.
        c.set_raw("support_material_interface_extruder", "4")
            .unwrap();
        let g = messages(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(g[0].message.contains("tool 4"), "{}", g[0].message);
        assert_eq!(
            g[0].fix.as_ref().unwrap().settings,
            [("support_material_interface_extruder".into(), "0".into())]
        );
        // A soluble support filament that also prints the model.
        c.set_raw("support_material_interface_extruder", "2")
            .unwrap();
        c.set_raw("support_material_contact_distance", "0").unwrap();
        c.set_raw("filament_soluble", "0,1").unwrap();
        let mesh = mica_core::mesh::TriangleMesh {
            vertices: vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [5., 5., 10.]],
            faces: vec![[0, 1, 2], [0, 1, 3], [1, 2, 3], [2, 0, 3]],
        };
        let object = Object::new(mica_core::input::InputObject::plain(
            "duck.stl",
            0,
            0,
            mesh,
            mica_core::print_job::ToolAssignments([Some(1), Some(1), Some(1), None, Some(1)]),
            Config::defaults(),
        ));
        let g = messages(&c, std::slice::from_ref(&object));
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(
            g[0].message.contains("also prints duck"),
            "{}",
            g[0].message
        );
        assert!(g[0].fix.is_none());
    }

    /// A profile value for a setting the editor hides (Mica does not implement it) is said,
    /// not silently kept.
    #[test]
    fn unsupported_profile_settings_are_reported() {
        let mut c = base();
        c.set_raw("gcode_flavor", "marlin2").unwrap();
        c.set_raw("extra_perimeters", "0").unwrap();
        assert!(
            guard_rails(&c, &[]).is_empty(),
            "{:?}",
            guard_rails(&c, &[])
        );
        c.set_raw("use_firmware_retraction", "1").unwrap();
        c.set_raw("support_material_pattern", "honeycomb").unwrap();
        let g = guard_rails(&c, &[]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(
            g[0].message.starts_with("Not supported by Mica"),
            "{}",
            g[0].message
        );
        assert!(g[0].message.contains(": on;"), "{}", g[0].message);
        assert!(g[0].message.contains("Honeycomb"), "{}", g[0].message);
    }
}
