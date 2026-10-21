// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Importing the user's presets from OrcaSlicer, Snapmaker Orca and PrusaSlicer, on request,
//! and cleaning them up again: the review list (what was found, what already exists, what
//! suits the user's printers), importing a selection, removing imported profiles, merging
//! duplicates and re-importing presets changed in the other slicer. The other slicers' files
//! are only ever read. The windows are in `panels/import.rs`.

use crate::panels::profiles::{Profiles, Vendor};
use crate::state::Prefs;
use mica_profiles::import::{self, Found, ORCA_LIBRARY, Source, Status};
use mica_profiles::orca;
use mica_profiles::prusa::Kind;
use mica_profiles::user::{self, UserProfile, orca_kind};
use std::collections::BTreeMap;

/// One found preset in the review list.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub found: Found,
    pub status: Status,
    /// The Mica vendor it builds on (`Prusa`, `Snapmaker`, …), when Mica has its parent.
    pub vendor: Option<String>,
    /// Why it cannot be imported.
    pub problem: Option<String>,
    /// It suits a printer in My printers (or is one): checked by default.
    pub for_my_printers: bool,
    /// The same settings as another preset in the list or a Mica profile (named here): left
    /// unchecked, so the same preset isn't imported twice.
    pub duplicate_of: Option<String>,
    pub selected: bool,
    /// Changed since it was imported, in the other slicer and in Mica: the settings both
    /// changed. Re-importing keeps Mica's values unless `take_theirs`.
    pub conflicts: Vec<String>,
    /// For the settings in `conflicts`: use the other slicer's values ("Take theirs").
    pub take_theirs: bool,
}

impl Candidate {
    pub fn importable(&self) -> bool {
        self.problem.is_none() && !matches!(self.status, Status::Imported(_))
    }

    /// Checked by default (and by "For my printers"): new, for My printers, not a duplicate.
    pub fn default_pick(&self) -> bool {
        self.importable()
            && self.status == Status::New
            && self.for_my_printers
            && self.duplicate_of.is_none()
    }
}

/// An imported profile whose source file changed (or disappeared) since.
#[derive(Clone, Debug, PartialEq)]
pub struct Stale {
    pub kind: Kind,
    pub name: String,
    pub source: Source,
    /// `false`: the file is gone from the other slicer.
    pub exists: bool,
    /// Settings changed in the other slicer and in Mica since the import.
    pub conflicts: Vec<String>,
}

/// What an import did.
#[derive(Debug, Default)]
pub struct Imported {
    pub names: Vec<(Kind, String)>,
    pub errors: Vec<String>,
}

impl Profiles {
    /// Every preset in the other slicers' folders (and the files that could not be read).
    pub fn scan_imports(&self) -> (Vec<Found>, Vec<String>) {
        import::scan(&self.import_dirs)
    }

    /// The presets a startup notice offers: not imported yet, not asked about before, from a
    /// slicer the user still wants to be asked about.
    pub fn new_imports(&self, prefs: &Prefs) -> Vec<Found> {
        let (found, _) = self.scan_imports();
        found
            .into_iter()
            .filter(|f| {
                prefs.import.asks(f.source)
                    && !prefs.import.was_seen(f)
                    && import::status(f, &self.user) == Status::New
            })
            .collect()
    }

    /// Every preset in the other slicers' folders that is not in Mica yet, whether or not the
    /// startup notice asked about it (the Welcome window's Import card).
    pub fn importable(&self) -> Vec<Found> {
        let (found, _) = self.scan_imports();
        found
            .into_iter()
            .filter(|f| import::status(f, &self.user) == Status::New)
            .collect()
    }

    /// The Mica vendor key of a user profile (`Prusa` for PrusaSlicer-based ones).
    pub fn profile_vendor(&self, p: &UserProfile) -> String {
        let prusa_name = self
            .vendor_index("Prusa")
            .and_then(|vi| match &self.vendors[vi].1 {
                Vendor::Prusa(b) => b.vendor.get("name").cloned(),
                Vendor::Orca(_) => None,
            })
            .unwrap_or_else(|| "PrusaResearch".into());
        if p.orca.is_none() && (p.inherits_source.is_empty() || p.inherits_source == prusa_name) {
            "Prusa".into()
        } else if self
            .orca_bundles()
            .any(|(_, b)| user::is_library_filament(p, b))
        {
            ORCA_LIBRARY.into()
        } else {
            p.inherits_source.clone()
        }
    }

