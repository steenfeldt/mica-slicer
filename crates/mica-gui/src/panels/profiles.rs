// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Profile bundles (Prusa INI, Orca/Snapmaker JSON), the user's own Mica profiles, the
//! current printer/print/filament selection and the composed configuration.

use crate::state::{Prefs, printers};
use mica_core::config::{Config, ConfigValue, Scope, option_defs};
use mica_profiles::{
    orca,
    prusa::{Bundle, Kind},
    user::{Store, UserProfile},
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const SNAPMAKER_BEDS: [&str; 4] = [
    "High Temp Plate",
    "Textured PEI Plate",
    "Cool Plate",
    "Engineering Plate",
];

/// A loaded profile bundle: PrusaSlicer's (INI) or one OrcaSlicer vendor's (JSON: Snapmaker,
/// RatRig, Voron, …; loaded on first use, see `Profiles::ensure_vendor`).
pub enum Vendor {
    Prusa(Bundle),
    Orca(orca::Bundle),
}

impl Vendor {
    /// The `inherits_source` written into Mica user profiles derived from this vendor.
    pub fn source_name(&self) -> String {
        match self {
            Vendor::Prusa(b) => b
                .vendor
                .get("name")
                .cloned()
                .unwrap_or_else(|| "PrusaResearch".into()),
            Vendor::Orca(b) => b.primary_vendor().to_string(),
        }
    }
    /// Directory name under a slicer's `profiles/` that holds this vendor's bed models,
    /// textures and pictures.
    pub fn asset_dir(&self) -> &str {
        match self {
            Vendor::Prusa(_) => "PrusaResearch",
            Vendor::Orca(b) => b.primary_vendor(),
        }
    }
    /// The bed model and texture file names of a printer model, as the bundle states them.
    pub fn bed_asset_names(&self, printer_model: &str) -> (Option<String>, Option<String>) {
        match self {
            Vendor::Prusa(b) => {
                let m = b.printer_models.get(printer_model);
                (
                    m.and_then(|m| m.get("bed_model").cloned()),
                    m.and_then(|m| m.get("bed_texture").cloned()),
                )
            }
            Vendor::Orca(b) => {
                let m = b.machine_model(printer_model);
                let s = |k: &str| {
                    m.and_then(|m| m.get(k))
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                };
                (s("bed_model"), s("bed_texture"))
            }
        }
    }
    pub fn version(&self) -> String {
        match self {
            Vendor::Prusa(b) => b.vendor.get("config_version").cloned().unwrap_or_default(),
            Vendor::Orca(b) => b
                .vendors
                .values()
                .next()
                .and_then(|v| v.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        }
    }
}

/// Which preset list a kind belongs to, shared by both bundle types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetKind {
    Printer,
    Print,
    Filament,
}

impl PresetKind {
    pub fn prusa(self) -> Kind {
        match self {
            PresetKind::Printer => Kind::Printer,
            PresetKind::Print => Kind::Print,
            PresetKind::Filament => Kind::Filament,
        }
    }
    fn orca(self) -> orca::Kind {
        match self {
            PresetKind::Printer => orca::Kind::Machine,
            PresetKind::Print => orca::Kind::Process,
            PresetKind::Filament => orca::Kind::Filament,
        }
    }
}

/// The settings of the selected Mica user profiles (canonical names → text), per scope.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserSettings {
    pub print: BTreeMap<String, String>,
    pub printer: BTreeMap<String, String>,
    /// One entry per tool.
    pub filaments: Vec<BTreeMap<String, String>>,
}

pub struct Profiles {
    pub vendors: Vec<(String, Vendor)>,
    pub vendor: usize,
    pub printer: String,
    pub process: String,
    pub filaments: Vec<String>,
    pub bed: String,
    pub printers: Vec<String>,
    pub processes: Vec<String>,
    pub materials: Vec<String>,
    pub warnings: Vec<String>,
    /// Mica user profiles derived from the loaded Orca bundles: imported Orca presets
    /// (registered in the bundle as user presets) and Mica's own (overlays on their parent).
    pub(crate) orca_user: Vec<UserProfile>,
    /// Every profile in the store, read once (and updated by Mica's own changes), so pickers
    /// can tell system, own and imported profiles apart without reading files every frame.
    pub(crate) user: Vec<UserProfile>,
    /// The other slicers' user preset folders (`import::source_dirs` of the home folder; none
    /// in tests unless a test points it at a fake home).
    pub import_dirs: Vec<mica_profiles::import::SourceDir>,
    /// The home folder (and Windows' `APPDATA`) those folders were found in, where the other
    /// slicers' app settings (their visible filaments) are read; `None` in tests unless set.
    pub import_home: Option<(PathBuf, Option<PathBuf>)>,
    /// The print and filament lists also hold presets made for other printers ("Show all").
    pub show_incompatible: bool,
    /// Which listed print / filament presets are not made for the selected printer.
    pub incompatible: BTreeSet<(Kind, String)>,
    pub store: Store,
    /// The OrcaSlicer `profiles` folder the Orca vendors come from (`MICA_ORCA_PROFILES`, an
    /// installed OrcaSlicer, or the pinned checkout); `None` when there is none.
    pub orca_root: Option<PathBuf>,
}

