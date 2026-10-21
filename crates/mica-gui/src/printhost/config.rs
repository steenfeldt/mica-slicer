// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/Utils/PrusaConnect.cpp :: PrusaConnectNew::upload
// Changes: Original Mica connection config and credential references; only the browser printer-page URL pattern is adapted, with a user-entered, validated ID and no OAuth.

//! The physical printers Mica can talk to: `printers.toml` next to `mica.toml`, shared with
//! the CLI (`mica print --printer-host NAME`). Secrets are never in this file: an entry names
//! a Keychain reference (`keychain = "…"`) or, for CLI setups, an environment variable.
//! Each entry may link to a Mica printer profile (`profile = "Vendor/Preset"`); several
//! physical printers can share one profile.

use super::keychain::SecretStore;
use mica_printhost::{Credentials, PrinterConfig, PrinterKind};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const FILE_NAME: &str = "printers.toml";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    PrusaConnect,
    PrusaLink,
    Moonraker,
    SnapmakerU1,
}
impl Kind {
    pub const ALL: [Kind; 4] = [
        Kind::PrusaLink,
        Kind::PrusaConnect,
        Kind::Moonraker,
        Kind::SnapmakerU1,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Kind::PrusaLink => "PrusaLink",
            Kind::PrusaConnect => "Prusa Connect (API key)",
            Kind::Moonraker => "Klipper (Moonraker)",
            Kind::SnapmakerU1 => "Snapmaker U1",
        }
    }
    pub fn host_kind(self) -> PrinterKind {
        match self {
            Kind::PrusaLink => PrinterKind::PrusaLink,
            Kind::PrusaConnect => PrinterKind::PrusaConnect,
            Kind::Moonraker => PrinterKind::Moonraker,
            Kind::SnapmakerU1 => PrinterKind::SnapmakerU1,
        }
    }
    /// What the credential field is called for this backend.
    pub fn secret_label(self) -> &'static str {
        match self {
            Kind::PrusaLink | Kind::PrusaConnect => "API key",
            Kind::Moonraker | Kind::SnapmakerU1 => "API key (optional)",
        }
    }
    /// The vendor of Mica's profiles this backend belongs to.
    pub fn vendor(self) -> &'static str {
        match self {
            Kind::PrusaLink | Kind::PrusaConnect => "Prusa",
            Kind::Moonraker | Kind::SnapmakerU1 => "Snapmaker",
        }
    }
}

/// The generator line Mica writes into G-code for a Moonraker printer
/// (`mica_gcode_identity`): Mica's own, or one older Moonraker forks recognise as OrcaSlicer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileHeader {
    Mica,
    OrcaCompatible,
}
impl FileHeader {
    /// The `mica_gcode_identity` value.
    pub fn key(self) -> &'static str {
        match self {
            FileHeader::Mica => "mica",
            FileHeader::OrcaCompatible => "orca_compatible",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FileHeader::Mica => "Mica Slicer",
            FileHeader::OrcaCompatible => "Orca-compatible",
        }
    }
}

/// One physical printer. `username` set means PrusaLink digest login (the secret is the
/// password); otherwise the secret is the API key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Printer {
    /// Stable for the session (never saved): what the Machine view, the Print dialog and
    /// pending syncs remember, so removing or reordering printers cannot point them at
    /// another one. Assigned by `Printers` when an entry is loaded or added; 0 = unassigned.
    #[serde(skip)]
    pub id: u64,
    pub name: String,
    pub kind: Kind,
    pub address: String,
    /// Non-secret printer ID from its Connect page, entered explicitly (never inferred from the key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_printer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Keychain reference of the API key or password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain: Option<String>,
    /// CLI-style secret references; the GUI reads them and never writes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
    /// The Mica printer profile this printer prints with, as `Vendor/Preset name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The model the printer reported at the last test, for matching profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A camera URL entered by hand (`rtsp://…`, an MJPEG stream or a snapshot JPEG over
    /// HTTP), used instead of what the printer reports. Never carries credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// The camera's login: a user name here, the password in the Keychain (reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_keychain: Option<String>,
    /// Moonraker printers: the G-code file header chosen in its settings; `None` is
    /// Automatic (the profile's default, corrected by `file_header_detected`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_header: Option<FileHeader>,
    /// What Automatic learned from the printer after an upload: its firmware names Mica's
    /// own header `Unknown`, so files for it use the Orca-compatible one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_header_detected: Option<FileHeader>,
    /// Inline secrets are refused at load time; this field only exists to say so.
    #[serde(default, skip_serializing)]
    api_key: Option<String>,
    #[serde(default, skip_serializing)]
    password: Option<String>,
}