    /// The loaded Orca vendors: `(vendor index, bundle)`.
    pub(crate) fn orca_bundles(&self) -> impl Iterator<Item = (usize, &orca::Bundle)> {
        self.vendors
            .iter()
            .enumerate()
            .filter_map(|(vi, (_, v))| match v {
                Vendor::Orca(b) => Some((vi, b)),
                Vendor::Prusa(_) => None,
            })
    }

    /// The bundle a profile of `vendor` (a vendor key, or [`ORCA_LIBRARY`]) resolves in: the
    /// vendor's own, or for the shared filament library the first loaded Orca vendor's.
    fn bundle_for(&self, vendor: &str) -> Option<usize> {
        if vendor == ORCA_LIBRARY {
            self.orca_bundles()
                .find(|(_, b)| b.vendors.contains_key(ORCA_LIBRARY))
                .map(|(vi, _)| vi)
        } else {
            self.vendor_index(vendor)
        }
    }

    /// Whether `vendor` (a profile's, see [`Self::profile_vendor`]) is that of the printers of
    /// vendor key `key`: the same, or the shared filament library on any Orca vendor.
    pub(crate) fn vendor_covers(&self, vendor: &str, key: &str) -> bool {
        vendor == key
            || (vendor == ORCA_LIBRARY
                && self
                    .vendor_index(key)
                    .is_some_and(|vi| matches!(self.vendors[vi].1, Vendor::Orca(_))))
    }

    /// The Orca vendor a found Orca preset builds on: one already loaded that has its parent
    /// (Snapmaker first for Snapmaker Orca, then the vendors of My printers), else the vendor
    /// whose index lists the parent (loaded now).
    fn orca_vendor_for(
        &mut self,
        f: &Found,
        prefs: &Prefs,
        index: &mut Option<BTreeMap<(orca::Kind, String), Vec<String>>>,
    ) -> Result<String, String> {
        let kind = orca_kind(f.kind);
        if f.inherits.is_empty() {
            return if f.source == Source::SnapmakerOrca && self.vendor_index("Snapmaker").is_some()
            {
                Ok("Snapmaker".into())
            } else {
                Err(
                    "It doesn't build on a system profile, so Mica can't tell which printer \
                     it is for."
                        .into(),
                )
            };
        }
        let library = |vendor: &str| f.kind == Kind::Filament && vendor == ORCA_LIBRARY;
        // A filament built directly on Orca's shared library suits every Orca vendor.
        if self.orca_bundles().any(|(_, b)| {
            b.preset(kind, &f.inherits)
                .is_some_and(|p| !p.is_user && library(&p.vendor))
        }) {
            return Ok(ORCA_LIBRARY.into());
        }
        let mut keys: Vec<String> = self
            .vendors
            .iter()
            .filter_map(|(key, v)| match v {
                Vendor::Orca(b) => b
                    .preset(kind, &f.inherits)
                    .is_some_and(|p| !p.is_user)
                    .then(|| key.clone()),
                Vendor::Prusa(_) => None,
            })
            .collect();
        if keys.is_empty()
            && let Some(root) = self.orca_root.clone()
        {
            let index = index.get_or_insert_with(|| orca::discovery::preset_vendors(&root));
            let listed = index.get(&(kind, f.inherits.clone()));
            if listed.into_iter().flatten().any(|v| library(v)) {
                if self.bundle_for(ORCA_LIBRARY).is_none() {
                    self.ensure_vendor("Snapmaker")?;
                }
                return Ok(ORCA_LIBRARY.into());
            }
            keys = listed
                .into_iter()
                .flatten()
                .filter(|v| *v != ORCA_LIBRARY)
                .cloned()
                .collect();
        }
        let mine: Vec<&str> = prefs.printers().map(|(v, _)| v).collect();
        keys.sort_by_key(|k| {
            (
                !(f.source == Source::SnapmakerOrca && k == "Snapmaker"),
                !mine.contains(&k.as_str()),
                k.clone(),
            )
        });
        let Some(key) = keys.into_iter().next() else {
            return Err(format!(
                "Its parent profile “{}” isn't among Mica's {} profiles.",
                f.inherits,
                Source::OrcaSlicer.app_name()
            ));
        };
        self.ensure_vendor(&key)?;
        Ok(key)
    }

