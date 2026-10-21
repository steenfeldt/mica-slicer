// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/libslic3r/GCode.cpp :: GCode::process_layer
// Source: orca@8500fcdcca src/libslic3r/calib.cpp :: CalibPressureAdvanceLine::print_pa_lines, CalibPressureAdvancePattern::generate_custom_gcodes
// Changes: Typed engine controls at path boundaries; Buddy M572 and Klipper PA, runtime retraction/flow; normal export is untouched when controls are absent.
// Changes: On multi-tool printers temperature commands name the printing tool, and startup retraction is tracked for it.
use crate::{
    Flavor,
    writer::{Writer, decimal, num},
};
use mica_core::{calib::Control, settings::PrintSettings};

pub(crate) fn layer_settings(
    s: &PrintSettings,
    control: Option<Control>,
) -> std::borrow::Cow<'_, PrintSettings> {
    settings(
        s,
        control.map(|mut c| {
            c.flow_ratio = None;
            c
        }),
    )
}

pub(crate) fn finish(w: &mut Writer, flavor: Flavor, first: Option<Control>) {
    if first.is_some_and(|c| c.pressure_advance.is_some()) {
        emit(
            w,
            flavor,
            Some(Control {
                pressure_advance: Some(0.),
                ..Default::default()
            }),
            None,
        );
    }
}

pub(crate) fn settings(
    s: &PrintSettings,
    control: Option<Control>,
) -> std::borrow::Cow<'_, PrintSettings> {
    let Some(c) = control.filter(|c| c.retract_length.is_some() || c.flow_ratio.is_some()) else {
        return std::borrow::Cow::Borrowed(s);
    };
    let mut s = s.clone();
    if let Some(length) = c.retract_length {
        s.retract_length = length;
    }
    if let Some(ratio) = c.flow_ratio {
        s.extrusion_multiplier *= ratio;
    }
    std::borrow::Cow::Owned(s)
}
/// Account for explicit extrusion in startup scripts before a calibration. In particular the
/// XL leaves E retracted even after G92 E0. Ordinary slicing keeps its established behaviour.
/// `initial` is the tool the test prints with: its pending retraction is the one that counts.
pub(crate) fn after_start(
    w: &mut Writer,
    start: &str,
    filament: &str,
    relative_e: bool,
    initial: usize,
) {
    let mut relative = true;
    let mut tool = 0usize;
    let mut states = std::collections::BTreeMap::<usize, (f64, f64)>::new();
    for line in start
        .lines()
        .chain(std::iter::once(if relative_e { "M83" } else { "M82" }))
        .chain(filament.lines())
    {
        let mut words = line.split(';').next().unwrap_or("").split_whitespace();
        let Some(command) = words.next() else {
            continue;
        };
        match command {
            "M83" => relative = true,
            "M82" => relative = false,
            "G0" | "G1" | "G2" | "G3" | "G92" => {
                let Some(e) = words
                    .find_map(|v| v.strip_prefix('E')?.parse::<f64>().ok())
                    .filter(|e| e.is_finite())
                else {
                    continue;
                };
                let (position, pending) = states.entry(tool).or_default();
                if command != "G92" {
                    let delta = if relative { e } else { e - *position };
                    *pending = (*pending - delta).max(0.);
                }
                *position = if relative && command != "G92" {
                    *position + e
                } else {
                    e
                };
            }
            c if c.starts_with('T') => {
                if let Ok(t) = c[1..].parse::<usize>() {
                    tool = t;
                }
            }
            _ => {}
        }
    }
    let pending = states.get(&initial).map_or(0., |s| s.1);
    let (_, existing, extra, _) = w.template_state();
    if pending > existing {
        w.apply_template_state(None, pending, extra);
    }
}

