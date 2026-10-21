// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_core::config::Config;
use mica_gcode::time_estimate::{Analysis, Estimate, estimate};
use mica_gcode::writer::MoveStats;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Cursor, Seek};
use std::path::{Path, PathBuf};

fn footer_config(src: &mut (impl BufRead + Seek)) -> Config {
    let mut config = Config::defaults();
    let mut line = String::new();
    loop {
        line.clear();
        if src.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if let Some((k, v)) = line
            .strip_prefix("; ")
            .and_then(|s| s.trim_end().split_once(" = "))
        {
            // Explicitly exclude the oracle values from estimator input.
            if k.starts_with("machine_")
                || matches!(
                    k,
                    "gcode_flavor"
                        | "silent_mode"
                        | "remaining_times"
                        | "filament_diameter"
                        | "use_volumetric_e"
                        | "printer_notes"
                )
            {
                config.set_raw(k, v).unwrap();
            }
        }
    }
    src.rewind().unwrap();
    config
}

fn assert_times(actual: Estimate, normal: f64, silent: f64, first: [f64; 2]) {
    assert!((actual.normal / normal - 1.0).abs() < 0.01, "{actual:?}");
    assert!(
        (actual.silent.unwrap() / silent - 1.0).abs() < 0.01,
        "{actual:?}"
    );
    assert!(
        (actual.first_layer_normal - first[0]).abs() < 1.0,
        "{actual:?}"
    );
    assert!(
        (actual.first_layer_silent.unwrap() - first[1]).abs() < 1.0,
        "{actual:?}"
    );
}

fn progress(src: impl BufRead) -> BTreeSet<(usize, String)> {
    let mut motion = 0;
    let mut result = BTreeSet::new();
    for line in src.lines() {
        let line = line.unwrap();
        let cmd = line.split_whitespace().next().unwrap_or("");
        if matches!(cmd, "G0" | "G1" | "G2" | "G3" | "G28") {
            motion += 1;
        }
        if line.starts_with("M73 P") || line.starts_with("M73 Q") {
            result.insert((motion, line));
        }
    }
    result
}

#[test]
fn prusaslicer_cube_estimates_and_every_m73_position() {
    // Reuse the existing binary-codec oracle; do not add another full G-code fixture.
    let fixture = include_bytes!("fixtures/cube20_ps.gcode");
    let mut src = Cursor::new(fixture);
    let config = footer_config(&mut src);
    let mut analysis = Analysis::new(&mut src, &config).unwrap();
    assert_times(analysis.estimate, 923.0, 998.0, [35.0, 35.0]);
    let mut output = Vec::new();
    let stats = analysis
        .write(&mut src, &mut output, MoveStats::default())
        .unwrap();
    assert_eq!(stats.time_s, analysis.estimate.normal);
    assert_eq!(
        progress(Cursor::new(fixture)),
        progress(Cursor::new(output))
    );
}

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "needs the PrusaSlicer 2.9.6 reference app (out/ref or MICA_PRUSASLICER + MICA_PRUSA_DATADIR): cargo test -- --ignored"]
fn live_prusaslicer_cube_and_benchy() {
    let root = mica_testkit::root().join("out/ref");
    let app = std::env::var_os("MICA_PRUSASLICER")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("PrusaSlicer-2.9.6.app/Contents/MacOS/PrusaSlicer"));
    let data = std::env::var_os("MICA_PRUSA_DATADIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("datadir"));
    assert!(
        app.is_file() && data.is_dir(),
        "the live timing oracle needs PrusaSlicer 2.9.6: set MICA_PRUSASLICER and MICA_PRUSA_DATADIR"
    );
    let temp =
        Temp(std::env::temp_dir().join(format!("mica-time-ps-oracle-{}", std::process::id())));
    std::fs::create_dir_all(&temp.0).unwrap();
    for (model, expected, first) in [
        ("cube20", [923.0, 998.0], [35.0, 35.0]),
        ("3DBenchy", [2308.0, 2666.0], [57.0, 59.0]),
    ] {
        let reference = temp.0.join(format!("{model}.gcode"));
        let result = std::process::Command::new(&app)
            .arg("--datadir")
            .arg(&data)
            .args([
                "--printer-profile",
                "Prusa CORE One HF0.4 nozzle",
                "--print-profile",
                "0.20mm SPEED @COREONE HF0.4",
                "--material-profile",
                "Prusament PLA @COREONE HF0.4",
                "--center",
                "125,110",
                "--no-binary-gcode",
                "--export-gcode",
                "-o",
            ])
            .arg(&reference)
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../tests/models/{model}.stl")),
            )
            .current_dir(&temp.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut src = BufReader::new(std::fs::File::open(&reference).unwrap());
        let config = footer_config(&mut src);
        let mut analysis = Analysis::new(&mut src, &config).unwrap();
        assert_times(analysis.estimate, expected[0], expected[1], first);
        let output = temp.0.join(format!("{model}-retimed.gcode"));
        analysis
            .write(
                &mut src,
                &mut std::fs::File::create(&output).unwrap(),
                MoveStats::default(),
            )
            .unwrap();
        let original = progress(BufReader::new(std::fs::File::open(&reference).unwrap()));
        let rewritten = progress(BufReader::new(std::fs::File::open(&output).unwrap()));
        let equal = original.intersection(&rewritten).count();
        // Require >99% exact line positions and values, across both modes and the whole print.
        assert!(
            equal as f64 / original.len() as f64 > 0.99,
            "{model}: {equal}/{} M73 matches",
            original.len()
        );
        eprintln!(
            "{model}: {:?}, {equal}/{} M73 matches",
            analysis.estimate,
            original.len()
        );
    }
}

#[test]
fn postprocess_fills_headers_and_handles_no_final_newline() {
    let mut c = Config::defaults();
    c.set_raw("silent_mode", "1").unwrap();
    c.set_raw("remaining_times", "1").unwrap();
    let text = ";_GP_FIRST_LINE_M73_PLACEHOLDER\n; seconds __MICA_PRINT_TIME_SEC__\nG1 X10 F600\nG1 X20\n;_GP_LAST_LINE_M73_PLACEHOLDER\n;_GP_ESTIMATED_PRINTING_TIME_PLACEHOLDER";
    let mut src = Cursor::new(text);
    let mut a = Analysis::new(&mut src, &c).unwrap();
    let mut output = Vec::new();
    let stats = a
        .write(&mut src, &mut output, MoveStats::default())
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains(&format!("; seconds {:.2}", stats.time_s)));
    assert!(output.starts_with("M73 P0 R0\nM73 Q0 S0\n"));
    assert!(output.contains("M73 P100 R0\nM73 Q100 S0\n"));
    assert!(output.contains("estimated first layer printing time (silent mode)"));
    assert!(!output.contains("PLACEHOLDER"));
    c.set_raw("remaining_times", "0").unwrap();
    let mut src = Cursor::new(text);
    let mut a = Analysis::new(&mut src, &c).unwrap();
    let mut output = Vec::new();
    a.write(&mut src, &mut output, MoveStats::default())
        .unwrap();
    assert!(!String::from_utf8(output).unwrap().contains("M73"));
}

#[test]
fn timing_does_not_read_statistics_comments() {
    let c = Config::defaults();
    let base = estimate(&mut Cursor::new("G1 X10 F600\nG1 X20\n"), &c).unwrap();
    let poisoned = estimate(&mut Cursor::new("; estimated printing time (normal mode) = 10d 0h 0m 0s\nM73 P50 R500\nG1 X10 F600\nG1 X20\n"), &c).unwrap();
    assert_eq!(base, poisoned);
}