    /// Whether a found preset suits one of My printers (or is one): the parent's
    /// compatibility, or the preset's own printer list when it has one.
    fn suits_my_printers(&self, f: &Found, vendor: &str, prefs: &Prefs) -> bool {
        if vendor == ORCA_LIBRARY {
            let keys: Vec<&str> = prefs
                .printers()
                .map(|(v, _)| v)
                .filter(|v| self.vendor_covers(vendor, v))
                .collect();
            return keys
                .into_iter()
                .any(|k| self.suits_printers_of(f, k, prefs));
        }
        self.suits_printers_of(f, vendor, prefs)
    }

    /// [`Self::suits_my_printers`] for My printers of vendor key `vendor`.
    fn suits_printers_of(&self, f: &Found, vendor: &str, prefs: &Prefs) -> bool {
        let Some(vi) = self.vendor_index(vendor) else {
            return false;
        };
        let printers: Vec<&str> = prefs
            .printers()
            .filter(|(v, _)| *v == vendor)
            .map(|(_, n)| n)
            .collect();
        if f.kind == Kind::Printer {
            return printers.iter().any(|p| *p == f.inherits || *p == f.name);
        }
        if f.inherits.is_empty() {
            return false;
        }
        match &self.vendors[vi].1 {
            Vendor::Prusa(b) => printers.iter().any(|p| {
                b.is_compatible_with_printer(f.kind, &f.inherits, p)
                    .unwrap_or(false)
            }),
            Vendor::Orca(b) => {
                let own: Option<Vec<String>> = std::fs::read_to_string(&f.path)
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .and_then(|j| {
                        let list = j.get("compatible_printers")?.as_array()?;
                        let list: Vec<String> = list
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect();
                        (!list.is_empty()).then_some(list)
                    });
                printers.iter().any(|p| match &own {
                    Some(list) => list.iter().any(|n| n == p),
                    None => b
                        .is_compatible_with_printer(orca_kind(f.kind), &f.inherits, p)
                        .unwrap_or(false),
                })
            }
        }
    }