impl Profiles {
    pub fn discover_with_store(store: Store) -> Self {
        let mut vendors = Vec::new();
        let mut warnings = Vec::new();
        let mut candidates = Vec::new();
        if let Some(p) = std::env::var_os("MICA_PRUSA_BUNDLE") {
            candidates.push(PathBuf::from(p));
        }
        if let Some(home) = mica_core::user_data::home() {
            for p in [
                "Library/Application Support/PrusaSlicer/vendor/PrusaResearch.ini",
                ".config/PrusaSlicer/vendor/PrusaResearch.ini",
                ".var/app/com.prusa3d.PrusaSlicer/config/PrusaSlicer/vendor/PrusaResearch.ini",
            ] {
                candidates.push(Path::new(&home).join(p));
            }
        }
        if let Some(p) = mica_core::user_data::var_dir("APPDATA") {
            candidates.push(Path::new(&p).join("PrusaSlicer/vendor/PrusaResearch.ini"));
        }
        // A development checkout's pinned copy (never in a packaged build).
        if let Some(upstream) = mica_core::user_data::upstream_dir() {
            candidates.push(upstream.join("prusa-profiles/PrusaResearch/2.5.12.ini"));
        }
        let mut orca_user = Vec::new();
        let user_profiles: Vec<UserProfile> = [Kind::Printer, Kind::Print, Kind::Filament]
            .into_iter()
            .filter_map(|k| match store.list(k) {
                Ok(list) => Some(list),
                Err(e) => {
                    warnings.push(e.to_string());
                    None
                }
            })
            .flatten()
            .collect();
        if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
            match std::fs::read_to_string(p) {
                Ok(s) => {
                    let mut b = Bundle::parse(&s);
                    let vendor_name = b.vendor.get("name").cloned().unwrap_or_default();
                    for p in &user_profiles {
                        if p.orca.is_none()
                            && (p.inherits_source.is_empty() || p.inherits_source == vendor_name)
                        {
                            b.add_user_preset(p.kind, &p.name, &p.inherits, &p.settings);
                        }
                    }
                    vendors.push(("Prusa".into(), Vendor::Prusa(b)));
                }
                Err(e) => warnings.push(e.to_string()),
            }
        } else {
            warnings.push(
                "Prusa profiles missing. Install PrusaSlicer or set MICA_PRUSA_BUNDLE.".into(),
            );
        }
        let root = std::env::var_os("MICA_ORCA_PROFILES").map(PathBuf::from);
        let mut orca_root = root.clone();
        match orca::discovery::load_installed(root.as_deref(), "Snapmaker") {
            Ok((mut b, found)) => {
                orca_root = Some(found);
                // OrcaSlicer's and Snapmaker Orca's own user presets are not read here: they
                // are imported on request (Settings › Profiles), and only Mica's copies load.
                let mine: Vec<&UserProfile> = user_profiles
                    .iter()
                    .filter(|p| mica_profiles::user::on_orca_vendor(p, "Snapmaker", &b))
                    .collect();
                warnings.extend(mica_profiles::user::register_orca(
                    &mut b,
                    mine.iter().copied(),
                ));
                orca_user.extend(mine.into_iter().cloned());
                vendors.push(("Snapmaker".into(), Vendor::Orca(b)));
            }
            Err(e) => warnings.push(e.to_string()),
        }
        let mut p = Self {
            vendors,
            vendor: 0,
            printer: "Prusa CORE One HF0.4 nozzle".into(),
            process: "0.20mm SPEED @COREONE HF0.4".into(),
            filaments: vec!["Prusament PLA @COREONE HF0.4".into()],
            bed: "High Temp Plate".into(),
            printers: vec![],
            processes: vec![],
            materials: vec![],
            warnings,
            orca_user,
            user: user_profiles,
            import_dirs: vec![],
            import_home: None,
            show_incompatible: false,
            incompatible: BTreeSet::new(),
            store,
            orca_root: orca_root.filter(|r| r.is_dir()),
        };
        if let Some(home) = mica_core::user_data::home() {
            let appdata = mica_core::user_data::var_dir("APPDATA");
            p.look_in(&home, appdata.as_deref());
        }
        p.refresh();
        p
    }

    /// Where to look for the other slicers' presets and settings: the home folder `home`
    /// (tests point it at a fake one).
    pub fn look_in(&mut self, home: &Path, appdata: Option<&Path>) {
        self.import_dirs = mica_profiles::import::source_dirs(home, appdata);
        self.import_home = Some((home.to_path_buf(), appdata.map(Path::to_path_buf)));
    }

    /// Loads one OrcaSlicer vendor's full bundle (`Ratrig`, `Voron`, …) with the shared
    /// filament library. Slow for big vendors: call it off the UI thread where possible.
    pub fn load_orca_vendor(root: Option<&Path>, key: &str) -> Result<orca::Bundle, String> {
        orca::discovery::load_installed(root, key)
            .map(|(b, _)| b)
            .map_err(|e| format!("{key} profiles could not be loaded: {e}"))
    }

    /// Adds a loaded Orca vendor bundle (and the Mica user profiles derived from it); returns
    /// its index. A vendor that is already loaded stays as it is.
    pub fn install_vendor(&mut self, key: &str, mut bundle: orca::Bundle) -> usize {
        if let Some(vi) = self.vendor_index(key) {
            return vi;
        }
        let mine: Vec<UserProfile> = self
            .user
            .iter()
            .filter(|p| mica_profiles::user::on_orca_vendor(p, key, &bundle))
            .cloned()
            .collect();
        self.warnings
            .extend(mica_profiles::user::register_orca(&mut bundle, &mine));
        // Shared-library filaments are listed once, whichever vendors load them.
        for p in mine {
            if !self
                .orca_user
                .iter()
                .any(|o| o.kind == p.kind && o.name == p.name)
            {
                self.orca_user.push(p);
            }
        }
        self.vendors.push((key.to_string(), Vendor::Orca(bundle)));
        self.vendors.len() - 1
    }

    /// The index of vendor `key`, loading its Orca bundle first when needed (on this thread).
    pub fn ensure_vendor(&mut self, key: &str) -> Result<usize, String> {
        if let Some(vi) = self.vendor_index(key) {
            return Ok(vi);
        }
        if key == "Prusa" || !orca::discovery::is_index_name(key) {
            return Err(format!("No {key} profiles are installed."));
        }
        let bundle = Self::load_orca_vendor(self.orca_root.as_deref(), key)?;
        Ok(self.install_vendor(key, bundle))
    }

    /// At startup: loads the Orca vendors that remembered printers name (My printers, the
    /// last selection, the default printer, printers.toml links), so they restore. Other
    /// vendors wait until the Add printer gallery picks one.
    pub fn load_referenced<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) {
        let mut seen: Vec<&str> = vec![];
        for key in keys {
            if key.is_empty() || seen.contains(&key) || self.vendor_index(key).is_some() {
                continue;
            }
            seen.push(key);
            if let Err(e) = self.ensure_vendor(key) {
                self.warnings.push(e);
            }
        }
    }

    /// The Orca dialect of the selected vendor's bundle (`None` for PrusaSlicer bundles).
    pub fn orca_dialect(&self) -> Option<orca::Dialect> {
        match self.vendors.get(self.vendor)? {
            (_, Vendor::Orca(b)) => Some(b.dialect),
            (_, Vendor::Prusa(_)) => None,
        }
    }

    pub fn vendor_name(&self) -> &str {
        self.vendors
            .get(self.vendor)
            .map_or("", |(name, _)| name.as_str())
    }

    /// The selected printer model's bed sheet and texture files, looked up in the pinned
    /// upstream checkouts and the installed slicers (never bundled).
    pub fn bed_assets(&self, config: &Config) -> crate::scene::bed_assets::AssetFiles {
        self.bed_assets_of(self.vendor, config)
    }

    /// `bed_assets` for a printer of another vendor (an opened G-code file's).
    pub fn bed_assets_of(
        &self,
        vendor: usize,
        config: &Config,
    ) -> crate::scene::bed_assets::AssetFiles {
        let Some((_, vendor)) = self.vendors.get(vendor) else {
            return Default::default();
        };
        let Some(model) = config.string("printer_model").map(str::trim) else {
            return Default::default();
        };
        let (bed_model, bed_texture) = vendor.bed_asset_names(model);
        crate::scene::bed_assets::resolve(
            vendor.asset_dir(),
            bed_model.as_deref(),
            bed_texture.as_deref(),
        )
    }

    /// The slicer the selected printer's profile was made for: PrusaSlicer for a Prusa bundle
    /// printer, OrcaSlicer for an Orca vendor profile. The default of `mica_slicing_rules`.
    pub fn profile_rules(&self) -> mica_core::rules::Rules {
        if self.is_orca() {
            mica_core::rules::Rules::OrcaSlicer
        } else {
            mica_core::rules::Rules::PrusaSlicer
        }
    }

    pub fn is_orca(&self) -> bool {
        matches!(self.vendors.get(self.vendor), Some((_, Vendor::Orca(_))))
    }

    pub fn vendor_index(&self, name: &str) -> Option<usize> {
        self.vendors.iter().position(|(n, _)| n == name)
    }

    /// First run: scan PrusaSlicer and Snapmaker Orca for the user's printers (their system
    /// presets) and last selections, and remember them in `prefs`. The other slicers' own
    /// presets are not imported here: Mica asks first (`App::offer_imports`).
    pub fn detect(&mut self, prefs: &mut Prefs) {
        prefs.detected = true;
        if let Some(dir) = printers::prusaslicer_dir()
            && let Ok(text) = std::fs::read_to_string(dir.join("PrusaSlicer.ini"))
            && let Some(vi) = self.vendor_index("Prusa")
        {
            let state = printers::parse_prusaslicer_ini(&text);
            if let Vendor::Prusa(b) = &mut self.vendors[vi].1 {
                for name in printers::presets_for_models(b, &state.models) {
                    prefs.add_printer("Prusa", &name);
                }
            }
            if !state.printer.is_empty() {
                self.vendor = vi;
                self.printer = state.printer.clone();
                self.refresh();
                if self.processes.contains(&state.print) {
                    self.process = state.print.clone();
                    self.refresh();
                }
                if self.materials.contains(&state.filament) {
                    self.filaments.fill(state.filament.clone());
                }
            }
        }
        if self.vendor_index("Snapmaker").is_some() && printers::snapmaker_orca_installed() {
            prefs.add_printer("Snapmaker", "Snapmaker U1 (0.4 nozzle)");
        }
        if prefs.my_printers.is_empty() {
            prefs.add_printer(self.vendor_name(), &self.printer);
        }
        self.store_selection(prefs);
    }

    /// Restores the remembered selection, if those presets still exist.
    pub fn restore(&mut self, prefs: &Prefs) {
        let s = &prefs.selection;
        if let Some(vi) = self.vendor_index(&s.vendor) {
            self.vendor = vi;
            self.printer = s.printer.clone();
            self.refresh();
            if self.processes.contains(&s.print) {
                self.process = s.print.clone();
                self.refresh();
            }
            for (i, f) in s.filaments.iter().enumerate() {
                if i < self.filaments.len() && self.materials.contains(f) {
                    self.filaments[i] = f.clone();
                }
            }
            if SNAPMAKER_BEDS.contains(&s.bed.as_str()) {
                self.bed = s.bed.clone();
            }
        }
    }

    pub fn store_selection(&self, prefs: &mut Prefs) {
        prefs.selection = crate::state::Selection {
            vendor: self.vendor_name().to_string(),
            printer: self.printer.clone(),
            print: self.process.clone(),
            filaments: self.filaments.clone(),
            bed: self.bed.clone(),
        };
    }

    /// The remembered printers that exist in the loaded bundles: `(vendor index, name)`.
    pub fn my_printers(&self, prefs: &Prefs) -> Vec<(usize, String)> {
        prefs
            .printers()
            .filter_map(|(vendor, name)| {
                let vi = self.vendor_index(vendor)?;
                self.printer_exists(vi, name)
                    .then(|| (vi, name.to_string()))
            })
            .collect()
    }

    pub fn printer_exists(&self, vendor: usize, name: &str) -> bool {
        match &self.vendors[vendor].1 {
            Vendor::Prusa(b) => b.preset_names(Kind::Printer).contains(&name),
            Vendor::Orca(b) => b.preset_names(orca::Kind::Machine).contains(&name),
        }
    }

    /// All printer presets of every vendor: `(vendor index, vendor name, preset name)`.
    pub fn all_printers(&self) -> Vec<(usize, String, String)> {
        let mut out = Vec::new();
        for (vi, (vendor, v)) in self.vendors.iter().enumerate() {
            let names: Vec<&str> = match v {
                Vendor::Prusa(b) => b.preset_names(Kind::Printer),
                Vendor::Orca(b) => {
                    let mut n = b.preset_names(orca::Kind::Machine);
                    n.sort_unstable();
                    n
                }
            };
            out.extend(
                names
                    .into_iter()
                    .map(|n| (vi, vendor.clone(), n.to_string())),
            );
        }
        out
    }

    /// Switches to a printer and picks sensible print/filament defaults for it.
    pub fn select_printer(&mut self, vendor: usize, name: &str) {
        let changed_vendor = vendor != self.vendor;
        let changed_printer = changed_vendor || name != self.printer;
        self.vendor = vendor;
        self.printer = name.to_string();
        if changed_vendor {
            self.process.clear();
            self.filaments = vec![String::new()];
        }
        self.refresh();
        if !self.processes.contains(&self.process)
            || (changed_printer && self.is_incompatible(PresetKind::Print, &self.process))
            || changed_vendor
        {
            if let Some(p) = self.default_process() {
                self.process = p;
            }
            self.refresh();
        }
        if (!self.materials.contains(&self.filaments[0])
            || (changed_printer && self.is_incompatible(PresetKind::Filament, &self.filaments[0]))
            || changed_vendor)
            && let Some(f) = self.default_material()
        {
            self.filaments.fill(f);
        }
        // A different printer starts on its own plate; the last choice only sticks for the
        // same printer.
        if changed_printer && self.is_orca() {
            self.bed = self.default_bed();
            self.refresh();
        }
    }

    /// The print profile Mica picks for the current printer: a 0.20 mm SPEED / Standard one.
    fn default_process(&self) -> Option<String> {
        let ok = |p: &&String| !self.is_incompatible(PresetKind::Print, p);
        self.processes
            .iter()
            .filter(ok)
            .find(|p| p.contains("0.20") && (p.contains("SPEED") || p.contains("Standard")))
            .or_else(|| {
                self.processes
                    .iter()
                    .filter(ok)
                    .find(|p| p.contains("0.20"))
            })
            .cloned()
    }

    /// The filament Mica picks for the current printer and print profile: a PLA.
    fn default_material(&self) -> Option<String> {
        self.materials
            .iter()
            .filter(|f| !self.is_incompatible(PresetKind::Filament, f))
            .find(|f| f.starts_with("Prusament PLA") || f.starts_with("Generic PLA"))
            .cloned()
    }

    /// New project: the current printer with its default print and filament profiles, or the
    /// ones Preferences name (`print`, `filament`; empty = automatic) when the printer offers
    /// them. The bed type goes back to the printer's first.
    pub fn reset_to_defaults(&mut self, print: &str, filament: &str) {
        self.refresh();
        if let Some(p) = Some(print)
            .filter(|p| !p.is_empty() && self.processes.iter().any(|x| x == p))
            .map(str::to_string)
            .or_else(|| self.default_process())
        {
            self.process = p;
        }
        self.refresh();
        if let Some(f) = Some(filament)
            .filter(|f| !f.is_empty() && self.materials.iter().any(|x| x == f))
            .map(str::to_string)
            .or_else(|| self.default_material())
        {
            self.filaments.fill(f);
        }
        if self.is_orca() {
            self.bed = self.default_bed();
        }
        self.refresh();
    }

    /// The selected printer's own bed plate (Orca `default_bed_type`, e.g. the U1's Textured
    /// PEI Plate). A machine without one gets Orca's default, the Cool Plate, as the CLI does
    /// (`orca::Bundle::single_tool_config`); a plate Mica does not offer, the first known.
    pub fn default_bed(&self) -> String {
        if let Some((_, Vendor::Orca(b))) = self.vendors.get(self.vendor)
            && let Ok(m) = b.flatten(orca::Kind::Machine, &self.printer)
        {
            let bed = orca::bed_type_name(
                m.get("default_bed_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            if bed.is_empty() {
                return "Cool Plate".into();
            }
            if SNAPMAKER_BEDS.contains(&bed) {
                return bed.to_string();
            }
        }
        SNAPMAKER_BEDS[0].to_string()
    }

    pub fn refresh(&mut self) {
        let Some((_, v)) = self.vendors.get(self.vendor) else {
            return;
        };
        self.printers = match v {
            Vendor::Prusa(b) => b.preset_names(Kind::Printer),
            Vendor::Orca(b) => {
                let mut n = b.preset_names(orca::Kind::Machine);
                n.sort_unstable();
                n
            }
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        ensure(&mut self.printer, &self.printers);
        self.processes = self.listed(PresetKind::Print);
        ensure(&mut self.process, &self.processes);
        self.materials = self.listed(PresetKind::Filament);
        for f in &mut self.filaments {
            ensure(f, &self.materials);
        }
        if let Ok(c) = self.config() {
            let first = self.filaments[0].clone();
            self.filaments
                .resize(c.vector_len("nozzle_diameter").max(1), first);
        }
    }

    /// Compatible presets of a kind for the current printer (and print, for filaments),
    /// including Mica user profiles derived from Orca presets.
    fn compatible(&self, kind: PresetKind) -> Vec<String> {
        let Some((_, v)) = self.vendors.get(self.vendor) else {
            return vec![];
        };
        let mut names: Vec<String> = match v {
            Vendor::Prusa(b) => b
                .compatible_presets(
                    kind.prusa(),
                    &self.printer,
                    (kind == PresetKind::Filament).then_some(self.process.as_str()),
                )
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Vendor::Orca(b) => {
                let base = |kind: Kind, name: &str| {
                    self.orca_user
                        .iter()
                        .find(|p| p.kind == kind && p.name == name)
                        .map_or(name.to_string(), |p| p.orca_base().to_string())
                };
                let printer = base(Kind::Printer, &self.printer);
                let process = base(Kind::Print, &self.process);
                let mut n: Vec<String> = b
                    .compatible_presets(
                        kind.orca(),
                        &printer,
                        (kind == PresetKind::Filament).then_some(process.as_str()),
                    )
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                n.sort_unstable();
                // Mica's own profiles follow their parent; imported Orca presets are in the
                // bundle's list already.
                let user: Vec<String> = self
                    .orca_user
                    .iter()
                    .filter(|p| {
                        p.kind == kind.prusa() && p.orca.is_none() && n.contains(&p.inherits)
                    })
                    .map(|p| p.name.clone())
                    .collect();
                n.extend(user);
                n
            }
        };
        names.dedup();
        names
    }

    /// Every selectable preset of a kind in the selected vendor, compatible or not.
    pub(crate) fn all_presets(&self, kind: PresetKind) -> Vec<String> {
        let Some((_, v)) = self.vendors.get(self.vendor) else {
            return vec![];
        };
        let mut names: Vec<String> = match v {
            Vendor::Prusa(b) => b
                .preset_names(kind.prusa())
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Vendor::Orca(b) => {
                let mut n: Vec<String> = b
                    .preset_names(kind.orca())
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                n.sort_unstable();
                n.extend(
                    self.orca_user
                        .iter()
                        .filter(|p| p.kind == kind.prusa() && p.orca.is_none())
                        .map(|p| p.name.clone()),
                );
                n
            }
        };
        names.dedup();
        names
    }

    /// The compatible presets, then (with "Show all") the others, which are remembered in
    /// `incompatible`.
    fn listed(&mut self, kind: PresetKind) -> Vec<String> {
        let mut names = self.compatible(kind);
        self.incompatible.retain(|(k, _)| *k != kind.prusa());
        if self.show_incompatible {
            for n in self.all_presets(kind) {
                if !names.contains(&n) {
                    self.incompatible.insert((kind.prusa(), n.clone()));
                    names.push(n);
                }
            }
        }
        names
    }

    /// Whether a listed print or filament preset is not made for the selected printer.
    pub fn is_incompatible(&self, kind: PresetKind, name: &str) -> bool {
        self.incompatible
            .contains(&(kind.prusa(), name.to_string()))
    }

    /// The Mica user profile of this kind and name, if it is one.
    pub fn user_profile(&self, kind: PresetKind, name: &str) -> Option<&UserProfile> {
        self.user
            .iter()
            .find(|p| p.kind == kind.prusa() && p.name == name)
    }

    /// Whether a preset is one of the user's own (editable) profiles: made in Mica or imported.
    pub fn is_user_preset(&self, kind: PresetKind, name: &str) -> bool {
        self.user_profile(kind, name).is_some()
    }

    /// Where a listed preset comes from (pickers group and badge by it).
    pub fn origin(&self, kind: PresetKind, name: &str) -> Origin {
        match self.user_profile(kind, name) {
            None => Origin::System,
            Some(p) => match p.source() {
                Some(source) => Origin::Imported(source),
                None => Origin::Mine,
            },
        }
    }

    /// The parent of a Mica user profile, or the preset itself.
    pub fn system_parent(&self, kind: PresetKind, name: &str) -> String {
        self.user_profile(kind, name)
            .map_or(name.to_string(), |p| p.inherits.clone())
    }

    /// Saves `settings` as a user profile named `name` inheriting from `parent`, registers it
    /// and returns it.
    pub fn save_user_profile(
        &mut self,
        kind: PresetKind,
        name: &str,
        parent: &str,
        settings: &BTreeMap<String, String>,
    ) -> Result<UserProfile, String> {
        let (_, vendor) = self.vendors.get(self.vendor).ok_or("no profile bundle")?;
        let parent = self.system_parent(kind, parent);
        let existing = self.store.load(kind.prusa(), name).ok();
        let mut merged = existing
            .as_ref()
            .map(|p| p.settings.clone())
            .unwrap_or_default();
        merged.extend(settings.iter().map(|(k, v)| (k.clone(), v.clone())));
        let profile = UserProfile {
            kind: kind.prusa(),
            name: name.to_string(),
            inherits: parent,
            inherits_source: vendor.source_name(),
            inherits_version: vendor.version(),
            imported_from: existing.as_ref().and_then(|p| p.imported_from.clone()),
            settings: merged,
            import: existing.as_ref().and_then(|p| p.import.clone()),
            orca: existing.and_then(|p| p.orca),
        };
        self.store.save(&profile).map_err(|e| e.to_string())?;
        self.user
            .retain(|p| !(p.kind == profile.kind && p.name == profile.name));
        self.user.push(profile.clone());
        match &mut self.vendors[self.vendor].1 {
            Vendor::Prusa(b) => {
                b.add_user_preset(
                    profile.kind,
                    &profile.name,
                    &profile.inherits,
                    &profile.settings,
                );
            }
            Vendor::Orca(_) => {
                self.orca_user
                    .retain(|p| !(p.kind == profile.kind && p.name == profile.name));
                self.orca_user.push(profile.clone());
            }
        }
        Ok(profile)
    }

    /// Colour of a filament preset (`#RRGGBB`), from the preset or the tool's extruder colour.
    pub fn filament_colour(&self, name: &str) -> Option<[u8; 3]> {
        let (_, v) = self.vendors.get(self.vendor)?;
        let raw = match v {
            Vendor::Prusa(b) => b
                .flatten(Kind::Filament, name)
                .ok()?
                .get("filament_colour")
                .cloned(),
            Vendor::Orca(b) => {
                let flat = b.flatten(orca::Kind::Filament, name).ok()?;
                ["default_filament_colour", "filament_colour"]
                    .iter()
                    .find_map(|k| flat.get(*k))
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_array()?.first()?.as_str().map(str::to_string))
                    })
            }
        }?;
        parse_colour(raw.trim().trim_matches('"'))
    }

    pub fn nozzle_count(&self) -> usize {
        self.filaments.len().max(1)
    }

    /// The selected Snapmaker presets composed the way OrcaSlicer writes a project's
    /// `project_settings.config`: every option of the machine, the process and the per-tool
    /// filaments (one entry per slot), the project state (bed type, filament colours) and the
    /// preset names. `None` for a Prusa vendor. Mica user profiles derived from Orca presets
    /// are named as selected with their system parent in `inherits_group`; their own settings
    /// come back as canonical overlays per scope, to be applied on top and listed as changes.
    pub fn orca_project_config(
        &self,
        colours: &[String],
    ) -> Option<Result<(orca::RawConfig, UserSettings), String>> {
        use serde_json::Value;
        let Some((_, Vendor::Orca(b))) = self.vendors.get(self.vendor) else {
            return None;
        };
        let user = |kind: Kind, name: &str| {
            self.orca_user
                .iter()
                .find(|p| p.kind == kind && p.name == name)
        };
        let printer = user(Kind::Printer, &self.printer);
        let process = user(Kind::Print, &self.process);
        let filaments: Vec<Option<&UserProfile>> = self
            .filaments
            .iter()
            .map(|f| user(Kind::Filament, f))
            .collect();
        let filament_names: Vec<&str> = self
            .filaments
            .iter()
            .zip(&filaments)
            .map(|(name, u)| u.map_or(name.as_str(), |p| p.orca_base()))
            .collect();
        let mut raw = match mica_profiles::project::selected_orca(
            b,
            printer.map_or(self.printer.as_str(), |p| p.orca_base()),
            process.map_or(self.process.as_str(), |p| p.orca_base()),
            &filament_names,
            &self.bed,
            colours,
        ) {
            Ok(raw) => raw,
            Err(e) => return Some(Err(e)),
        };
        // Name the presets as selected (user profiles under their own names) so a reopened
        // project matches them again; Orca falls back to the parents in `inherits_group`.
        raw.insert(
            "printer_settings_id".into(),
            Value::String(self.printer.clone()),
        );
        raw.insert(
            "print_settings_id".into(),
            Value::String(self.process.clone()),
        );
        raw.insert(
            "filament_settings_id".into(),
            Value::Array(self.filaments.iter().cloned().map(Value::String).collect()),
        );
        let mut inherits: Vec<Value> = std::iter::once(process)
            .chain(filaments.iter().copied())
            .chain(std::iter::once(printer))
            .map(|u| Value::String(u.map_or(String::new(), |p| p.inherits.clone())))
            .collect();
        if let Some(Value::Array(group)) = raw.get("inherits_group") {
            for (slot, old) in inherits.iter_mut().zip(group) {
                if slot.as_str() == Some("") {
                    *slot = old.clone();
                }
            }
        }
        if inherits
            .iter()
            .any(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            raw.insert("inherits_group".into(), Value::Array(inherits));
        }
        let settings = UserSettings {
            print: process.map(|p| p.settings.clone()).unwrap_or_default(),
            printer: printer.map(|p| p.settings.clone()).unwrap_or_default(),
            filaments: filaments
                .iter()
                .map(|u| u.map(|p| p.settings.clone()).unwrap_or_default())
                .collect(),
        };
        Some(Ok((raw, settings)))
    }

    /// The printer presets of a vendor whose `printer_model` is `code` (`COREONE`), with their
    /// `printer_variant` (nozzle): how an opened G-code file that only names its model code
    /// (`M862.3`, binary printer metadata) finds its preset.
    pub fn presets_of_model(&self, code: &str) -> Vec<(usize, String, String)> {
        let mut out = Vec::new();
        for (vi, (_, v)) in self.vendors.iter().enumerate() {
            if let Vendor::Prusa(b) = v {
                for name in b.preset_names(Kind::Printer) {
                    let Ok(flat) = b.flatten(Kind::Printer, name) else {
                        continue;
                    };
                    if flat.get("printer_model").map(|m| m.trim()) == Some(code) {
                        let variant = flat.get("printer_variant").cloned().unwrap_or_default();
                        out.push((vi, name.to_string(), variant));
                    }
                }
            }
        }
        out
    }

    /// A printer preset's configuration with Mica's default print and filament profiles for
    /// it (what an opened G-code file for another printer is checked against).
    pub fn printer_defaults_config(&self, vendor: usize, printer: &str) -> Result<Config, String> {
        let (_, v) = self.vendors.get(vendor).ok_or("No such profile vendor")?;
        let pick = |names: Vec<&str>, prefer: &dyn Fn(&str) -> bool| -> Option<String> {
            names
                .iter()
                .find(|n| prefer(n))
                .or(names.first())
                .map(|s| s.to_string())
        };
        let print_pref =
            |p: &str| p.contains("0.20") && (p.contains("SPEED") || p.contains("Standard"));
        let pla = |f: &str| f.contains("PLA");
        match v {
            Vendor::Prusa(b) => {
                let print = pick(
                    b.compatible_presets(Kind::Print, printer, None),
                    &print_pref,
                )
                .ok_or("the printer has no print profiles")?;
                let filament = pick(
                    b.compatible_presets(Kind::Filament, printer, Some(&print)),
                    &pla,
                )
                .ok_or("the printer has no filament profiles")?;
                b.full_config(printer, &print, &filament)
                    .map_err(|e| e.to_string())
            }
            Vendor::Orca(b) => {
                let process = pick(
                    b.compatible_presets(orca::Kind::Process, printer, None),
                    &print_pref,
                )
                .ok_or("the printer has no process profiles")?;
                let filament = pick(
                    b.compatible_presets(orca::Kind::Filament, printer, Some(&process)),
                    &pla,
                )
                .ok_or("the printer has no filament profiles")?;
                b.single_tool_config(printer, &process, &filament, None)
                    .map_err(|e| e.to_string())
            }
        }
    }

    /// The composed configuration of the selected presets (no editor overlays).
    pub fn config(&self) -> Result<Config, String> {
        self.config_for_bed(&self.bed)
    }

    /// The configuration with another bed type (a plate's own, see `plate_settings`); only
    /// Snapmaker / Orca bundles have bed types, Prusa ones ignore it.
    pub fn config_for_bed(&self, bed: &str) -> Result<Config, String> {
        let (_, v) = self
            .vendors
            .get(self.vendor)
            .ok_or("No profile bundles available")?;
        let resolve = |f: &str| -> Result<Config, String> {
            match v {
                Vendor::Prusa(b) => b
                    .full_config(&self.printer, &self.process, f)
                    .map_err(|e| e.to_string()),
                Vendor::Orca(b) => {
                    // The same composition as the CLI (`user::orca_config`).
                    let user = |kind: Kind, name: &str| {
                        self.orca_user
                            .iter()
                            .find(|p| p.kind == kind && p.name == name)
                    };
                    mica_profiles::user::orca_config(
                        b,
                        (&self.printer, user(Kind::Printer, &self.printer)),
                        (&self.process, user(Kind::Print, &self.process)),
                        (f, user(Kind::Filament, f)),
                        Some(bed),
                    )
                }
            }
        };
        let mut c = resolve(&self.filaments[0])?;
        // Orca-only filament options (`pressure_advance`…) are typed vectors too; they are
        // composed per tool like the canonical ones.
        let orca_dialect = match v {
            Vendor::Orca(b) => Some(b.dialect),
            Vendor::Prusa(_) => None,
        };
        for (i, f) in self.filaments.iter().enumerate().skip(1) {
            let tool = resolve(f)?;
            for (key, value) in tool.values() {
                let filament_key = option_defs().get(key).map_or_else(
                    || {
                        orca_dialect
                            .is_some_and(|d| orca::option_scope(key, d) == Some(Scope::Filament))
                    },
                    |d| d.scope == Scope::Filament,
                ) || key == "filament_settings_id";
                if filament_key
                    && let ConfigValue::Vector(source) = value
                    && let Some(value) = source.first()
                {
                    let mut values = match c.get(key) {
                        Some(ConfigValue::Vector(v)) => v.clone(),
                        _ => continue,
                    };
                    if let Some(entry) = values.get_mut(i) {
                        *entry = value.clone();
                    }
                    c.set(key, ConfigValue::Vector(values));
                }
            }
            // Orca filament options Mica keeps as text (`nozzle_temperature_range_low/high`,
            // …) are per tool too: without this every tool carried the first filament's, and
            // the validator judged a PLA tool against a PETG range.
            // The bundle's own schema (`discovery::load_installed` detects Snapmaker Orca's
            // flow-variant set), as the CLI composes it.
            if let Some(dialect) = orca_dialect {
                for (key, raw) in tool.extras() {
                    let numeric = orca::option_type(key, dialect).is_some_and(|t| {
                        matches!(
                            t,
                            "Ints" | "Floats" | "Percents" | "Bools" | "FloatsOrPercents"
                        )
                    });
                    if !numeric || orca::option_scope(key, dialect) != Some(Scope::Filament) {
                        continue;
                    }
                    let Some(first) = raw.split(',').next() else {
                        continue;
                    };
                    let Some(current) = c.extra(key) else {
                        continue;
                    };
                    let mut values: Vec<String> =
                        current.split(',').map(|s| s.to_string()).collect();
                    if let Some(entry) = values.get_mut(i) {
                        *entry = first.to_string();
                        let joined = values.join(",");
                        c.set_raw(key, &joined).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        // Preset bookkeeping is kept as raw text; name every tool's filament for the summary.
        c.set_raw(
            "filament_settings_id",
            &mica_core::config::escape_strings(&self.filaments),
        )
        .map_err(|e| e.to_string())?;
        Ok(c)
    }
}

/// Where a preset comes from: the vendor bundle, Mica (made by the user), or another slicer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    System,
    Mine,
    Imported(mica_profiles::import::Source),
}

fn ensure(value: &mut String, options: &[String]) {
    if !options.contains(value) {
        *value = options.first().cloned().unwrap_or_default();
    }
}

pub fn parse_colour(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if hex.len() < 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_store(name: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("mica-gui-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open(dir)
    }
    fn profiles(bundle: &str, store: Store) -> Profiles {
        let mut b = Bundle::parse(bundle);
        let _ = store.register_all(&mut b);
        let mut p = Profiles {
            vendors: vec![("Prusa".into(), Vendor::Prusa(b))],
            vendor: 0,
            printer: "P".into(),
            process: "Q".into(),
            filaments: vec!["A".into(), "B".into()],
            bed: String::new(),
            printers: vec![],
            processes: vec![],
            materials: vec![],
            warnings: vec![],
            orca_user: vec![],
            user: store.list_all().unwrap_or_default(),
            import_dirs: vec![],
            import_home: None,
            show_incompatible: false,
            incompatible: BTreeSet::new(),
            orca_root: None,
            store,
        };
        p.refresh();
        p
    }
    const BUNDLE: &str = "[vendor]\nname = TestVendor\nconfig_version = 1.0.0\n[printer:P]\nnozzle_diameter = 0.4,0.4\nprinter_model = X\n[printer:Single]\nnozzle_diameter = 0.4\nprinter_model = Y\n[print:Q]\nlayer_height = 0.2\n[print:Only Y]\nlayer_height = 0.3\ncompatible_printers_condition = printer_model==\"Y\"\n[filament:A]\ntemperature = 210\nfilament_type = PLA\nfilament_colour = #FF8000\n[filament:B]\ntemperature = 240\nfilament_type = PETG\ncompatible_printers_condition = printer_model==\"X\"\n";

    #[test]
    fn selected_filaments_are_composed_per_tool() {
        let profiles = profiles(BUNDLE, test_store("compose"));
        let c = profiles.config().unwrap();
        assert_eq!(c.vector_len("temperature"), 2);
        assert_eq!(
            mica_core::print_job::tool_config(&c, 0).float_at("temperature", 0),
            Some(210.)
        );
        assert_eq!(
            mica_core::print_job::tool_config(&c, 1).float_at("temperature", 0),
            Some(240.)
        );
        assert_eq!(
            mica_core::config::unescape_strings(&c.raw("filament_settings_id").unwrap()).unwrap(),
            ["A", "B"]
        );
        assert_eq!(profiles.filament_colour("A"), Some([255, 128, 0]));
        assert_eq!(profiles.filament_colour("B"), None);
    }

    #[test]
    fn compatibility_filters_follow_the_printer() {
        let mut p = profiles(BUNDLE, test_store("compat"));
        assert_eq!(p.processes, ["Q"]);
        assert_eq!(p.materials, ["A", "B"]);
        p.select_printer(0, "Single");
        assert_eq!(p.processes, ["Only Y", "Q"]);
        assert_eq!(p.materials, ["A"]);
        assert_eq!(p.filaments, ["A"]);
        assert_eq!(p.nozzle_count(), 1);
    }

    #[test]
    fn user_profiles_save_register_and_remember() {
        let mut p = profiles(BUNDLE, test_store("save"));
        let settings = BTreeMap::from([("temperature".to_string(), "205".to_string())]);
        let saved = p
            .save_user_profile(PresetKind::Filament, "A tweaked", "A", &settings)
            .unwrap();
        assert_eq!(saved.inherits, "A");
        assert_eq!(saved.inherits_source, "TestVendor");
        p.refresh();
        assert!(p.materials.contains(&"A tweaked".to_string()));
        assert!(p.is_user_preset(PresetKind::Filament, "A tweaked"));
        assert_eq!(p.system_parent(PresetKind::Filament, "A tweaked"), "A");
        // Saving a tweak of the tweak inherits from the system parent and keeps old overrides.
        let more = BTreeMap::from([("filament_type".to_string(), "PLA+".to_string())]);
        let again = p
            .save_user_profile(PresetKind::Filament, "A tweaked", "A tweaked", &more)
            .unwrap();
        assert_eq!(again.inherits, "A");
        assert_eq!(again.settings.len(), 2);
        p.filaments = vec!["A tweaked".into(), "A tweaked".into()];
        let c = p.config().unwrap();
        assert_eq!(c.float("temperature"), Some(205.));
        let mut prefs = Prefs::default();
        p.store_selection(&mut prefs);
        assert_eq!(prefs.selection.filaments, ["A tweaked", "A tweaked"]);
        let mut fresh = profiles(BUNDLE, Store::open(p.store.root.clone()));
        fresh.restore(&prefs);
        assert_eq!(fresh.filaments, ["A tweaked", "A tweaked"]);
        let _ = std::fs::remove_dir_all(&p.store.root);
    }

    #[test]
    fn my_printers_only_lists_existing_presets() {
        let p = profiles(BUNDLE, test_store("mine"));
        let mut prefs = Prefs::default();
        prefs.add_printer("Prusa", "P");
        prefs.add_printer("Prusa", "Gone");
        prefs.add_printer("Bambu", "P");
        assert_eq!(p.my_printers(&prefs), vec![(0, "P".to_string())]);
        assert_eq!(p.all_printers().len(), 2);
    }
}