impl Printer {
    pub fn new(kind: Kind) -> Self {
        Self {
            id: 0,
            name: String::new(),
            kind,
            address: if kind == Kind::PrusaConnect {
                mica_printhost::CONNECT_URL.into()
            } else {
                String::new()
            },
            connect_printer_id: None,
            username: None,
            keychain: None,
            api_key_env: None,
            password_env: None,
            profile: None,
            model: None,
            camera: None,
            camera_username: None,
            camera_keychain: None,
            file_header: None,
            file_header_detected: None,
            api_key: None,
            password: None,
        }
    }
    /// Whether this printer reads G-code metadata through Moonraker (the File header setting).
    pub fn is_moonraker(&self) -> bool {
        matches!(self.kind, Kind::Moonraker | Kind::SnapmakerU1)
    }

    /// The `mica_gcode_identity` files for this printer are written with, when the printer
    /// decides it (`None`: the profile's default stands).
    pub fn gcode_identity(&self) -> Option<&'static str> {
        if !self.is_moonraker() {
            return None;
        }
        self.file_header
            .or(self.file_header_detected)
            .map(FileHeader::key)
    }

    pub fn connect_page(&self) -> Option<String> {
        let id = self.connect_printer_id.as_deref()?.trim();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return None;
        }
        Some(format!(
            "https://connect.prusa3d.com/printer/{id}/dashboard"
        ))
    }
    /// Whether some credential reference is set (not whether the secret exists).
    pub fn has_secret(&self) -> bool {
        self.keychain.is_some() || self.api_key_env.is_some() || self.password_env.is_some()
    }
    /// The backend credentials, with the secret read from the store or the environment.
    pub fn credentials(&self, secrets: &dyn SecretStore) -> Result<Credentials, String> {
        let secret = if let Some(reference) = &self.keychain {
            match secrets.get(reference)? {
                Some(s) => Some(s),
                None => {
                    return Err(format!(
                        "The {} for {} is missing from the {}; enter it again in My printers",
                        if self.username.is_some() {
                            "password"
                        } else {
                            "API key"
                        },
                        self.name,
                        secrets.label()
                    ));
                }
            }
        } else if let Some(var) = self.api_key_env.as_ref().or(self.password_env.as_ref()) {
            Some(
                std::env::var(var)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        format!(
                            "The environment variable {var} (credential of {}) is unset",
                            self.name
                        )
                    })?,
            )
        } else {
            None
        };
        Ok(match (&self.username, secret) {
            (Some(username), Some(password)) if self.kind == Kind::PrusaLink => {
                Credentials::Digest {
                    username: username.clone(),
                    password,
                }
            }
            (Some(_), Some(_)) => {
                return Err("Only PrusaLink printers use a user name and password".into());
            }
            (Some(_), None) => {
                return Err(format!("{} has a user name but no password", self.name));
            }
            (None, Some(key)) => Credentials::ApiKey(key),
            (None, None) => Credentials::None,
        })
    }
    pub fn config(&self, secrets: &dyn SecretStore) -> Result<PrinterConfig, String> {
        Ok(PrinterConfig {
            name: self.name.clone(),
            kind: self.kind.host_kind(),
            address: self.address.trim().to_string(),
            credentials: self.credentials(secrets)?,
        })
    }
    /// The camera login, the password read from the store (on the calling thread).
    pub fn camera_auth(
        &self,
        secrets: &dyn SecretStore,
    ) -> Result<Option<mica_camera::Auth>, String> {
        let Some(username) = self
            .camera_username
            .clone()
            .filter(|u| !u.trim().is_empty())
        else {
            return Ok(None);
        };
        let password = match &self.camera_keychain {
            Some(r) => secrets.get(r)?.ok_or_else(|| {
                format!(
                    "The camera password of {} is missing from the {}; enter it again in My printers",
                    self.name,
                    secrets.label()
                )
            })?,
            None => String::new(),
        };
        Ok(Some(mica_camera::Auth { username, password }))
    }
    /// Problems that stop an entry from being saved.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Give the printer a name".into());
        }
        if self.address.trim().is_empty() {
            return Err("Enter the printer's address (name or IP, optionally with a port)".into());
        }
        let a = self.address.trim();
        if a.contains('@') || a.contains('?') || a.contains('#') || a.contains(char::is_whitespace)
        {
            return Err("The address is a host name or IP, optionally with http:// and a port; no spaces, credentials or query".into());
        }
        if self
            .username
            .as_deref()
            .is_some_and(|u| !u.trim().is_empty())
            && self.kind != Kind::PrusaLink
        {
            return Err("Only PrusaLink printers log in with a user name and password".into());
        }
        if self.connect_printer_id.is_some() && self.connect_page().is_none() {
            return Err(
                "Enter the printer ID from the Connect page URL: /printer/ID/dashboard".into(),
            );
        }
        if let Some(c) = self.camera.as_deref().filter(|c| !c.trim().is_empty()) {
            mica_camera::Source::from_url(c).map_err(|e| format!("Camera URL: {e}"))?;
        }
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    printers: Vec<Printer>,
}