    /// The review list: every found preset with its status, the vendor it builds on and
    /// whether it suits My printers. New presets for My printers start checked. May load the
    /// Orca vendors the presets build on.
    pub fn import_candidates(&mut self, prefs: &Prefs) -> (Vec<Candidate>, Vec<String>) {
        let (found, errors) = self.scan_imports();
        let mut index = None;
        let mut out: Vec<Candidate> = Vec::new();
        // Each Orca candidate's content, to spot the same preset in two places.
        let mut contents: Vec<Option<UserProfile>> = Vec::new();
        for f in found {
            let status = import::status(&f, &self.user);
            let vendor = if f.source.is_orca() {
                self.orca_vendor_for(&f, prefs, &mut index)
            } else {
                self.prusa_parent_check(&f).map(|()| "Prusa".to_string())
            };
            let (vendor, problem) = match vendor {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e)),
            };
            let for_my_printers = vendor
                .as_deref()
                .is_some_and(|v| self.suits_my_printers(&f, v, prefs));
            let content = match (&vendor, f.source.is_orca()) {
                (Some(v), true) => import::orca_profile(&f, v, "", &f.name, 0).ok(),
                _ => None,
            };
            let duplicate_of = content.as_ref().and_then(|c| {
                out.iter()
                    .zip(&contents)
                    .find(|(_, o)| o.as_ref().is_some_and(|o| o.same_content(c)))
                    .map(|(o, _)| {
                        let name = crate::preset_names::display_name(&o.found.name);
                        format!("{}'s “{name}”", o.found.source.app_name())
                    })
                    .or_else(|| {
                        (status == Status::New)
                            .then(|| self.user.iter().find(|u| u.same_content(c)))
                            .flatten()
                            .map(|u| {
                                format!("Mica's “{}”", crate::preset_names::display_name(&u.name))
                            })
                    })
            });
            let conflicts = match (&status, &vendor) {
                (Status::Changed(_), Some(v)) => self.reimport_conflicts(&f, v),
                _ => vec![],
            };
            let mut c = Candidate {
                found: f,
                status,
                vendor,
                problem,
                for_my_printers,
                duplicate_of,
                selected: false,
                conflicts,
                take_theirs: false,
            };
            c.selected = c.default_pick();
            out.push(c);
            contents.push(content);
        }
        (out, errors)
    }

    /// A fresh Mica copy of a found preset named `name`, built on `vendor`, as an import makes
    /// it before Mica's own changes go on top.
    fn fresh_copy(
        &self,
        f: &Found,
        vendor: &str,
        name: &str,
        date: u64,
    ) -> Result<UserProfile, String> {
        match self.bundle_for(vendor).map(|vi| &self.vendors[vi].1) {
            Some(Vendor::Prusa(b)) => {
                import::prusa_profile(f, b, name, date).map_err(|e| e.to_string())
            }
            Some(Vendor::Orca(b)) => {
                let version = b
                    .vendors
                    .get(vendor)
                    .and_then(|v| v.get("version"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                import::orca_profile(f, vendor, &version, name, date)
            }
            None => Err(format!("No {vendor} profiles are loaded.")),
        }
    }

    /// The settings changed both in the other slicer and in Mica since `f` was imported.
    fn reimport_conflicts(&self, f: &Found, vendor: &str) -> Vec<String> {
        let Some(old) = import::imported_copy(f, &self.user) else {
            return vec![];
        };
        self.fresh_copy(f, vendor, &old.name, 0)
            .map(|new| import::conflicts(old, &new))
            .unwrap_or_default()
    }

    fn prusa_parent_check(&self, f: &Found) -> Result<(), String> {
        let Some(Vendor::Prusa(b)) = self.vendor_index("Prusa").map(|vi| &self.vendors[vi].1)
        else {
            return Err("Mica has no PrusaSlicer profiles to build it on.".into());
        };
        if !f.inherits.is_empty() && b.flatten(f.kind, &f.inherits).is_err() {
            return Err(format!(
                "Its parent profile “{}” isn't among Mica's PrusaSlicer profiles.",
                f.inherits
            ));
        }
        Ok(())
    }

    /// Whether a vendor bundle has a system preset of this name (an imported one must not
    /// take it).
    fn system_name_taken(&self, vendor: &str, kind: Kind, name: &str) -> bool {
        match self.bundle_for(vendor).map(|vi| &self.vendors[vi].1) {
            Some(Vendor::Prusa(b)) => {
                b.preset_names(kind).contains(&name)
                    && !self.user.iter().any(|p| p.kind == kind && p.name == name)
            }
            Some(Vendor::Orca(b)) => b.preset(orca_kind(kind), name).is_some_and(|p| !p.is_user),
            None => false,
        }
    }

    /// Imports (or re-imports changed) candidates into Mica's store and registers them, so
    /// they can be selected at once. Imported printers join My printers. Importing a file again
    /// updates Mica's copy; Mica's own changes are kept on top (for the settings the other
    /// slicer changed too, its values with `Candidate::take_theirs`). Printers in My printers
    /// without a visible-filament list get the other slicer's.
    pub fn import(&mut self, candidates: &[&Candidate], prefs: &mut Prefs) -> Imported {
        let mut done = Imported::default();
        let date = import::now();
        for c in candidates {
            // An unchanged earlier import is already there: nothing to do.
            let (Some(vendor), None, false) = (
                &c.vendor,
                &c.problem,
                matches!(c.status, Status::Imported(_)),
            ) else {
                continue;
            };
            let f = &c.found;
            let existing = self.user.clone();
            let name =
                import::target_name(f, &existing, |n| self.system_name_taken(vendor, f.kind, n));
            let earlier = import::imported_copy(f, &existing).cloned();
            let built = self.fresh_copy(f, vendor, &name, date).map(|mut p| {
                if let Some(old) = &earlier {
                    import::carry_edits(old, &mut p, c.take_theirs);
                }
                p
            });
            let profile = match built {
                Ok(p) => p,
                Err(e) => {
                    done.errors.push(format!("{}: {e}", f.name));
                    continue;
                }
            };
            if let Err(e) = self.register(&profile) {
                done.errors.push(format!("{}: {e}", f.name));
                continue;
            }
            if let Err(e) = import::save(&self.store, profile.clone(), &mut self.user) {
                self.unregister(&profile);
                done.errors.push(e.to_string());
                continue;
            }
            if profile.kind == Kind::Printer {
                prefs.add_printer(vendor, &profile.name);
            }
            prefs.import.mark_seen([f]);
            done.names.push((profile.kind, profile.name));
        }
        self.refresh();
        self.seed_visible_filaments(prefs);
        done
    }

    /// Registers a stored profile in its vendor's bundle (replacing an earlier version).
    fn register(&mut self, p: &UserProfile) -> Result<(), String> {
        let vendor = self.profile_vendor(p);
        if vendor == ORCA_LIBRARY {
            // Every loaded Orca vendor lists it.
            let all: Vec<usize> = self.orca_bundles().map(|(vi, _)| vi).collect();
            for vi in all {
                if let Vendor::Orca(b) = &mut self.vendors[vi].1
                    && let Some(e) = user::register_orca(b, [p]).pop()
                {
                    return Err(e);
                }
            }
            self.orca_user
                .retain(|o| !(o.kind == p.kind && o.name == p.name));
            self.orca_user.push(p.clone());
            return Ok(());
        }
        let Some(vi) = self.vendor_index(&vendor) else {
            return Ok(());
        };
        match &mut self.vendors[vi].1 {
            Vendor::Prusa(b) => {
                b.add_user_preset(p.kind, &p.name, &p.inherits, &p.settings);
            }
            Vendor::Orca(b) => {
                if let Some(e) = mica_profiles::user::register_orca(b, [p]).pop() {
                    return Err(e);
                }
                self.orca_user
                    .retain(|o| !(o.kind == p.kind && o.name == p.name));
                self.orca_user.push(p.clone());
            }
        }
        Ok(())
    }

    fn unregister(&mut self, p: &UserProfile) {
        let vendor = self.profile_vendor(p);
        if vendor == ORCA_LIBRARY {
            for (_, v) in &mut self.vendors {
                if let Vendor::Orca(b) = v {
                    b.remove_user_preset(orca_kind(p.kind), &p.name);
                }
            }
        } else if let Some(vi) = self.vendor_index(&vendor) {
            match &mut self.vendors[vi].1 {
                Vendor::Prusa(b) => b.remove_user_preset(p.kind, &p.name),
                Vendor::Orca(b) => b.remove_user_preset(orca_kind(p.kind), &p.name),
            }
        }
        self.orca_user
            .retain(|o| !(o.kind == p.kind && o.name == p.name));
    }

    /// The imported profiles.
    pub fn imported(&self) -> Vec<&UserProfile> {
        self.user.iter().filter(|p| p.source().is_some()).collect()
    }

    /// Whether the preferences or the current selection name a profile.
    pub fn in_use(&self, p: &UserProfile, prefs: &Prefs) -> bool {
        let vendor = self.profile_vendor(p);
        let n = p.name.as_str();
        let selected = |sel_vendor: &str, printer: &str, print: &str, filaments: &[String]| {
            self.vendor_covers(&vendor, sel_vendor)
                && match p.kind {
                    Kind::Printer => printer == n,
                    Kind::Print => print == n,
                    Kind::Filament => filaments.iter().any(|f| f == n),
                }
        };
        selected(
            self.vendor_name(),
            &self.printer,
            &self.process,
            &self.filaments,
        ) || selected(
            &prefs.selection.vendor,
            &prefs.selection.printer,
            &prefs.selection.print,
            &prefs.selection.filaments,
        ) || match p.kind {
            Kind::Printer => {
                prefs.has_printer(&vendor, n)
                    || prefs.defaults.printer == crate::state::printer_id(&vendor, n)
            }
            Kind::Print => prefs.defaults.print == n,
            Kind::Filament => prefs.defaults.filament == n,
        }
    }

    /// Imported profiles nothing uses (not selected, not in My printers, not a default).
    pub fn unused_imported(&self, prefs: &Prefs) -> Vec<(Kind, String)> {
        self.imported()
            .into_iter()
            .filter(|p| !self.in_use(p, prefs))
            .map(|p| (p.kind, p.name.clone()))
            .collect()
    }

    /// Moves profiles to Mica's trash folder (never touching the other slicers' files) and
    /// forgets them: My printers and the selection let go of them.
    pub fn remove_profiles(&mut self, which: &[(Kind, String)], prefs: &mut Prefs) -> Imported {
        let mut done = Imported::default();
        for (kind, name) in which {
            let Some(p) = self
                .user
                .iter()
                .find(|p| p.kind == *kind && p.name == *name)
                .cloned()
            else {
                continue;
            };
            if let Err(e) = self.store.delete(*kind, name) {
                done.errors.push(e.to_string());
                continue;
            }
            self.unregister(&p);
            self.user.retain(|u| !(u.kind == *kind && u.name == *name));
            if *kind == Kind::Printer {
                let vendor = self.profile_vendor(&p);
                prefs.remove_printer(&vendor, name);
            }
            done.names.push((*kind, name.clone()));
        }
        self.after_removal(prefs);
        done
    }

    /// After profiles went away: a removed printer gives way to the first of My printers, a
    /// removed print or filament to a compatible one.
    fn after_removal(&mut self, prefs: &Prefs) {
        if !self.printer_exists(self.vendor, &self.printer)
            && let Some((vi, name)) = self.my_printers(prefs).into_iter().next()
        {
            self.select_printer(vi, &name);
        }
        self.refresh();
    }

    /// Groups of profiles with identical settings (at least one of them imported).
    pub fn duplicate_groups(&self) -> Vec<Vec<(Kind, String)>> {
        import::duplicates(&self.user)
            .into_iter()
            .map(|g| {
                g.into_iter()
                    .map(|i| (self.user[i].kind, self.user[i].name.clone()))
                    .collect()
            })
            .collect()
    }

    /// Keeps one profile of a duplicate group (the one in use, else Mica's own, else the
    /// first) and removes the others; whatever named them names the kept one.
    pub fn merge_duplicates(&mut self, group: &[(Kind, String)], prefs: &mut Prefs) -> Imported {
        let profiles: Vec<UserProfile> = group
            .iter()
            .filter_map(|(k, n)| self.user.iter().find(|p| p.kind == *k && p.name == *n))
            .cloned()
            .collect();
        let Some(keep) = profiles
            .iter()
            .find(|p| self.in_use(p, prefs))
            .or_else(|| profiles.iter().find(|p| p.source().is_none()))
            .or(profiles.first())
            .cloned()
        else {
            return Imported::default();
        };
        let vendor = self.profile_vendor(&keep);
        let others: Vec<(Kind, String)> = profiles
            .iter()
            .filter(|p| p.name != keep.name)
            .map(|p| (p.kind, p.name.clone()))
            .collect();
        for (kind, name) in &others {
            match kind {
                Kind::Printer => {
                    if prefs.has_printer(&vendor, name) {
                        prefs.remove_printer(&vendor, name);
                        prefs.add_printer(&vendor, &keep.name);
                    }
                    if prefs.selection.printer == *name {
                        prefs.selection.printer = keep.name.clone();
                    }
                    if self.printer == *name {
                        self.printer = keep.name.clone();
                    }
                }
                Kind::Print => {
                    if prefs.selection.print == *name {
                        prefs.selection.print = keep.name.clone();
                    }
                    if self.process == *name {
                        self.process = keep.name.clone();
                    }
                }
                Kind::Filament => {
                    for f in prefs
                        .selection
                        .filaments
                        .iter_mut()
                        .chain(self.filaments.iter_mut())
                    {
                        if f == name {
                            *f = keep.name.clone();
                        }
                    }
                }
            }
        }
        self.remove_profiles(&others, prefs)
    }

    /// Imported profiles whose file changed in the other slicer since (or is gone).
    pub fn stale_imports(&self) -> Vec<Stale> {
        self.imported()
            .into_iter()
            .filter_map(|p| {
                let info = p.import.as_ref()?;
                let exists = info.path.is_file();
                let changed = match std::fs::read(&info.path) {
                    Ok(bytes) => import::hash(&bytes) != info.hash,
                    Err(_) => true,
                };
                changed.then(|| Stale {
                    kind: p.kind,
                    name: p.name.clone(),
                    source: info.source,
                    exists,
                    conflicts: if exists {
                        self.stale_conflicts(p)
                    } else {
                        vec![]
                    },
                })
            })
            .collect()
    }

    /// The settings an imported profile's re-import would ask about (see `Candidate`).
    fn stale_conflicts(&self, p: &UserProfile) -> Vec<String> {
        let Some(info) = &p.import else {
            return vec![];
        };
        let Ok(bytes) = std::fs::read(&info.path) else {
            return vec![];
        };
        let f = Found {
            source: info.source,
            kind: p.kind,
            path: info.path.clone(),
            name: info.original_name.clone(),
            inherits: p.inherits.clone(),
            hash: import::hash(&bytes),
        };
        let vendor = self.profile_vendor(p);
        self.fresh_copy(&f, &vendor, &p.name, 0)
            .map(|new| import::conflicts(p, &new))
            .unwrap_or_default()
    }

    /// Re-imports the named imported profiles from their (changed) files, keeping Mica's own
    /// changes; with `take_theirs`, the settings changed on both sides take the other slicer's
    /// values.
    pub fn reimport(
        &mut self,
        which: &[(Kind, String)],
        take_theirs: bool,
        prefs: &mut Prefs,
    ) -> Imported {
        let (mut candidates, _) = self.import_candidates(prefs);
        for c in &mut candidates {
            c.take_theirs = take_theirs;
        }
        let chosen: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| match &c.status {
                Status::Changed(n) | Status::Imported(n) => {
                    which.iter().any(|(k, w)| *k == c.found.kind && w == n)
                }
                Status::New => false,
            })
            .collect();
        self.import(&chosen, prefs)
    }

    /// For a Mica that loaded OrcaSlicer's and Snapmaker Orca's user presets directly (before
    /// imports were asked for): the ones the preferences still name are imported, so the
    /// selection and My printers keep working. Returns what was imported.
    pub fn import_in_use(&mut self, prefs: &mut Prefs) -> Imported {
        let referenced = |f: &Found, prefs: &Prefs| {
            let n = f.name.as_str();
            prefs.printers().any(|(_, p)| p == n)
                || match f.kind {
                    Kind::Printer => prefs.selection.printer == n,
                    Kind::Print => prefs.selection.print == n,
                    Kind::Filament => prefs.selection.filaments.iter().any(|x| x == n),
                }
        };
        let (found, _) = self.scan_imports();
        if !found
            .iter()
            .any(|f| f.source.is_orca() && referenced(f, prefs))
        {
            return Imported::default();
        }
        let (candidates, _) = self.import_candidates(prefs);
        let chosen: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| {
                c.found.source.is_orca()
                    && c.status == Status::New
                    && c.found.name == import::target_name(&c.found, &self.user, |_| false)
                    && referenced(&c.found, prefs)
            })
            .collect();
        self.import(&chosen, prefs)
    }
}

