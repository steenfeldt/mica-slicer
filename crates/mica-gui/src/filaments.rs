// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Filaments and tools. A project has filaments (F1…Fn: a preset, edits and a colour each),
//! never fewer than the printer has tools and more when it has more colours. Everything in the
//! scene names filaments: part and object tools, painted areas, mixed colors, layer tool
//! changes, the tool settings. A plate is one print job: it maps the filaments it uses onto
//! the printer's tools ([`assign`]), and the slicer only ever sees tools. At the slice the
//! plate is translated ([`App::plate_tools`], [`to_tools`]): its objects and layer changes
//! from filament numbers to tool numbers, and its configuration takes each tool's filament
//! settings from the filament on that tool. With as many filaments as tools, filament n is
//! tool n and nothing is translated.

use crate::app::App;
use mica_core::config::{Config, ConfigValue, Scope, option_defs};
use mica_core::full_spectrum::FullSpectrum;
use mica_profiles::orca;
use std::collections::{BTreeMap, BTreeSet};

/// Whether `key` holds one value per filament (one per tool in a plate's configuration).
pub fn is_filament_key(key: &str, dialect: Option<orca::Dialect>) -> bool {
    option_defs().get(key).map_or_else(
        || dialect.is_some_and(|d| orca::option_scope(key, d) == Some(Scope::Filament)),
        |d| d.scope == Scope::Filament,
    ) || key == "filament_settings_id"
}

/// Orca filament options Mica keeps as text (`nozzle_temperature_range_low`, …): numeric
/// lists, one entry per filament.
fn is_filament_list(key: &str, dialect: Option<orca::Dialect>) -> bool {
    dialect.is_some_and(|d| {
        orca::option_scope(key, d) == Some(Scope::Filament)
            && orca::option_type(key, d).is_some_and(|t| {
                matches!(
                    t,
                    "Ints" | "Floats" | "Percents" | "Bools" | "FloatsOrPercents"
                )
            })
    })
}

/// Gives every per-filament option of `c` `n` entries; new entries repeat the first.
pub fn resize(c: &mut Config, n: usize, dialect: Option<orca::Dialect>) {
    let keys: Vec<String> = c
        .values()
        .filter(|(k, v)| matches!(v, ConfigValue::Vector(_)) && is_filament_key(k, dialect))
        .map(|(k, _)| k.to_string())
        .collect();
    for k in keys {
        c.resize_vector(&k, n);
    }
    let lists: Vec<(String, String)> = c
        .extras()
        .filter(|(k, _)| is_filament_list(k, dialect))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    for (k, raw) in lists {
        let mut values: Vec<&str> = raw.split(',').collect();
        let first = values[0];
        values.resize(n, first);
        let _ = c.set_raw(&k, &values.join(","));
    }
}

/// Which filament each tool of a plate prints (zero-based), and the plate's renumbering from
/// one-based filament (and mixed color) numbers to tool numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolMap {
    /// `slots[tool]` = the filament on that tool.
    pub slots: Vec<usize>,
    /// One-based filament (or mixed color) -> tool, where they differ.
    pub map: BTreeMap<usize, usize>,
}

impl ToolMap {
    /// Filament n is tool n: nothing to translate.
    pub fn is_identity(&self) -> bool {
        self.map.is_empty() && self.slots.iter().enumerate().all(|(t, &f)| t == f)
    }
}

/// The tools for a plate that uses the filaments `used` (zero-based) out of `filaments`, on a
/// printer with `tools` tools, with the mixed colors `mixes` (their one-based numbers, after
/// the filaments; on the plate they follow the tools): a used filament
/// stays on its own tool when the printer has it, the others take the free tools in order,
/// and the tools left over keep their own filament where they can (their settings still go
/// into the G-code's per-tool values). `Err` lists the filaments that found no tool.
pub fn assign(
    used: &BTreeSet<usize>,
    filaments: usize,
    tools: usize,
    mixes: &[usize],
) -> Result<ToolMap, Vec<usize>> {
    let tools = tools.max(1);
    let mut slots: Vec<Option<usize>> = vec![None; tools];
    for &f in used.iter().filter(|&&f| f < tools) {
        slots[f] = Some(f);
    }
    let mut unplaced = Vec::new();
    for &f in used.iter().filter(|&&f| f >= tools) {
        match slots.iter().position(Option::is_none) {
            Some(t) => slots[t] = Some(f),
            None => unplaced.push(f),
        }
    }
    if !unplaced.is_empty() {
        return Err(unplaced);
    }
    let taken: BTreeSet<usize> = slots.iter().flatten().copied().collect();
    let slots: Vec<usize> = slots
        .iter()
        .enumerate()
        .map(|(t, s)| match s {
            Some(f) => *f,
            None if t < filaments && !taken.contains(&t) => t,
            None => 0,
        })
        .collect();
    let mut map = BTreeMap::new();
    for (t, &f) in slots.iter().enumerate() {
        if used.contains(&f) && f != t {
            map.insert(f + 1, t + 1);
        }
    }
    let filaments = filaments.max(1);
    if filaments != tools {
        for &id in mixes.iter().filter(|&&id| id > filaments) {
            map.insert(id, id - filaments + tools);
        }
    }
    Ok(ToolMap { slots, map })
}

