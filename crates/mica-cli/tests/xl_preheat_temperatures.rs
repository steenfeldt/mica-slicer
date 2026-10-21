// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! XL multi-tool temperature lines against PrusaSlicer 2.9.6 (fixture: lines extracted from its
//! output for the same synthetic projects). PrusaSlicer's tool change preheat (`M104.1`) takes
//! `temperature` for every tool change of a multi-layer print, first layer included: its
//! post-processor reads the layer counter after the whole program was analyzed. Mica wrote
//! `first_layer_temperature` there (230 instead of 225 for Prusament PLA).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// M104/M109 lines of layers 0-3 and 98+ (`L<layer> <code>`) and the distinct M104.1
/// tool/temperature pairs per phase, as `fixture.py` writes them for the reference.
fn extract(gcode: &str) -> (Vec<String>, BTreeSet<String>) {
    let (mut lines, mut preheat, mut layer) = (Vec::new(), BTreeSet::new(), 0);
    for line in gcode.lines() {
        let s = line.trim();
        if s == ";LAYER_CHANGE" {
            layer += 1;
        }
        let code = s.split(';').next().unwrap_or("").trim();
        match code.split(' ').next().unwrap_or("") {
            "M104.1" => {
                let kept: Vec<&str> = code
                    .split(' ')
                    .filter(|w| !w.starts_with('P') && !w.starts_with('Q'))
                    .collect();
                let phase = match layer {
                    0 => "start",
                    1 => "first",
                    _ => "later",
                };
                preheat.insert(format!("preheat {phase} {}", kept.join(" ")));
            }
            "M104" | "M109" if layer <= 3 || layer >= 98 => lines.push(format!("L{layer} {code}")),
            _ => {}
        }
    }
    (lines, preheat)
}

fn fixture() -> BTreeMap<String, (Vec<String>, BTreeSet<String>)> {
    let text = include_str!("fixtures/xl_preheat_temperatures.txt");
    let mut cases = BTreeMap::new();
    let mut current = None;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name.to_string());
            cases.insert(name.to_string(), (Vec::new(), BTreeSet::new()));
            continue;
        }
        let entry = cases.get_mut(current.as_ref().unwrap()).unwrap();
        if line.starts_with("preheat ") {
            entry.1.insert(line.to_string());
        } else {
            entry.0.push(line.to_string());
        }
    }
    cases
}

#[test]
fn xl_multi_tool_temperature_lines_match_prusaslicer() {
    if !mica_testkit::require_prusa_profiles() {
        return;
    }
    let bundle = mica_testkit::prusa_bundle();
    let cube = root().join("tests/models/cube20.stl");
    let painted = root().join("tests/models/painted-cube.3mf");
    let xl2 = "Original Prusa XL - 2T Input Shaper 0.4 nozzle";
    let xl5 = "Original Prusa XL - 5T Input Shaper 0.4 nozzle";
    let cases: [(&str, &str, Vec<&std::path::Path>, &[&str]); 3] = [
        ("xl2_two", xl2, vec![&cube, &cube], &[]),
        (
            "xl5_three",
            xl5,
            vec![&cube, &cube, &cube],
            &[
                "--set",
                "first_layer_temperature=230,240,265,230,230",
                "--set",
                "temperature=225,250,260,225,225",
                "--set",
                "idle_temperature=70,120,150,70,70",
            ],
        ),
        ("painted", xl2, vec![&painted], &[]),
    ];
    let reference = fixture();
    let dir = std::env::temp_dir().join(format!("mica-xl-preheat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, printer, models, extra) in cases {
        let output = dir.join(format!("{name}.gcode"));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mica"));
        cmd.env("MICA_CONFIG_DIR", dir.join(format!("settings-{name}")))
            .env("SOURCE_DATE_EPOCH", "0")
            .arg("slice")
            .args(&models)
            .arg("--bundle")
            .arg(&bundle)
            .args(["--printer", printer, "--print", "0.20mm SPEED @XLIS 0.4"])
            .args(["--filament", "Prusament PLA @XLIS"]);
        if models.len() > 1 {
            for t in 1..=models.len() {
                cmd.args(["--object-extruder", &format!("{t}={t}")]);
            }
        }
        let result = cmd.args(extra).arg("-o").arg(&output).output().unwrap();
        assert!(
            result.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let (lines, preheat) = extract(&std::fs::read_to_string(&output).unwrap());
        let (want_lines, want_preheat) = &reference[name];
        assert_eq!(&preheat, want_preheat, "{name}: M104.1 temperatures");
        assert_eq!(&lines, want_lines, "{name}: M104/M109 lines");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