/// Visible filaments: which system filaments the filament picker lists per printer (the
/// user's own and imported ones are always listed, as in OrcaSlicer and PrusaSlicer). Only
/// the picker reads it; the selection and the G-code never depend on it.
impl Profiles {
    /// The system filaments of vendor `vi` made for its printer `printer`, with the names
    /// they were renamed from.
    fn system_filaments_for(&self, vi: usize, printer: &str) -> Vec<(String, Vec<String>)> {
        let is_user = |n: &str| {
            self.user
                .iter()
                .any(|p| p.kind == Kind::Filament && p.name == n)
        };
        match &self.vendors[vi].1 {
            Vendor::Prusa(b) => b
                .compatible_presets(Kind::Filament, printer, None)
                .into_iter()
                .filter(|n| !is_user(n))
                .map(|n| {
                    let renamed = b
                        .flatten(Kind::Filament, n)
                        .ok()
                        .and_then(|f| f.get("renamed_from").cloned())
                        .map(|r| mica_profiles::visible::renamed_from(&r))
                        .unwrap_or_default();
                    (n.to_string(), renamed)
                })
                .collect(),
            Vendor::Orca(b) => {
                let base = self
                    .orca_user
                    .iter()
                    .find(|p| p.kind == Kind::Printer && p.name == printer)
                    .map_or(printer, |p| p.orca_base());
                b.compatible_presets(orca::Kind::Filament, base, None)
                    .into_iter()
                    .filter_map(|n| {
                        let p = b.preset(orca::Kind::Filament, n)?;
                        (!p.is_user && !is_user(n)).then(|| {
                            let renamed = p
                                .metadata
                                .get("renamed_from")
                                .and_then(|v| v.as_str())
                                .map(mica_profiles::visible::renamed_from)
                                .unwrap_or_default();
                            (n.to_string(), renamed)
                        })
                    })
                    .collect()
            }
        }
    }

