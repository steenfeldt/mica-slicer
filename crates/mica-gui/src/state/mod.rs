// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Persistent GUI state: the Mica config file (TOML) with the remembered printer, presets,
//! window size, recent files and preferences. No secrets are ever written here.

pub mod printers;
pub mod undo;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "mica.toml";
/// How many recent files File › Open recent keeps unless the user chose otherwise.
pub const DEFAULT_RECENT: usize = 10;
/// The longest recent-files list Preferences offers.
pub const MAX_RECENT: usize = 30;
/// Delay between the last change and an automatic re-slice, unless the user chose otherwise.
pub const DEFAULT_AUTO_SLICE_DELAY_MS: u64 = 500;
/// The range Preferences offers for that delay.
pub const AUTO_SLICE_DELAY_RANGE_MS: std::ops::RangeInclusive<u64> = 200..=5000;
/// Format 1 defaulted auto-slice to on; format 2 makes it an opt-in and resets it once.
pub const FORMAT: i64 = 2;

/// Everything Mica remembers between sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub format: i64,
    pub window: Window,
    pub selection: Selection,
    /// Printer presets the user has chosen to see first, as `vendor/preset name`.
    pub my_printers: Vec<String>,
    pub recent_files: Vec<PathBuf>,
    pub auto_slice: bool,
    /// How long after the last change an automatic slice starts (milliseconds).
    pub auto_slice_delay_ms: u64,
    pub advanced: bool,
    /// `dark`, `light` or `follow-system` (see `theme::Choice`); anything else (including
    /// the former `system`) means dark.
    pub theme: String,
    /// The settings topic picked in the process card's rail (`Essentials`, `Quality`, …).
    pub settings_topic: String,
    /// The Settings window's open tab (`general`, `slicing`, `profiles`, `files`, `features`).
    pub preferences_tab: String,
    /// Folded sections inside the settings topics, as `Topic/Section`.
    pub collapsed_sections: Vec<String>,
    /// Topics showing their advanced settings ("Show more").
    pub expanded_sections: Vec<String>,
    /// Set once the PrusaSlicer / Orca installation has been scanned on first run.
    pub detected: bool,
    /// The settings sidebar sits at the right edge of the window (the default) or the left.
    pub sidebar_right: bool,
    /// How big the 3D view's floating toolbar and plate bar are (Settings › General).
    pub toolbar_size: ToolbarSize,
    /// How many files File › Open recent remembers (0 = none).
    pub recent_limit: usize,
    /// What New project (and a launch) start with; empty fields mean "automatic".
    pub defaults: Defaults,
    /// The 3D view: perspective (on) or orthographic camera, as the P key toggles.
    pub perspective: bool,
    /// Scrolling zooms towards the mouse pointer (on) or the centre of the view.
    pub zoom_to_pointer: bool,
    /// Scrolling up zooms out instead of in.
    pub reverse_zoom: bool,
    /// Colours picked last for filaments (`#RRGGBB`, newest first), for the colour popup.
    pub recent_colours: Vec<String>,
    /// Ask the update server for a newer Mica at start, at most once a day.
    pub check_updates: bool,
    /// When that last happened (seconds since 1970; 0: never).
    pub last_update_check: u64,
    /// The Snapmaker U1 start options last sent, by printer name (My printers).
    pub u1_start: std::collections::BTreeMap<String, crate::printhost::u1map::Remembered>,
    /// Mica's own features, each switched on or off in Settings › Features.
    pub features: Features,
    /// Starred surface textures (`builtin:…`, `procedural:…`), first shown in the gallery.
    pub texture_favorites: Vec<String>,
    /// Importing presets from other slicers: what not to ask about again.
    pub import: ImportPrefs,
    /// The system filaments the filament picker lists, per printer (`vendor/preset name`).
    /// A printer without an entry lists every filament made for it. Only what the picker shows:
    /// the selection and the G-code never depend on it.
    pub visible_filaments: std::collections::BTreeMap<String, Vec<String>>,
    /// The Welcome window was closed once (it opens by itself only on the first launch).
    pub welcome_seen: bool,
    /// The quick tour ran once (Welcome › Take a quick tour, Help › Show quick tour); it never
    /// starts by itself.
    pub tour_seen: bool,
    /// The Mica version that ran last: a different one shows its What's new once.
    pub last_version: String,
}

