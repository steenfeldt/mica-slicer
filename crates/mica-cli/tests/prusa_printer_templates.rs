// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Every bundled Prusa printer's start/end (and tool change) templates must run with one,
//! two and all of its tools in use, as they do in PrusaSlicer 2.9.6. The XL 2T's start
//! G-code guards its third to fifth tool purges with `is_extruder_used[2..4]`; PrusaSlicer
//! pads that vector to 255 entries, so those reads are false instead of falling back to
//! element 0 (which made Mica run `e_retracted[2] = …` on a two-tool printer).
//!
//! Printers whose start, end and tool change templates and tool count are identical are
//! sliced once, with their default (or first compatible) print and filament presets. The
//! corpus printers (CORE One, XL 5T, MINI IS, MK3.5) are always among them, with the corpus
//! presets.

use mica_profiles::prusa::{Bundle, Kind};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// A 10 x 10 x 1 mm box, so each case slices only a few layers.
fn write_box(path: &Path) {
    let (x, y, z) = (10.0f32, 10.0f32, 1.0f32);
    let v = [
        [0., 0., 0.],
        [x, 0., 0.],
        [x, y, 0.],
        [0., y, 0.],
        [0., 0., z],
        [x, 0., z],
        [x, y, z],
        [0., y, z],
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    let mut s = String::from("solid box\n");
    for f in faces {
        s += "facet normal 0 0 0\nouter loop\n";
        for i in f {
            let p: [f32; 3] = v[i];
            s += &format!("vertex {} {} {}\n", p[0], p[1], p[2]);
        }
        s += "endloop\nendfacet\n";
    }
    s += "endsolid box\n";
    std::fs::write(path, s).unwrap();
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

struct Case {
    printer: String,
    print: String,
    filament: String,
    tools: usize,
}

/// The corpus printers (tests/corpus/manifest.json) with their presets: they come first, so
/// a printer sharing their templates does not stand in for them.
const CORPUS_PRINTERS: [(&str, &str, &str); 4] = [
    (
        "Prusa CORE One HF0.4 nozzle",
        "0.20mm SPEED @COREONE HF0.4",
        "Prusament PLA @COREONE HF0.4",
    ),
    (
        "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
        "0.20mm SPEED @XLIS 0.4",
        "Prusament PLA @XLIS",
    ),
    (
        "Original Prusa MINI & MINI+ Input Shaper",
        "0.20mm SPEED @MINIIS 0.4",
        "Prusament PLA @MINIIS",
    ),
    (
        "Original Prusa MK3.5 & MK3.5S 0.4 nozzle",
        "0.20mm SPEED @MK3.5 0.4",
        "Prusament PLA @MK3.5",
    ),
];

fn cases(bundle: &Bundle) -> Vec<Case> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let names = bundle.preset_names(Kind::Printer);
    let first: Vec<&str> = CORPUS_PRINTERS.iter().map(|c| c.0).collect();
    let ordered = first
        .iter()
        .copied()
        .chain(names.into_iter().filter(|n| !first.contains(n)));
    for printer in ordered {
        let Ok(flat) = bundle.flatten(Kind::Printer, printer) else {
            continue;
        };
        if flat.get("printer_technology").map(String::as_str) == Some("SLA") {
            continue;
        }
        let get = |k: &str| flat.get(k).map_or("", String::as_str);
        // Mica writes Marlin 2 and Klipper only, and single-extruder MMU prints are not
        // supported yet (the CLI says so); those printers are not template failures.
        if !matches!(get("gcode_flavor"), "marlin2" | "klipper") {
            continue;
        }
        let mmu = get("single_extruder_multi_material") == "1";
        let tools = flat
            .get("nozzle_diameter")
            .map_or(1, |v| v.split(',').count());
        let key = (
            get("start_gcode").to_string(),
            get("end_gcode").to_string(),
            get("toolchange_gcode").to_string(),
            tools,
        );
        if seen.contains(&key) {
            continue;
        }
        let corpus = CORPUS_PRINTERS.iter().find(|c| c.0 == printer);
        // The corpus presets; else the defaults when they suit the printer; otherwise the
        // first compatible preset (listing every compatible preset is slow, so only on that
        // fallback).
        let print = corpus
            .map(|c| c.1)
            .or_else(|| flat.get("default_print_profile").map(|p| unquote(p)))
            .filter(|p| {
                bundle
                    .is_compatible_with_printer(Kind::Print, p, printer)
                    .unwrap_or(false)
            })
            .or_else(|| {
                bundle
                    .compatible_presets(Kind::Print, printer, None)
                    .first()
                    .copied()
            });
        let Some(print) = print else { continue };
        let filament = corpus
            .map(|c| c.2)
            .or_else(|| {
                flat.get("default_filament_profile")
                    .and_then(|f| f.split(';').next())
                    .map(unquote)
            })
            .filter(|f| {
                bundle
                    .is_compatible_with_printer(Kind::Filament, f, printer)
                    .unwrap_or(false)
                    && bundle.is_compatible_with_print(f, print).unwrap_or(false)
            })
            .or_else(|| {
                bundle
                    .compatible_presets(Kind::Filament, printer, Some(print))
                    .first()
                    .copied()
            });
        let Some(filament) = filament else { continue };
        seen.insert(key);
        let max = if mmu { 1 } else { tools };
        let counts: BTreeSet<usize> = [1, 2, max].into_iter().filter(|&k| k <= max).collect();
        for k in counts {
            out.push(Case {
                printer: printer.to_string(),
                print: print.to_string(),
                filament: filament.to_string(),
                tools: k,
            });
        }
    }
    out
}

#[test]
fn prusa_printer_templates_run_with_one_two_and_all_tools() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let text = std::fs::read_to_string(mica_testkit::prusa_bundle()).unwrap();
    let bundle = Bundle::parse(&text);
    let cases = cases(&bundle);
    assert!(
        cases
            .iter()
            .any(|c| c.printer.starts_with("Original Prusa XL - 2T Input Shaper") && c.tools == 2),
        "the XL 2T two-tool case must be covered"
    );
    for (printer, print, filament) in CORPUS_PRINTERS {
        assert!(
            cases
                .iter()
                .any(|c| c.printer == printer && c.print == print && c.filament == filament),
            "{printer} with {print} / {filament} must be covered"
        );
    }
    let dir = std::env::temp_dir().join(format!("mica-prusa-templates-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("box.stl");
    write_box(&model);
    let run = |i: usize, c: &Case| -> Option<String> {
        let output = dir.join(format!("{i}.gcode"));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mica"));
        cmd.env("MICA_CONFIG_DIR", dir.join(format!("settings-{i}")))
            .env("SOURCE_DATE_EPOCH", "0")
            .arg("slice");
        for _ in 0..c.tools {
            cmd.arg(&model);
        }
        cmd.arg("--bundle")
            .arg(mica_testkit::prusa_bundle())
            .args(["--printer", &c.printer, "--print", &c.print])
            .args(["--filament", &c.filament]);
        for t in 1..=c.tools {
            cmd.args(["--object-extruder", &format!("{t}={t}")]);
        }
        let result = cmd.arg("-o").arg(&output).output().unwrap();
        let _ = std::fs::remove_file(&output);
        (!result.status.success()).then(|| {
            format!(
                "{} / {} / {} with {} tool(s): {}",
                c.printer,
                c.print,
                c.filament,
                c.tools,
                String::from_utf8_lossy(&result.stderr).trim()
            )
        })
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let (cases, run) = (&cases, &run);
                s.spawn(move || {
                    cases
                        .iter()
                        .enumerate()
                        .skip(t)
                        .step_by(threads)
                        .filter_map(|(i, c)| run(i, c))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// The XL 2T with one object per tool: the start G-code purges the second tool, then the
/// initial one, and skips the third to fifth tool blocks, exactly like PrusaSlicer 2.9.6
/// (tool, park, pick and purge lines of its output for the same two-object project).
#[test]
fn xl_2t_two_tool_start_gcode_matches_prusaslicer() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let bundle_path = mica_testkit::prusa_bundle();
    let dir = std::env::temp_dir().join(format!("mica-xl2t-start-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("box.stl");
    write_box(&model);
    let output = dir.join("out.gcode");
    let result = Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_CONFIG_DIR", dir.join("settings"))
        .env("SOURCE_DATE_EPOCH", "0")
        .arg("slice")
        .arg(&model)
        .arg(&model)
        .arg("--bundle")
        .arg(&bundle_path)
        .args([
            "--printer",
            "Original Prusa XL - 2T Input Shaper 0.4 nozzle",
            "--print",
            "0.20mm SPEED @XLIS 0.4",
            "--filament",
            "Prusament PLA @XLIS",
            "--object-extruder",
            "1=1",
            "--object-extruder",
            "2=2",
            "-o",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let g = std::fs::read_to_string(&output).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let start: Vec<&str> = g
        .lines()
        .take_while(|l| *l != ";LAYER_CHANGE")
        .filter(|l| {
            l.starts_with("; purge")
                || l.starts_with("M109 T")
                || l.starts_with("P0 ")
                || (l.starts_with('T') && l.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
        })
        .collect();
    assert_eq!(
        start,
        [
            "T1 S1 L0 D0",
            "T0 S1 L0 D0",
            "M109 T0 S170 ; wait for temp",
            "M109 T0 S170 ; wait for temp",
            "P0 S1 L1 D0; park the tool",
            "; purge second tool",
            "P0 S1 L2 D0; park the tool",
            "M109 T1 S230",
            "T1 S1 L0 D0; pick the tool",
            "; purge initial tool",
            "P0 S1 L2 D0; park the tool",
            "M109 T0 S230",
            "T0 S1 L0 D0; pick the tool",
            "T0 S1 L0 D0",
        ]
    );
}