/// The configuration of a plate's tools: tool t takes the per-filament values of filament
/// `slots[t]`, and the tool settings name tools (`map`).
pub fn to_tools(c: &Config, tools: &ToolMap, dialect: Option<orca::Dialect>) -> Config {
    let mut out = c.clone();
    for (k, v) in c.values() {
        if let ConfigValue::Vector(items) = v
            && is_filament_key(k, dialect)
        {
            let picked = tools
                .slots
                .iter()
                .map(|&f| items.get(f).or_else(|| items.first()).cloned().flatten())
                .collect();
            out.set(k, ConfigValue::Vector(picked));
        }
    }
    for (k, raw) in c.extras() {
        if is_filament_list(k, dialect) {
            let values: Vec<&str> = raw.split(',').collect();
            let picked: Vec<&str> = tools
                .slots
                .iter()
                .map(|&f| values.get(f).copied().unwrap_or(values[0]))
                .collect();
            let _ = out.set_raw(k, &picked.join(","));
        }
    }
    for k in crate::mixes::TOOL_KEYS {
        if let Some(t) = c.raw(k).and_then(|v| v.trim().parse::<usize>().ok())
            && let Some(n) = tools.map.get(&t)
        {
            let _ = out.set_raw(k, &n.to_string());
        }
    }
    out
}

impl App {
    /// The project's filaments (at least one per tool).
    pub(crate) fn filament_count(&self) -> usize {
        self.profiles.filament_count()
    }

    /// The filament and mixed color numbers plate `i` names (one-based): its objects' tools,
    /// settings and painted areas, its layer tool changes, and with `settings` the tool
    /// settings of the plate and of the print where they print something (support tools with
    /// supports or a raft, the tower's tool with a tower).
    fn plate_references(&self, i: usize, settings: bool) -> BTreeSet<usize> {
        let mut used: BTreeSet<usize> = BTreeSet::new();
        let mut add = |t: usize| {
            if t > 0 {
                used.insert(t);
            }
        };
        let plate = self.plates.plate(i);
        let tool_settings = |o: &BTreeMap<String, String>| -> Vec<usize> {
            crate::mixes::TOOL_KEYS
                .iter()
                .filter_map(|k| o.get(*k)?.trim().parse().ok())
                .collect()
        };
        if settings {
            for t in tool_settings(&plate.overrides) {
                add(t);
            }
            if let Some(c) = self.effective_config() {
                let supports = c.bool("support_material").unwrap_or(false)
                    || c.int("raft_layers").unwrap_or(0) > 0;
                for k in crate::mixes::TOOL_KEYS {
                    let applies = match k {
                        "support_material_extruder" | "support_material_interface_extruder" => {
                            supports
                        }
                        "wipe_tower_extruder" => c.bool("wipe_tower").unwrap_or(false),
                        _ => true,
                    };
                    if applies && let Some(t) = c.raw(k).and_then(|v| v.trim().parse().ok()) {
                        add(t);
                    }
                }
            }
        }
        for o in &plate.objects {
            for t in o.tools.0.iter().flatten() {
                add(t + 1);
            }
            for t in tool_settings(&o.overrides)
                .into_iter()
                .chain(o.volumes.iter().flat_map(|v| tool_settings(&v.overrides)))
                .chain(
                    o.height_ranges
                        .iter()
                        .flat_map(|r| tool_settings(&r.overrides)),
                )
            {
                add(t);
            }
            for s in o.painting.material.states() {
                add(usize::from(s));
            }
            if let Some(r) = &o.region {
                for v in &r.volumes {
                    for k in crate::mixes::TOOL_KEYS {
                        if let Some(t) = v.config.raw(k).and_then(|v| v.trim().parse().ok()) {
                            add(t);
                        }
                    }
                    for s in v.painting.material.states() {
                        add(usize::from(s));
                    }
                }
            }
        }
        for item in &plate.custom_gcode.items {
            add(item.extruder);
        }
        used
    }

    /// The filaments plate `i` prints with (zero-based): what it names
    /// ([`Self::plate_references`]), mixed colors by their components.
    pub(crate) fn plate_filaments(&self, i: usize, settings: bool) -> BTreeSet<usize> {
        let filaments = self.filament_count();
        let mut used = BTreeSet::new();
        for t in self.plate_references(i, settings) {
            match self.mixes.list.iter().find(|m| m.id == t) {
                Some(mix) => used.extend(
                    mix.components
                        .iter()
                        .filter(|c| c.extruder > 0)
                        .map(|c| c.extruder - 1),
                ),
                None => {
                    used.insert(t - 1);
                }
            }
        }
        used.retain(|&f| f < filaments);
        used
    }