/// The size of the 3D view's floating toolbars (`toolbar_size` in mica.toml: `compact`,
/// `normal` or `large`; anything else reads as `normal`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ToolbarSize {
    Compact,
    #[default]
    Normal,
    Large,
}

impl ToolbarSize {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Normal, Self::Large];

    pub fn key(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Normal => "normal",
            Self::Large => "large",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Normal => "Normal",
            Self::Large => "Large",
        }
    }
}

impl From<String> for ToolbarSize {
    fn from(s: String) -> Self {
        Self::ALL
            .into_iter()
            .find(|t| t.key().eq_ignore_ascii_case(s.trim()))
            .unwrap_or_default()
    }
}

impl From<ToolbarSize> for String {
    fn from(t: ToolbarSize) -> Self {
        t.key().into()
    }
}

/// `[import]` in mica.toml: Mica asks before importing other slicers' presets, once per new
/// preset, and never for a slicer the user said not to ask about.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportPrefs {
    /// Slicers not to ask about (`mica_profiles::import::Source::key`).
    pub dont_ask: Vec<String>,
    /// Presets Mica has already asked about (`source|kind|file`); a notice only names new ones.
    pub seen: Vec<String>,
}

impl ImportPrefs {
    pub fn asks(&self, source: mica_profiles::import::Source) -> bool {
        !self.dont_ask.iter().any(|k| k == source.key())
    }

    pub fn set_asks(&mut self, source: mica_profiles::import::Source, ask: bool) {
        self.dont_ask.retain(|k| k != source.key());
        if !ask {
            self.dont_ask.push(source.key().into());
        }
    }

    /// The identity of a found preset, as `seen` keeps it.
    pub fn id(found: &mica_profiles::import::Found) -> String {
        format!(
            "{}|{}|{}",
            found.source.key(),
            mica_profiles::user::kind_name(found.kind),
            found.path.display()
        )
    }

    pub fn was_seen(&self, found: &mica_profiles::import::Found) -> bool {
        let id = Self::id(found);
        self.seen.contains(&id)
    }

    pub fn mark_seen<'a>(
        &mut self,
        found: impl IntoIterator<Item = &'a mica_profiles::import::Found>,
    ) {
        for f in found {
            if !self.was_seen(f) {
                self.seen.push(Self::id(f));
            }
        }
    }
}

/// Mica's own features with a switch in Settings › Features (`[features]` in mica.toml),
/// shared with the CLI so both slice alike.
pub use mica_profiles::features::{Feature, Features};

/// The profiles new projects start with. Each field is empty for "automatic": the printer
/// last used, and the print / filament profile Mica picks for the printer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    /// `vendor/preset name` of the printer a launch starts with.
    pub printer: String,
    /// Print profile, used when the printer offers it.
    pub print: String,
    /// Filament profile for every tool, used when the printer offers it.
    pub filament: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub width: f32,
    pub height: f32,
    pub sidebar_width: f32,
    /// Height of the object tree at the top of the sidebar (its list part).
    pub tree_height: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub vendor: String,
    pub printer: String,
    pub print: String,
    pub filaments: Vec<String>,
    pub bed: String,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            width: 1440.,
            height: 900.,
            sidebar_width: 370.,
            tree_height: 300.,
        }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            format: FORMAT,
            window: Window::default(),
            selection: Selection::default(),
            my_printers: vec![],
            recent_files: vec![],
            auto_slice: false,
            auto_slice_delay_ms: DEFAULT_AUTO_SLICE_DELAY_MS,
            advanced: false,
            theme: "dark".into(),
            settings_topic: "Essentials".into(),
            preferences_tab: "general".into(),
            collapsed_sections: vec![],
            expanded_sections: vec![],
            detected: false,
            sidebar_right: true,
            recent_limit: DEFAULT_RECENT,
            defaults: Defaults::default(),
            perspective: true,
            zoom_to_pointer: true,
            reverse_zoom: false,
            recent_colours: vec![],
            check_updates: true,
            last_update_check: 0,
            u1_start: Default::default(),
            features: Features::default(),
            texture_favorites: vec![],
            import: ImportPrefs::default(),
            toolbar_size: ToolbarSize::default(),
            visible_filaments: Default::default(),
            welcome_seen: false,
            tour_seen: false,
            last_version: String::new(),
        }
    }
}

/// The settings as this process last read or wrote them (see [`Prefs::save_merged`]). The
/// default (nothing known) makes the next save write every value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved(Option<toml::Table>);

