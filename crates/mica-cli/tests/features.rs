// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Settings › Features (`[features]` in the settings folder's mica.toml) as the CLI
//! process honors them: a feature that is off slices without it and says so; `mica mcp`
//! refuses to start with Mica MCP off.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn settings(dir: &Path, name: &str, features: &str) -> PathBuf {
    let settings = dir.join(name);
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(
        settings.join("mica.toml"),
        format!("theme = \"dark\"\n\n[features]\n{features}\n"),
    )
    .unwrap();
    settings
}

fn mica(settings: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mica"))
        .env("MICA_CONFIG_DIR", settings)
        .env("SOURCE_DATE_EPOCH", "0")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mica-features-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn textures_off_in_mica_toml_slice_without_them_and_say_so() {
    let dir = temp("textures");
    let cube = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/models/cube20.stl");
    let cube = cube.to_str().unwrap();
    let slice = |settings: &Path, textured: bool, out: &str| {
        let out = dir.join(out);
        let mut args = vec!["slice", cube];
        if textured {
            args.extend([
                "--set",
                "mica_texture_skin=external",
                "--set",
                "mica_texture_skin_texture=builtin:shingles",
            ]);
        }
        args.extend(["-o", out.to_str().unwrap()]);
        let r = mica(settings, &args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        (
            std::fs::read(&out).unwrap(),
            String::from_utf8_lossy(&r.stderr).into_owned(),
        )
    };
    let on = settings(&dir, "on", "textures = true");
    let off = settings(&dir, "off", "textures = false");
    let (plain, _) = slice(&on, false, "plain.gcode");
    let (textured, quiet) = slice(&on, true, "textured.gcode");
    assert_ne!(plain, textured, "the texture changes the walls");
    assert!(!quiet.contains("Settings › Features"));
    let (without, said) = slice(&off, true, "without.gcode");
    assert!(
        said.contains(
            "This project uses Surface textures, which is turned off in Settings › Features"
        ),
        "{said}"
    );
    assert!(
        plain == without,
        "textures off slices like no texture at all"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_refuses_to_start_when_off() {
    let dir = temp("mcp");
    let off = settings(&dir, "off", "mcp = false");
    let r = mica(&off, &["mcp"]);
    assert!(!r.status.success());
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(
        err.contains("Mica MCP is turned off in Settings › Features"),
        "{err}"
    );
    assert!(
        err.contains("mcp = true") && err.contains("mica.toml"),
        "{err}"
    );
    // Help still works, and with MCP on (stdin closed) the server starts and ends cleanly.
    assert!(mica(&off, &["mcp", "--help"]).status.success());
    let on = settings(&dir, "on", "mcp = true");
    let r = mica(&on, &["mcp"]);
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(!err.contains("turned off"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
