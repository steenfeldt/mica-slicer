// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use mica_printhost::{
    Credentials, FileFormat, JobRequirements, PrinterConfig, PrinterHost, PrinterKind, Timeouts,
    ToolRequirement, UploadAction,
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    printers: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    kind: String,
    #[serde(default)]
    address: String,
    #[allow(dead_code)]
    connect_printer_id: Option<String>,
    api_key_env: Option<String>,
    username: Option<String>,
    password_env: Option<String>,
    keychain: Option<String>,
    /// Written by the GUI (the linked Mica profile and the model the printer reported); the
    /// CLI reads past them so one file serves both.
    #[allow(dead_code)]
    profile: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
    /// The GUI's camera and G-code file-header settings, read past as well.
    #[allow(dead_code)]
    camera: Option<String>,
    #[allow(dead_code)]
    camera_username: Option<String>,
    #[allow(dead_code)]
    camera_keychain: Option<String>,
    #[allow(dead_code)]
    file_header: Option<String>,
    #[allow(dead_code)]
    file_header_detected: Option<String>,
}
impl Entry {
    fn resolve(
        self,
        env: impl Fn(&str) -> Result<String, String>,
    ) -> Result<PrinterConfig, String> {
        let kind = match self.kind.as_str() {
            "PrusaLink" => PrinterKind::PrusaLink,
            "PrusaConnect" => PrinterKind::PrusaConnect,
            "Moonraker" => PrinterKind::Moonraker,
            "SnapmakerU1" => PrinterKind::SnapmakerU1,
            _ => {
                return Err(
                    "printer kind must be PrusaLink, PrusaConnect, Moonraker or SnapmakerU1".into(),
                );
            }
        };
        if self.keychain.is_some() {
            return Err(
                "this printer's secret is in the macOS Keychain (set up in Mica's My printers); \
                 the CLI cannot read the Keychain yet, so add api_key_env or password_env for \
                 command-line use"
                    .into(),
            );
        }
        let credentials = match (self.api_key_env, self.username, self.password_env) {
            (None, None, None) => Credentials::None,
            (Some(key), None, None) => Credentials::ApiKey(env(&key)?),
            (None, Some(username), Some(key)) if kind == PrinterKind::PrusaLink => {
                Credentials::Digest {
                    username,
                    password: env(&key)?,
                }
            }
            _ => return Err("use only api_key_env, or PrusaLink username + password_env".into()),
        };
        Ok(PrinterConfig {
            name: self.name,
            kind,
            address: self.address,
            credentials,
        })
    }
}
fn parse_config(text: &str, name: &str) -> Result<Entry, String> {
    // Parser diagnostics can quote TOML source; never echo a malformed secret-bearing line.
    let config: Config = toml::from_str(text).map_err(|_| {
        "invalid printers.toml (expected [[printers]] entries; inline secrets are forbidden)"
            .to_string()
    })?;
    let mut names = BTreeSet::new();
    for entry in &config.printers {
        if !names.insert(&entry.name) {
            return Err("duplicate printer name in printers.toml".into());
        }
    }
    config
        .printers
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("no printer named {name:?} in printers.toml"))
}
fn config_path() -> PathBuf {
    mica_profiles::user::Store::default_root()
        .parent()
        .unwrap_or(Path::new("."))
        .join("printers.toml")
}
fn host(name: &str) -> Result<Box<dyn PrinterHost>, String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config = parse_config(&text, name)?.resolve(|key| {
        std::env::var(key)
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("credential environment variable {key:?} is unset or empty"))
    })?;
    mica_printhost::connect(config, Timeouts::default()).map_err(|e| e.to_string())
}
pub(crate) fn printers(args: &[String]) -> Result<(), String> {
    let [command, name] = args else {
        return Err("usage: mica printers status <name>".into());
    };
    if command != "status" {
        return Err("usage: mica printers status <name>".into());
    }
    let h = host(name)?;
    if !h.capabilities().status {
        return Err(
            "This connection only sends files; open Prusa Connect for printer status".into(),
        );
    }
    let info = h.info().map_err(|e| e.to_string())?;
    let s = h.status().map_err(|e| e.to_string())?;
    println!(
        "{name}: {:?}; model {}; firmware {}",
        s.state,
        info.model.as_deref().unwrap_or("unknown"),
        info.firmware.as_deref().unwrap_or("unknown")
    );
    if let Some(name) = s.job_name {
        println!("Job: {name}");
    }
    if let Some(p) = s.progress {
        println!("Progress: {p:.1}%");
    }
    if let Some(t) = s.elapsed {
        println!("Elapsed: {t:.0} s");
    }
    if let Some(t) = s.remaining {
        println!("Remaining: ~{t:.0} s");
    }
    if let Some(l) = s.current_layer {
        println!("Layer: {l}");
    }
    if let Some(z) = s.z {
        println!("Z: {z:.2} mm");
    }
    for t in s.hotends {
        println!(
            "T{}: {:?} / {:?} °C",
            t.tool, t.temperature.current, t.temperature.target
        );
    }
    if let Some(t) = s.bed {
        println!("Bed: {:?} / {:?} °C", t.current, t.target);
    }
    if let Some(t) = s.chamber {
        println!("Chamber: {:?} / {:?} °C", t.current, t.target);
    }
    Ok(())
}
pub(crate) fn print(args: &[String]) -> Result<(), String> {
    let usage = "usage: mica print <file.gcode|file.bgcode> --printer-host <name> [--upload-only | --queue]";
    let Some(file) = args.first().filter(|a| !a.starts_with('-')) else {
        return Err(usage.into());
    };
    let mut name = None;
    let mut upload_only = false;
    let mut queue = false;
    let mut it = args[1..].iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--printer-host" if name.is_none() => name = Some(it.next().ok_or(usage)?),
            "--upload-only" if !upload_only && !queue => upload_only = true,
            "--queue" if !queue && !upload_only => queue = true,
            _ => return Err(usage.into()),
        }
    }
    let name = name.ok_or(usage)?;
    let path = Path::new(file);
    let requirements = requirements(path)?;
    let remote = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("file name must be UTF-8")?;
    let h = host(name)?;
    for issue in h.preflight(&requirements) {
        eprintln!("warning: {}", issue.message);
    }
    let mut last = 0;
    let action = if queue {
        UploadAction::Queue
    } else if upload_only {
        UploadAction::UploadOnly
    } else {
        UploadAction::Print
    };
    let outcome = h
        .upload_action_with_progress(path, remote, action, &mut |p| {
            let percent = p
                .sent
                .saturating_mul(100)
                .checked_div(p.total)
                .unwrap_or(100);
            if percent >= last + 10 || percent == 100 {
                eprintln!("Uploading: {percent}%");
                last = percent;
            }
        })
        .map_err(|e| e.to_string())?;
    if let Some(message) = &outcome.message {
        eprintln!(
            "{}: {message}",
            if outcome.warning {
                "warning"
            } else {
                "Connect"
            }
        );
    }
    let job = outcome.job;
    println!(
        "{}: uploaded {}{}",
        name,
        job.name,
        if job.started {
            "; print started"
        } else if queue {
            "; queue requested; check Prusa Connect"
        } else if upload_only {
            ""
        } else {
            "; printer did not confirm a start"
        }
    );
    Ok(())
}
/// Read tool selections and optional slicer metadata in bounded blocks, including binary G-code.
fn requirements(path: &Path) -> Result<JobRequirements, String> {
    let format = FileFormat::from_path(path).ok_or("expected a .gcode or .bgcode file")?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("expected a regular G-code file".into());
    }
    let mut scan = Scan::default();
    match format {
        FileFormat::Gcode => {
            for line in BufReader::new(file).lines() {
                scan.line(&line.map_err(|e| e.to_string())?);
            }
        }
        FileFormat::Bgcode => {
            use mica_gcode::bgcode::{
                Metadata,
                stream::{self, Block},
            };
            stream::visit(file, 16 * 1024 * 1024, |block| {
                match block {
                    Block::Gcode(text) => {
                        for line in text.lines() {
                            scan.line(line);
                        }
                    }
                    Block::Metadata(_, Metadata::Ini(entries)) => {
                        for (key, value) in entries {
                            scan.metadata(&key, &value);
                        }
                    }
                    _ => {}
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        }
    }
    if scan.tools.is_empty() {
        scan.tools.insert(0);
    }
    Ok(JobRequirements {
        tools: scan
            .tools
            .iter()
            .map(|&tool| ToolRequirement {
                tool,
                material: scan.materials.get(tool as usize).cloned(),
                colour: scan.colours.get(tool as usize).cloned(),
            })
            .collect(),
        format,
        bed_type: scan.bed,
        needs_chamber_sensor: false,
    })
}
#[derive(Default)]
struct Scan {
    tools: BTreeSet<u32>,
    materials: Vec<String>,
    colours: Vec<String>,
    bed: Option<String>,
}
impl Scan {
    fn line(&mut self, line: &str) {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix(';') {
            if let Some((key, value)) = comment.split_once('=') {
                self.metadata(key.trim(), value.trim());
            }
            return;
        }
        let command = line.split([';', ' ', '\t']).next().unwrap_or("");
        if let Some(tool) = command
            .strip_prefix('T')
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|t| *t < 256)
        {
            self.tools.insert(tool);
        }
    }
    fn metadata(&mut self, key: &str, value: &str) {
        let split = || value.split(';').map(|v| v.trim().to_owned()).collect();
        match key {
            "filament_type" => self.materials = split(),
            "filament_colour" | "extruder_colour" => self.colours = split(),
            "bed_type" | "curr_bed_type" => self.bed = Some(value.into()),
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_and_secret_references() {
        let config = "[[printers]]\nname='CORE One'\nkind='PrusaLink'\naddress='printer.local'\napi_key_env='MICA_KEY'";
        let c = parse_config(config, "CORE One")
            .unwrap()
            .resolve(|key| {
                assert_eq!(key, "MICA_KEY");
                Ok("secret".into())
            })
            .unwrap();
        assert!(!format!("{c:?}").contains("secret"));
        assert!(parse_config(&format!("{config}\napi_key='secret'"), "CORE One").is_err());
        assert!(parse_config(&format!("{config}\n{config}"), "CORE One").is_err());
        // The GUI's extra fields are accepted.
        assert!(
            parse_config(
                &format!(
                    "{config}\nprofile='Prusa/Prusa CORE One HF0.4 nozzle'\nmodel='Prusa CORE One'"
                ),
                "CORE One"
            )
            .is_ok()
        );
        assert!(
            parse_config(
                &format!(
                    "{config}\ncamera='rtsp://cam.local/stream'\ncamera_username='me'\ncamera_keychain='cam-id'\nfile_header='orca_compatible'\nfile_header_detected='orca_compatible'"
                ),
                "CORE One"
            )
            .is_ok()
        );
        assert!(
            parse_config(
                &config.replace("api_key_env='MICA_KEY'", "keychain='id'"),
                "CORE One"
            )
            .unwrap()
            .resolve(|_| panic!("must not resolve env"))
            .is_err()
        );
        assert!(
            parse_config(config, "CORE One")
                .unwrap()
                .resolve(|_| Err("missing".into()))
                .is_err()
        );
    }
    #[test]
    fn job_tools_and_metadata() {
        let mut s = Scan::default();
        for line in [
            "; filament_type = PLA;PETG",
            "; filament_colour = #FF0000;#00FF00",
            "; bed_type = Satin",
            "T0 S1",
            "T3 ; select",
            "; T5",
            "T-1",
            "T1000",
            "M104 T4 S200",
        ] {
            s.line(line);
        }
        assert_eq!(s.tools.into_iter().collect::<Vec<_>>(), vec![0, 3]);
        assert_eq!(s.materials, vec!["PLA", "PETG"]);
        assert_eq!(s.bed.as_deref(), Some("Satin"));
    }
    #[test]
    fn requirements_read_real_text_and_binary_jobs() {
        use mica_gcode::bgcode::{self, BgcodeMetadata, BinarizerConfig, Metadata};
        let root = std::env::temp_dir().join(format!("mica-print-cli-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let text =
            "; filament_type = PLA;PETG\n; filament_colour = #FF0000;#00FF00\nT0\nG1 X1\nT1\n";
        let plain = root.join("test.gcode");
        std::fs::write(&plain, text).unwrap();
        let job = requirements(&plain).unwrap();
        assert_eq!(job.format, FileFormat::Gcode);
        assert_eq!(job.tools.len(), 2);
        assert_eq!(job.tools[1].material.as_deref(), Some("PETG"));
        let binary = root.join("test.bgcode");
        let metadata = BgcodeMetadata {
            slicer: vec![Metadata::Ini(vec![(
                "filament_type".into(),
                "PLA;PETG".into(),
            )])],
            ..BgcodeMetadata::default()
        };
        std::fs::write(
            &binary,
            bgcode::encode("T0\nG1 X1\nT1\n", &metadata, &BinarizerConfig::default()).unwrap(),
        )
        .unwrap();
        let job = requirements(&binary).unwrap();
        assert_eq!(job.format, FileFormat::Bgcode);
        assert_eq!(job.tools.len(), 2);
        assert_eq!(job.tools[1].material.as_deref(), Some("PETG"));
        std::fs::write(&binary, b"GCDEbroken").unwrap();
        assert!(requirements(&binary).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn connect_config_uses_only_references_and_supports_queue_flag() {
        let text = "[[printers]]\nname='Cloud XL'\nkind='PrusaConnect'\naddress='https://connect.prusa3d.com'\napi_key_env='FAKE_CONNECT_KEY'\nconnect_printer_id='1234-abcd'";
        let config = parse_config(text, "Cloud XL")
            .unwrap()
            .resolve(|name| {
                assert_eq!(name, "FAKE_CONNECT_KEY");
                Ok("fake-connect-secret".into())
            })
            .unwrap();
        assert_eq!(config.kind, PrinterKind::PrusaConnect);
        assert!(!format!("{config:?}").contains("fake-connect-secret"));
        assert!(mica_printhost::connect(config, Timeouts::default()).is_ok()); // constructs only
        assert!(parse_config(&format!("{text}\napi_key='inline-secret'"), "Cloud XL").is_err());
        let args: Vec<String> = [
            "dummy.gcode",
            "--printer-host",
            "Cloud XL",
            "--queue",
            "--upload-only",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert!(print(&args).unwrap_err().starts_with("usage:"));
    }
}