/// Replaces, in `base`, every value `raw` also has with the same type (recursing into
/// tables): `base` keeps only the keys Mica knows, and a number written without a decimal
/// point (`width = 800`) still equals what Mica writes (`800.0`).
fn overlay_known(base: &mut toml::Table, raw: &toml::Table) {
    for (k, v) in base.iter_mut() {
        match (v, raw.get(k)) {
            (toml::Value::Table(b), Some(toml::Value::Table(r))) => overlay_known(b, r),
            (v, Some(r)) if v.type_str() == r.type_str() => *v = r.clone(),
            _ => {}
        }
    }
}

/// Writes into `disk` the values of `mine` that differ from `base` and removes the keys
/// `mine` dropped, table by table. Values this process did not change keep whatever `disk`
/// has (another process may have changed them). Without a base, every value is written.
fn apply_changes(disk: &mut toml::Table, base: Option<&toml::Table>, mine: &toml::Table) {
    for (k, m) in mine {
        let b = base.and_then(|b| b.get(k));
        match (b, m) {
            (b, toml::Value::Table(m)) => {
                let slot = disk
                    .entry(k.clone())
                    .or_insert_with(|| toml::Value::Table(Default::default()));
                if !slot.is_table() {
                    *slot = toml::Value::Table(Default::default());
                }
                if let toml::Value::Table(d) = slot {
                    apply_changes(d, b.and_then(toml::Value::as_table), m);
                }
            }
            (Some(b), m) if b == m => {}
            _ => {
                disk.insert(k.clone(), m.clone());
            }
        }
    }
    if let Some(base) = base {
        for k in base.keys().filter(|k| !mine.contains_key(*k)) {
            disk.remove(k);
        }
    }
}

impl Prefs {
    /// `~/Library/Application Support/Mica Slicer/mica.toml` (or `MICA_CONFIG_DIR`).
    pub fn default_path() -> PathBuf {
        mica_profiles::user::Store::default_root()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join(FILE_NAME)
    }

    /// Reads the file; a missing or broken file yields defaults (the error is returned alongside
    /// so the UI can mention it without refusing to start).
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let (p, warning, _) = Self::load_tracked(path);
        (p, warning)
    }

    /// [`Self::load`], plus what was read, for [`Self::save_merged`]: the values this process
    /// starts from, so a later save writes only what it changed.
    pub fn load_tracked(path: &Path) -> (Self, Option<String>, Saved) {
        let (p, raw, warning) = match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(p) => (p, toml::from_str::<toml::Table>(&text).ok(), None),
                Err(e) => (
                    Self::default(),
                    None,
                    Some(format!("{}: {e}; using defaults", path.display())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None, None),
            Err(e) => (
                Self::default(),
                None,
                Some(format!("{}: {e}; using defaults", path.display())),
            ),
        };
        // The base is what the file said for every key Mica knows (the defaults for keys it
        // did not have): a value Mica normalized while loading then counts as a change and
        // is written back.
        let mut base = p.to_table();
        if let Some(raw) = &raw {
            overlay_known(&mut base, raw);
        }
        (p, warning, Saved(Some(base)))
    }

    fn to_table(&self) -> toml::Table {
        toml::Table::try_from(self).expect("prefs always serialize")
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut p: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if p.format < 2 {
            // Format 1 files cannot tell whether the user chose auto-slice or inherited the
            // old default, so the setting goes back to off once; it is opt-in from now on.
            p.auto_slice = false;
            p.format = FORMAT;
        }
        // Hand-edited values outside what Preferences offers are brought back into range.
        p.recent_limit = p.recent_limit.min(MAX_RECENT);
        p.recent_files.truncate(p.recent_limit);
        p.auto_slice_delay_ms = p.auto_slice_delay_ms.clamp(
            *AUTO_SLICE_DELAY_RANGE_MS.start(),
            *AUTO_SLICE_DELAY_RANGE_MS.end(),
        );
        Ok(p)
    }

    #[cfg(test)]
    pub fn to_toml(&self) -> String {
        format!(
            "# Mica Slicer settings. Edit while Mica is closed; unknown keys are kept.\n{}",
            toml::to_string(self).expect("prefs always serialize")
        )
    }

    /// Writes every value (keys of a newer Mica in the file are kept). See [`Self::save_merged`].
    #[cfg(test)]
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.save_merged(path, &mut Saved::default())
    }

    /// Writes the values that differ from `saved` (what this process loaded or saved last)
    /// into the file as it is now, and remembers them in `saved`. Another Mica process using
    /// the same settings folder may have written other values meanwhile: they are kept, not
    /// overwritten with this process's older copy. The file is re-read and written under a
    /// lock, atomically (temporary file + rename), so a crash never leaves half a file and
    /// two processes saving at once lose nothing.
    pub fn save_merged(&self, path: &Path, saved: &mut Saved) -> Result<(), String> {
        let mine = self.to_table();
        mica_core::shared_file::locked(path, || {
            let mut disk = std::fs::read_to_string(path)
                .ok()
                .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
                .unwrap_or_default();
            apply_changes(&mut disk, saved.0.as_ref(), &mine);
            let text = format!(
                "# Mica Slicer settings. Edit while Mica is closed; unknown keys are kept.\n{}",
                toml::to_string(&disk).map_err(|e| e.to_string())?
            );
            mica_core::shared_file::write_text(path, &text)
        })?;
        saved.0 = Some(mine);
        Ok(())
    }

    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| *p != path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(self.recent_limit);
    }

    /// Changes how many recent files are kept; a smaller number drops the oldest at once.
    pub fn set_recent_limit(&mut self, n: usize) {
        self.recent_limit = n.min(MAX_RECENT);
        self.recent_files.truncate(self.recent_limit);
    }

    /// The automatic-slice delay as a duration.
    pub fn auto_slice_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.auto_slice_delay_ms)
    }

    pub fn has_printer(&self, vendor: &str, name: &str) -> bool {
        self.my_printers
            .iter()
            .any(|p| p == &printer_id(vendor, name))
    }

    pub fn add_printer(&mut self, vendor: &str, name: &str) {
        if !self.has_printer(vendor, name) {
            self.my_printers.push(printer_id(vendor, name));
        }
    }

    pub fn remove_printer(&mut self, vendor: &str, name: &str) {
        self.my_printers.retain(|p| p != &printer_id(vendor, name));
    }

    /// `(vendor, preset)` pairs of the remembered printers.
    pub fn printers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.my_printers.iter().filter_map(|p| p.split_once('/'))
    }
}