/// Parses `printers.toml`. Inline secrets and duplicate names are refused; the parser's own
/// message is not echoed because it can quote the offending (secret-bearing) line.
pub fn parse(text: &str) -> Result<Vec<Printer>, String> {
    let file: File = toml::from_str(text).map_err(|_| {
        format!("{FILE_NAME} is not valid (expected [[printers]] entries with name, kind, address and a keychain or env reference)")
    })?;
    let mut names = BTreeSet::new();
    for p in &file.printers {
        if p.api_key.is_some() || p.password.is_some() {
            return Err(format!(
                "{FILE_NAME}: {} has an inline secret; Mica keeps secrets in the Keychain only. Remove the api_key / password line.",
                p.name
            ));
        }
        if !names.insert(p.name.as_str()) {
            return Err(format!("{FILE_NAME}: two printers are named {}", p.name));
        }
    }
    Ok(file.printers)
}

pub fn to_toml(printers: &[Printer]) -> String {
    let file = File {
        printers: printers.to_vec(),
    };
    format!(
        "# Mica Slicer printers. Secrets are NOT in this file: `keychain` names a macOS Keychain\n\
         # item (service \"Mica Slicer\"), `api_key_env` / `password_env` an environment variable.\n\
         {}",
        toml::to_string(&file).expect("printers always serialize")
    )
}

/// `~/Library/Application Support/Mica Slicer/printers.toml` (or `MICA_CONFIG_DIR`).
pub fn default_path() -> PathBuf {
    crate::state::Prefs::default_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join(FILE_NAME)
}

/// Reads the file; a missing file is an empty list, a broken one an empty list plus the
/// reason, so the app still starts.
pub fn load(path: &Path) -> (Vec<Printer>, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(p) => (p, None),
            Err(e) => (vec![], Some(e)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (vec![], None),
        Err(e) => (vec![], Some(format!("{}: {e}", path.display()))),
    }
}

/// Writes atomically (temp file + rename).
pub fn save(path: &Path, printers: &[Printer]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, to_toml(printers)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// How well a physical printer fits a Mica profile, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    /// Explicitly linked to this profile.
    Linked,
    /// Same backend family and the reported model names the profile's model.
    Model,
    /// Same backend family (PrusaLink for Prusa profiles, Moonraker / U1 for Snapmaker).
    Family,
    /// Another kind of printer altogether.
    Other,
}

/// Lower-case alphanumeric tokens of a model name, without the vendor words.
fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| !t.is_empty() && !matches!(t.as_str(), "original" | "prusa" | "snapmaker"))
        .collect()
}

/// Ranks `printer` for the profile `vendor/preset` whose model name is `model` (the part of
/// the preset name before the nozzle variant).
pub fn fit(printer: &Printer, vendor: &str, preset: &str, model: &str) -> Fit {
    if printer.profile.as_deref() == Some(&format!("{vendor}/{preset}")) {
        return Fit::Linked;
    }
    if !printer.kind.vendor().eq_ignore_ascii_case(vendor) {
        return Fit::Other;
    }
    let wanted = tokens(model);
    if printer.kind == Kind::SnapmakerU1 && wanted.contains("u1") {
        return Fit::Model;
    }
    if let Some(reported) = &printer.model {
        // "Original Prusa XL 5T" fits "Original Prusa XL 5T Input Shaper" (and the other
        // way round); "MK4" does not fit "MK4S".
        let got = tokens(reported);
        if !wanted.is_empty()
            && !got.is_empty()
            && (wanted.is_subset(&got) || got.is_subset(&wanted))
        {
            return Fit::Model;
        }
    }
    Fit::Family
}