    /// Adds filaments until the project has `wanted` (new ones take the first filament's
    /// preset; their colours come from the project); whether it added any.
    pub(crate) fn grow_filaments(&mut self, wanted: usize) -> bool {
        let f = &mut self.profiles.filaments;
        if f.len() >= wanted {
            return false;
        }
        let first = f[0].clone();
        f.resize(wanted, first);
        self.overlays.resize(wanted);
        true
    }

    /// Adds the filaments the scene names beyond the project's (a project for a printer with
    /// more tools, objects of another file): a number that is not a mixed color is a filament.
    pub(crate) fn fit_filaments_to_scene(&mut self) -> bool {
        let wanted = (0..self.plates.len())
            .flat_map(|i| self.plate_references(i, false))
            .filter(|t| !self.mixes.list.iter().any(|m| m.id == *t))
            .max()
            .unwrap_or(0);
        self.grow_filaments(wanted)
    }

    /// The mixed colors as plate tools see them: their components on the plate's tools and
    /// their numbers after the printer's tools (`tools.map`). A mix the plate does not use
    /// may name filaments without a tool; they become tool 1, it prints nothing.
    pub(crate) fn tool_spectrum(&self, tools: &ToolMap) -> Option<FullSpectrum> {
        if self.mixes.list.is_empty() {
            return None;
        }
        let count = self.profiles.tool_count();
        let list = self
            .mixes
            .list
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.id = tools.map.get(&m.id).copied().unwrap_or(m.id);
                for c in &mut m.components {
                    c.extruder = match tools.map.get(&c.extruder) {
                        Some(&t) => t,
                        None if c.extruder <= count => c.extruder,
                        None => 1,
                    };
                }
                m
            })
            .collect();
        FullSpectrum::new(list, count).ok()
    }

    /// Plate `i`'s filaments on the printer's tools ([`assign`]), or why they do not fit.
    pub(crate) fn plate_tools(&self, i: usize) -> Result<ToolMap, String> {
        let tools = self.profiles.tool_count();
        let filaments = self.filament_count();
        let used = self.plate_filaments(i, true);
        assign(
            &used,
            filaments,
            tools,
            &self.mixes.list.iter().map(|m| m.id).collect::<Vec<_>>(),
        )
        .map_err(|_| {
            let names: Vec<String> = used.iter().map(|f| format!("F{}", f + 1)).collect();
            format!(
                "{} uses {} filaments ({}), the printer has {tools} tools; move some to \
                 another plate, or print some parts with the same filament",
                self.plates.plate(i).name,
                used.len(),
                names.join(", ")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(fs: &[usize]) -> BTreeSet<usize> {
        fs.iter().copied().collect()
    }

    #[test]
    fn filaments_that_fit_stay_on_their_own_tools() {
        let m = assign(&set(&[0, 2]), 4, 4, &[]).unwrap();
        assert_eq!(m.slots, [0, 1, 2, 3]);
        assert!(m.is_identity());
    }

    #[test]
    fn extra_filaments_take_the_free_tools() {
        // F5 and F6 on a 4-tool printer whose plate also uses F1 and F3.
        let m = assign(&set(&[0, 2, 4, 5]), 6, 4, &[]).unwrap();
        assert_eq!(m.slots, [0, 4, 2, 5]);
        assert_eq!(m.map, BTreeMap::from([(5, 2), (6, 4)]));
    }

    #[test]
    fn mixed_colors_follow_the_tools() {
        // Six filaments, two mixes (M1 = 7, M2 = 8) on a 4-tool printer: M1 = 5, M2 = 6.
        let m = assign(&set(&[0]), 6, 4, &[7, 8]).unwrap();
        assert_eq!(m.map, BTreeMap::from([(7, 5), (8, 6)]));
    }

    #[test]
    fn too_many_filaments_on_one_plate_are_named() {
        assert_eq!(assign(&set(&[0, 1, 2, 3, 4]), 5, 4, &[]), Err(vec![4]));
    }

    #[test]
    fn a_plates_configuration_takes_its_filaments_per_tool() {
        let mut c = Config::defaults();
        c.set_raw("nozzle_diameter", "0.4,0.4").unwrap();
        c.set_raw("temperature", "200,210,220").unwrap();
        c.set_raw("support_material_extruder", "3").unwrap();
        let tools = assign(&set(&[0, 2]), 3, 2, &[]).unwrap();
        let t = to_tools(&c, &tools, None);
        assert_eq!(t.raw("temperature").unwrap(), "200,220");
        assert_eq!(t.raw("support_material_extruder").unwrap(), "2");
        assert_eq!(t.raw("nozzle_diameter").unwrap(), "0.4,0.4");
    }

    #[test]
    fn more_filaments_than_tools_repeat_the_first_until_set() {
        let mut c = Config::defaults();
        c.set_raw("temperature", "200,210").unwrap();
        resize(&mut c, 4, None);
        assert_eq!(c.raw("temperature").unwrap(), "200,210,200,200");
    }
}