pub fn printer_id(vendor: &str, name: &str) -> String {
    format!("{vendor}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_recents() {
        let mut p = Prefs {
            selection: Selection {
                vendor: "Prusa".into(),
                printer: "Prusa CORE One HF0.4 nozzle".into(),
                print: "0.20mm SPEED @COREONE HF0.4".into(),
                filaments: vec!["Prusament PLA @COREONE HF0.4".into()],
                bed: String::new(),
            },
            ..Default::default()
        };
        p.add_printer("Prusa", "Prusa CORE One HF0.4 nozzle");
        p.add_printer("Prusa", "Prusa CORE One HF0.4 nozzle");
        assert_eq!(p.my_printers.len(), 1);
        // The settings rail's topic and folded sections survive a restart.
        p.settings_topic = "Quality".into();
        p.collapsed_sections = vec!["Quality/Seam".into()];
        for i in 0..12 {
            p.push_recent(PathBuf::from(format!("/tmp/model{i}.stl")));
        }
        p.push_recent(PathBuf::from("/tmp/model11.stl"));
        assert_eq!(p.recent_files.len(), DEFAULT_RECENT);
        assert_eq!(p.recent_files[0], PathBuf::from("/tmp/model11.stl"));
        let text = p.to_toml();
        assert!(text.starts_with("# Mica Slicer settings"));
        assert_eq!(Prefs::from_toml(&text).unwrap(), p);
        let dir = std::env::temp_dir().join(format!("mica-prefs-{}", std::process::id()));
        let path = dir.join("sub").join(FILE_NAME);
        p.save(&path).unwrap();
        let (loaded, warning) = Prefs::load(&path);
        assert_eq!(loaded, p);
        assert!(warning.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn features_default_on_except_brick_layers_and_round_trip() {
        let p = Prefs::from_toml("theme = \"light\"\n").unwrap();
        assert!(p.features.textures && p.features.color_mix && p.features.mcp);
        assert!(!p.features.brick_layers_enabled());
        let mut p = Prefs::default();
        p.features.color_mix = false;
        p.features.mcp = false;
        let text = p.to_toml();
        assert!(text.contains("[features]"), "{text}");
        let back = Prefs::from_toml(&text).unwrap();
        assert_eq!(back.features, p.features);
        // The CLI reads the same table from the same file.
        assert_eq!(
            mica_profiles::features::Features::from_toml(&text).unwrap(),
            p.features
        );
    }

    #[test]
    fn two_processes_saving_keep_each_others_changes() {
        let dir = std::env::temp_dir().join(format!("mica-prefs-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(
            &path,
            "future_key = 1\ntheme = \"light\"\n[window]\nwidth = 800\n",
        )
        .unwrap();
        // Two Mica processes start from the same file.
        let (mut a, _, mut a_saved) = Prefs::load_tracked(&path);
        let (mut b, _, mut b_saved) = Prefs::load_tracked(&path);
        a.welcome_seen = true;
        a.window.width = 1000.;
        a.save_merged(&path, &mut a_saved).unwrap();
        // The second still has its old copy of those values, and changes others.
        b.push_recent(PathBuf::from("/tmp/b.stl"));
        b.window.height = 700.;
        b.last_version = "9.9.9".into();
        b.save_merged(&path, &mut b_saved).unwrap();
        let (merged, warning) = Prefs::load(&path);
        assert!(warning.is_none());
        assert!(merged.welcome_seen);
        assert_eq!(merged.window.width, 1000.);
        assert_eq!(merged.window.height, 700.);
        assert_eq!(merged.recent_files, vec![PathBuf::from("/tmp/b.stl")]);
        assert_eq!(merged.last_version, "9.9.9");
        assert_eq!(merged.theme, "light");
        // Keys of a newer Mica survive both saves.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("future_key = 1"), "{text}");
        // A value the first changes back is written, though the second has it too.
        a.welcome_seen = false;
        a.save_merged(&path, &mut a_saved).unwrap();
        b.save_merged(&path, &mut b_saved).unwrap();
        assert!(!Prefs::load(&path).0.welcome_seen);
        // Many saves from two writers at once lose nothing and leave no temporary file.
        std::thread::scope(|s| {
            for who in ["a", "b"] {
                let path = &path;
                s.spawn(move || {
                    let (mut p, _, mut saved) = Prefs::load_tracked(path);
                    for i in 0..40 {
                        p.visible_filaments
                            .insert(format!("{who}{i}"), vec![format!("PLA {i}")]);
                        p.save_merged(path, &mut saved).unwrap();
                    }
                });
            }
        });
        let (p, _) = Prefs::load(&path);
        assert_eq!(p.visible_filaments.len(), 80);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .all(|n| n == FILE_NAME || n == "mica.toml.lock"),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_settings_file_is_merged_too() {
        // Neither process found a file: the second's save keeps what the first wrote.
        let dir = std::env::temp_dir().join(format!("mica-prefs-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FILE_NAME);
        let (mut a, _, mut a_saved) = Prefs::load_tracked(&path);
        let (mut b, _, mut b_saved) = Prefs::load_tracked(&path);
        a.welcome_seen = true;
        a.save_merged(&path, &mut a_saved).unwrap();
        b.theme = "light".into();
        b.save_merged(&path, &mut b_saved).unwrap();
        let (p, _) = Prefs::load(&path);
        assert!(p.welcome_seen);
        assert_eq!(p.theme, "light");
        // An old format file is brought up to date by the first save.
        std::fs::write(&path, "format = 1\nauto_slice = true\n").unwrap();
        let (c, _, mut c_saved) = Prefs::load_tracked(&path);
        c.save_merged(&path, &mut c_saved).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("format = 2") && text.contains("auto_slice = false"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn broken_or_missing_files_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("mica-prefs-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        let (p, w) = Prefs::load(&path);
        assert_eq!(p, Prefs::default());
        assert!(w.is_none());
        std::fs::write(&path, "this is = not [valid").unwrap();
        let (p, w) = Prefs::load(&path);
        assert_eq!(p, Prefs::default());
        assert!(w.is_some());
        // Unknown keys and missing sections are tolerated.
        std::fs::write(&path, "future_key = 1\n[window]\nwidth = 800\n").unwrap();
        let (p, w) = Prefs::load(&path);
        assert!(w.is_none());
        assert_eq!(p.window.width, 800.);
        assert_eq!(p.window.height, Window::default().height);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_slice_is_opt_in_and_old_files_reset_it_once() {
        assert!(!Prefs::default().auto_slice);
        assert!(Prefs::default().sidebar_right);
        // A format 1 file (auto-slice defaulted to on) comes back with it off, once.
        let old = Prefs::from_toml("format = 1\nauto_slice = true\n").unwrap();
        assert!(!old.auto_slice);
        assert_eq!(old.format, FORMAT);
        // A format 2 file keeps the user's explicit choice, and the sidebar side.
        let p = Prefs {
            auto_slice: true,
            sidebar_right: false,
            ..Default::default()
        };
        let again = Prefs::from_toml(&p.to_toml()).unwrap();
        assert!(again.auto_slice);
        assert!(!again.sidebar_right);
    }

    #[test]
    fn toolbar_size_defaults_to_normal_and_persists() {
        assert_eq!(Prefs::default().toolbar_size, ToolbarSize::Normal);
        for size in ToolbarSize::ALL {
            let p = Prefs {
                toolbar_size: size,
                ..Default::default()
            };
            let text = p.to_toml();
            assert!(text.contains(&format!("toolbar_size = \"{}\"", size.key())));
            assert_eq!(Prefs::from_toml(&text).unwrap().toolbar_size, size);
        }
        // A hand-edited value Mica does not know falls back to Normal, not to all defaults.
        let odd = Prefs::from_toml("toolbar_size = \"huge\"\nauto_slice_delay_ms = 900\n").unwrap();
        assert_eq!(odd.toolbar_size, ToolbarSize::Normal);
        assert_eq!(odd.auto_slice_delay_ms, 900);
    }

    #[test]
    fn first_run_is_dark_and_the_choice_persists() {
        use crate::theme::Mode;
        assert_eq!(Mode::from_name(&Prefs::default().theme), Mode::Dark);
        // A file written by an older build with `theme = "system"` still starts dark.
        let old = Prefs::from_toml("theme = \"system\"\n").unwrap();
        assert_eq!(Mode::from_name(&old.theme), Mode::Dark);
        let p = Prefs {
            theme: Mode::Light.name().into(),
            ..Default::default()
        };
        let again = Prefs::from_toml(&p.to_toml()).unwrap();
        assert_eq!(Mode::from_name(&again.theme), Mode::Light);
    }

    #[test]
    fn preferences_round_trip_and_stay_in_range() {
        let d = Prefs::default();
        assert_eq!(d.auto_slice_delay(), std::time::Duration::from_millis(500));
        assert_eq!(d.recent_limit, DEFAULT_RECENT);
        assert!(d.perspective && d.zoom_to_pointer && !d.reverse_zoom);
        assert_eq!(d.defaults, Defaults::default());
        let mut p = Prefs {
            auto_slice_delay_ms: 1500,
            theme: "follow-system".into(),
            perspective: false,
            zoom_to_pointer: false,
            reverse_zoom: true,
            defaults: Defaults {
                printer: "Prusa/Prusa CORE One HF0.4 nozzle".into(),
                print: "0.20mm SPEED @COREONE HF0.4".into(),
                filament: "Prusament PLA @COREONE HF0.4".into(),
            },
            ..Default::default()
        };
        for i in 0..8 {
            p.push_recent(PathBuf::from(format!("/tmp/m{i}.stl")));
        }
        // A smaller limit drops the oldest at once; later files respect it.
        p.set_recent_limit(3);
        assert_eq!(p.recent_files.len(), 3);
        assert_eq!(p.recent_files[0], PathBuf::from("/tmp/m7.stl"));
        p.push_recent(PathBuf::from("/tmp/new.stl"));
        assert_eq!(p.recent_files.len(), 3);
        p.set_recent_limit(0);
        p.push_recent(PathBuf::from("/tmp/none.stl"));
        assert!(p.recent_files.is_empty());
        p.set_recent_limit(500);
        assert_eq!(p.recent_limit, MAX_RECENT);
        let again = Prefs::from_toml(&p.to_toml()).unwrap();
        assert_eq!(again, p);
        // Hand-edited values come back into range.
        let odd = Prefs::from_toml("auto_slice_delay_ms = 1\nrecent_limit = 999\n").unwrap();
        assert_eq!(odd.auto_slice_delay_ms, *AUTO_SLICE_DELAY_RANGE_MS.start());
        assert_eq!(odd.recent_limit, MAX_RECENT);
        // Files written before these settings existed get the defaults.
        let old = Prefs::from_toml("format = 2\nauto_slice = true\n").unwrap();
        assert_eq!(old.auto_slice_delay_ms, DEFAULT_AUTO_SLICE_DELAY_MS);
        assert_eq!(old.recent_limit, DEFAULT_RECENT);
        assert!(old.perspective);
    }
}