    /// The other slicer's visible-filament list for printers of vendor `vi`: PrusaSlicer's for
    /// the PrusaSlicer bundle; for Orca vendors Snapmaker Orca's first for Snapmaker, else
    /// OrcaSlicer's first.
    fn other_slicer_visible(&self, vi: usize) -> Option<std::collections::BTreeSet<String>> {
        let (home, appdata) = self.import_home.as_ref()?;
        let order: &[Source] = match &self.vendors[vi] {
            (_, Vendor::Prusa(_)) => &[Source::PrusaSlicer],
            (key, Vendor::Orca(_)) if key == "Snapmaker" => {
                &[Source::SnapmakerOrca, Source::OrcaSlicer]
            }
            (_, Vendor::Orca(_)) => &[Source::OrcaSlicer, Source::SnapmakerOrca],
        };
        order
            .iter()
            .find_map(|s| mica_profiles::visible::read(*s, home, appdata.as_deref()))
    }

    /// Gives every printer in My printers without a visible-filament list the list its own
    /// slicer keeps (OrcaSlicer's selected filaments, PrusaSlicer's installed ones), limited to
    /// the filaments made for it. Nothing to seed from leaves the printer listing everything.
    /// Returns the printers seeded.
    pub fn seed_visible_filaments(&self, prefs: &mut Prefs) -> Vec<String> {
        let mut seeded = Vec::new();
        let printers: Vec<(String, String)> = prefs
            .printers()
            .map(|(v, n)| (v.to_string(), n.to_string()))
            .collect();
        for (vendor, printer) in printers {
            let id = crate::state::printer_id(&vendor, &printer);
            if prefs.visible_filaments.contains_key(&id) {
                continue;
            }
            let Some(vi) = self.vendor_index(&vendor) else {
                continue;
            };
            if !self.printer_exists(vi, &printer) {
                continue;
            }
            let Some(list) = self.other_slicer_visible(vi) else {
                continue;
            };
            let mut names: Vec<String> = self
                .system_filaments_for(vi, &printer)
                .into_iter()
                .filter(|(n, renamed)| mica_profiles::visible::listed(&list, n, renamed))
                .map(|(n, _)| n)
                .collect();
            if names.is_empty() {
                continue;
            }
            names.sort();
            names.dedup();
            prefs.visible_filaments.insert(id.clone(), names);
            seeded.push(id);
        }
        seeded
    }