/// Printers ordered for a profile: linked, then by model, then same family, then the rest;
/// stable within a rank. Returns `(index, fit)`.
pub fn ranked(printers: &[Printer], vendor: &str, preset: &str, model: &str) -> Vec<(usize, Fit)> {
    let mut out: Vec<(usize, Fit)> = printers
        .iter()
        .enumerate()
        .map(|(i, p)| (i, fit(p, vendor, preset, model)))
        .collect();
    out.sort_by_key(|(i, f)| (*f, *i));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printhost::keychain::MemoryStore;

    fn xl() -> Printer {
        Printer {
            name: "XL".into(),
            address: "xl.local".into(),
            keychain: Some("mica-printer-1".into()),
            model: Some("Original Prusa XL 5T".into()),
            ..Printer::new(Kind::PrusaLink)
        }
    }

    #[test]
    fn file_round_trips_without_secrets() {
        let printers = vec![
            xl(),
            Printer {
                name: "U1".into(),
                address: "http://u1.local:7125".into(),
                profile: Some("Snapmaker/Snapmaker U1 (0.4 nozzle)".into()),
                ..Printer::new(Kind::SnapmakerU1)
            },
            Printer {
                name: "cli".into(),
                address: "coreone.local".into(),
                username: Some("maker".into()),
                password_env: Some("MICA_XL_PASSWORD".into()),
                ..Printer::new(Kind::PrusaLink)
            },
        ];
        let text = to_toml(&printers);
        assert!(text.contains("kind = \"SnapmakerU1\""));
        assert!(text.contains("keychain = \"mica-printer-1\""));
        assert!(!text.contains("api_key ="));
        assert_eq!(parse(&text).unwrap(), printers);
        // The CLI's own example file parses too.
        let cli = "[[printers]]\nname = \"coreone\"\nkind = \"PrusaLink\"\naddress = \"coreone.local\"\napi_key_env = \"MICA_COREONE_KEY\"\n";
        assert_eq!(
            parse(cli).unwrap()[0].api_key_env.as_deref(),
            Some("MICA_COREONE_KEY")
        );
        // Inline secrets, duplicates and unknown keys are refused without echoing the line.
        let bad = parse(&format!("{cli}api_key = \"topsecret\"\n")).unwrap_err();
        assert!(bad.contains("inline secret") && !bad.contains("topsecret"));
        assert!(
            parse(&format!("{cli}{cli}"))
                .unwrap_err()
                .contains("two printers")
        );
        assert!(parse(&format!("{cli}colour = 1\n")).is_err());
        let dir = std::env::temp_dir().join(format!("mica-printers-{}", std::process::id()));
        let path = dir.join("sub").join(FILE_NAME);
        save(&path, &printers).unwrap();
        assert_eq!(load(&path), (printers, None));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn credentials_come_from_the_store_never_the_file() {
        let store = MemoryStore::default();
        let p = xl();
        let missing = p.credentials(&store).unwrap_err();
        assert!(missing.contains("missing") && missing.contains("API key"));
        store.set("mica-printer-1", "KEY-1234").unwrap();
        let c = p.config(&store).unwrap();
        assert!(matches!(&c.credentials, Credentials::ApiKey(k) if k == "KEY-1234"));
        assert!(!format!("{c:?}").contains("KEY-1234"));
        let digest = Printer {
            username: Some("maker".into()),
            ..xl()
        };
        assert!(matches!(
            digest.credentials(&store).unwrap(),
            Credentials::Digest { .. }
        ));
        let none = Printer::new(Kind::Moonraker);
        assert!(matches!(
            none.credentials(&store).unwrap(),
            Credentials::None
        ));
        let wrong = Printer {
            username: Some("maker".into()),
            keychain: Some("mica-printer-1".into()),
            ..Printer::new(Kind::Moonraker)
        };
        assert!(wrong.credentials(&store).is_err());
        assert!(wrong.validate().is_err());
        assert!(Printer::new(Kind::PrusaLink).validate().is_err());
        assert!(
            Printer {
                name: "x".into(),
                address: "user:pw@host".into(),
                ..Printer::new(Kind::PrusaLink)
            }
            .validate()
            .is_err()
        );
        assert!(xl().validate().is_ok());
    }

    #[test]
    fn profiles_pick_the_first_matching_physical_printer() {
        let printers = vec![
            Printer {
                name: "U1".into(),
                ..Printer::new(Kind::SnapmakerU1)
            },
            Printer {
                name: "CORE One".into(),
                model: Some("Prusa CORE One".into()),
                ..Printer::new(Kind::PrusaLink)
            },
            xl(),
            Printer {
                name: "XL linked".into(),
                profile: Some("Prusa/Original Prusa XL 5T Input Shaper 0.4 nozzle".into()),
                ..Printer::new(Kind::PrusaLink)
            },
        ];
        let r = ranked(
            &printers,
            "Prusa",
            "Original Prusa XL 5T Input Shaper 0.4 nozzle",
            "Original Prusa XL 5T Input Shaper",
        );
        assert_eq!(
            r,
            vec![
                (3, Fit::Linked),
                (2, Fit::Model),
                (1, Fit::Family),
                (0, Fit::Other)
            ]
        );
        let r = ranked(
            &printers,
            "Prusa",
            "Prusa CORE One HF0.4 nozzle",
            "Prusa CORE One",
        );
        assert_eq!(r[0], (1, Fit::Model));
        let r = ranked(
            &printers,
            "Snapmaker",
            "Snapmaker U1 (0.4 nozzle)",
            "Snapmaker U1",
        );
        assert_eq!(r[0], (0, Fit::Model));
        assert!(r[1..].iter().all(|(_, f)| *f == Fit::Other));
    }
}