/// `tool` is the printing tool on a multi-tool printer: temperature changes then name it, as
/// the multi-tool exporter's own temperature commands do.
pub(crate) fn emit(w: &mut Writer, flavor: Flavor, control: Option<Control>, tool: Option<usize>) {
    let Some(c) = control else {
        return;
    };
    if let Some(v) = c.sample {
        w.raw(&format!("; calibration sample={}", num(v, 6)));
    }
    if let Some(pa) = c.pressure_advance {
        if flavor == Flavor::Klipper {
            w.set_pressure_advance(pa);
        } else {
            w.raw(&format!(
                "M572 S{} ; calibration pressure advance",
                decimal(pa, 6)
            ));
        }
    }
    if let Some(t) = c.temperature {
        match tool {
            Some(tool) => w.raw(&format!(
                "M104 S{} T{tool} ; calibration temperature",
                num(t, 0)
            )),
            None => w.raw(&format!("M104 S{} ; calibration temperature", num(t, 0))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        config::Config,
        flow::Flow,
        geometry::{BoundingBox, Point},
        print::LayerSource,
        toolpath::{ExtrusionPath, LayerToolpaths, Role},
    };

    struct Source(Control);
    impl LayerSource for Source {
        fn calibration_control(&self, _: usize, _: usize) -> Option<Control> {
            Some(self.0)
        }
        fn layer_count(&self) -> usize {
            1
        }
        fn layer_z(&self, _: usize) -> (f64, f64) {
            (0.2, 0.2)
        }
        fn first_layer_bbox(&self) -> Option<BoundingBox> {
            Some(BoundingBox {
                min: Point::from_mm(100., 100.),
                max: Point::from_mm(120., 110.),
            })
        }
        fn toolpaths(&self, _: std::ops::Range<usize>) -> Vec<LayerToolpaths> {
            vec![LayerToolpaths {
                support_groups: Vec::new(),
                index: 0,
                print_z: 0.2,
                height: 0.2,
                paths: vec![100., 110.]
                    .into_iter()
                    .map(|y| ExtrusionPath {
                        role: Role::ExternalPerimeter,
                        perimeter_index: Some(0),
                        points: vec![Point::from_mm(100., y), Point::from_mm(120., y)],
                        flow: Flow::new(0.45, 0.2),
                        seam: None,
                        overhang: None,
                        brick: None,
                    })
                    .collect(),
            }]
        }
    }
    #[test]
    fn startup_retraction_survives_e_reset_and_tool_switches() {
        let mut w = Writer::new();
        after_start(
            &mut w,
            "M83\nG1 E5\nG1 E-0.8\nG92 E0\nT1\nG1 E-3\nT0",
            "",
            true,
            0,
        );
        assert_eq!(w.template_state().1, 0.8);
        // The tool the test prints with counts, not T0.
        let mut w = Writer::new();
        after_start(
            &mut w,
            "M83\nG1 E5\nG1 E-0.8\nG92 E0\nT1\nG1 E-3\nT0",
            "",
            true,
            1,
        );
        assert_eq!(w.template_state().1, 3.);
        let mut w = Writer::new();
        after_start(&mut w, "M82\nG92 E0\nG1 E5\nG1 E4.2\nG92 E0", "", false, 0);
        assert!((w.template_state().1 - 0.8).abs() < 1e-8);
    }
    #[test]
    fn actual_e_and_speed_follow_controls() {
        let mut config = Config::defaults();
        config.set_raw("cooling", "0").unwrap();
        config.set_raw("gcode_flavor", "marlin2").unwrap();
        config.set_raw("use_relative_e_distances", "1").unwrap();
        let s = PrintSettings::from_config(&config).unwrap();
        let control = Control {
            flow_ratio: Some(1.2),
            speed_mm_s: Some(100.),
            retract_length: Some(1.6),
            ..Default::default()
        };
        let out =
            crate::export::export_gcode(&Source(control), &config, &s, "calibration").unwrap();
        let expected = 40. * Flow::new(0.45, 0.2).mm3_per_mm() * s.extrusion_multiplier * 1.2
            / (std::f64::consts::PI * (s.filament_diameter / 2.).powi(2));
        assert!(
            (out.stats.filament_mm - expected).abs() < 1e-4,
            "{} vs {}",
            out.stats.filament_mm,
            expected
        );
        assert!(out.gcode.contains("G1 F6000"));
        assert!(out.gcode.contains("G1 E-1.6 "), "{}", out.gcode);
    }
    /// A test for tool 3 of a three-tool printer prints as a plate assigned to that tool: the
    /// templates see it as the initial and only used tool, it is selected once, and the
    /// calibration temperature commands name it.
    #[test]
    fn multi_tool_calibration_prints_on_its_tool() {
        let mut config = Config::defaults();
        for (k, v) in [
            ("gcode_flavor", "marlin2"),
            ("use_relative_e_distances", "1"),
            ("nozzle_diameter", "0.4,0.4,0.4"),
            (
                "start_gcode",
                "; initial={initial_tool} used={is_extruder_used[0]}{is_extruder_used[1]}{is_extruder_used[2]}",
            ),
        ] {
            config.set_raw(k, v).unwrap();
        }
        let mut p = mica_core::calib::Parameters::new(mica_core::calib::Kind::Temperature);
        (p.start, p.end, p.tool) = (220., 215., 2);
        let print = mica_core::calib::generate(&p, &config)
            .unwrap()
            .prepare(&config)
            .unwrap();
        let out =
            crate::export::export_gcode(&print, &print.config, &print.settings, "calibration")
                .unwrap();
        let g = &out.gcode;
        assert!(
            g.contains("; initial=2 used=falsefalsetrue"),
            "{}",
            &g[..2000]
        );
        let selects: Vec<&str> = g
            .lines()
            .filter(|l| l.starts_with('T') && l[1..].starts_with(char::is_numeric))
            .collect();
        assert_eq!(selects, ["T2"]);
        assert!(g.contains("M104 S220 T2 ; calibration temperature"));
        assert!(g.contains("M104 S215 T2 ; calibration temperature"));
        assert!(!g.contains("M104 S220 ; calibration temperature"));
    }
    #[test]
    fn calibration_commands_follow_startup_and_reset_pa() {
        for (flavor, command) in [
            ("marlin2", "M572 S0.02"),
            ("klipper", "SET_PRESSURE_ADVANCE ADVANCE=0.02"),
        ] {
            let mut config = Config::defaults();
            config.set_raw("gcode_flavor", flavor).unwrap();
            config.set_raw("use_relative_e_distances", "1").unwrap();
            config.set_raw("start_gcode", "; startup complete").unwrap();
            let s = PrintSettings::from_config(&config).unwrap();
            let out = crate::export::export_gcode(
                &Source(Control {
                    pressure_advance: Some(0.02),
                    temperature: Some(220.),
                    ..Default::default()
                }),
                &config,
                &s,
                "calibration",
            )
            .unwrap();
            let start = out.gcode.find("; startup complete").unwrap();
            assert!(out.gcode.find(command).unwrap() > start, "{}", out.gcode);
            assert!(out.gcode.contains("M104 S220 ; calibration temperature"));
            let reset = if flavor == "klipper" {
                "SET_PRESSURE_ADVANCE ADVANCE=0;"
            } else {
                "M572 S0 ; calibration pressure advance"
            };
            assert!(out.gcode.contains(reset), "{}", out.gcode);
        }
    }
}