    /// The selected printer's visible-filament list, if it has one.
    pub fn visible_filaments<'a>(&self, prefs: &'a Prefs) -> Option<&'a Vec<String>> {
        prefs
            .visible_filaments
            .get(&crate::state::printer_id(self.vendor_name(), &self.printer))
    }

    /// Whether the filament picker leaves out a listed filament: a system filament the
    /// selected printer's list doesn't name. A selected filament is always listed.
    pub fn filament_hidden(&self, prefs: &Prefs, name: &str) -> bool {
        self.visible_filaments(prefs).is_some_and(|list| {
            !list.iter().any(|n| n == name)
                && !self.filaments.iter().any(|f| f == name)
                // Filaments for other printers follow "Show profiles for other printers".
                && !self.is_incompatible(crate::panels::profiles::PresetKind::Filament, name)
                && self
                    .user_profile(crate::panels::profiles::PresetKind::Filament, name)
                    .is_none()
        })
    }

    /// Makes `name` visible or hidden on the selected printer. A printer without a list starts
    /// from every system filament made for it.
    pub fn set_filament_visible(&self, prefs: &mut Prefs, name: &str, visible: bool) {
        let id = crate::state::printer_id(self.vendor_name(), &self.printer);
        let list = prefs.visible_filaments.entry(id).or_insert_with(|| {
            self.materials
                .iter()
                .filter(|m| {
                    !self.is_incompatible(crate::panels::profiles::PresetKind::Filament, m)
                        && self
                            .user_profile(crate::panels::profiles::PresetKind::Filament, m)
                            .is_none()
                })
                .cloned()
                .collect()
        });
        list.retain(|n| n != name);
        if visible {
            list.push(name.to_string());
            list.sort();
        }
    }

    /// "Show all": the selected printer lists every filament made for it again.
    pub fn show_all_filaments(&self, prefs: &mut Prefs) {
        prefs
            .visible_filaments
            .remove(&crate::state::printer_id(self.vendor_name(), &self.printer));
    }
}

/// "Found 12 printer, 30 filament and 8 print profiles in OrcaSlicer": what a notice says
/// about `found`, per slicer.
pub fn found_phrase(found: &[Found]) -> String {
    let mut parts = Vec::new();
    for source in Source::ALL {
        let count = |k: Kind| {
            found
                .iter()
                .filter(|f| f.source == source && f.kind == k)
                .count()
        };
        let (printers, filaments, prints) = (
            count(Kind::Printer),
            count(Kind::Filament),
            count(Kind::Print),
        );
        if printers + filaments + prints > 0 {
            parts.push(format!(
                "{} in {}",
                import::count_phrase(printers, filaments, prints),
                source.app_name()
            ));
        }
    }
    match parts.as_slice() {
        [] => String::new(),
        [one] => format!("Found {one}"),
        [rest @ .., last] => format!("Found {} and {last}", rest.join(", ")),
    }
}
