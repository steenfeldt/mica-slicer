// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Moving what prints with one physical tool to another: "Swap with T3" and "Move to T3" on a
//! row of the Filaments card. Everything that names the tool follows: objects, parts and
//! height ranges, painted areas, mixed colors, layer tool and color changes, plate settings and
//! the print settings' support and wipe tower tools. The filaments in the slots stay where
//! they are, so the parts take the other slot's filament (after a sync from the printer, the
//! parts go back to the colors they had).

use crate::app::App;
use mica_core::custom_gcode::Kind;
use mica_core::fdm_mixer;
use mica_core::full_spectrum::FullSpectrum;
use std::collections::BTreeMap;

/// Zero-based physical tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRemap {
    /// The two tools trade everything that prints with them.
    Swap(usize, usize),
    /// Everything that prints with the first tool prints with the second; the first is unused.
    Move(usize, usize),
}

/// The print settings that name a tool (one-based, 0 for "the current one").
const PRINT_KEYS: [&str; 3] = [
    "support_material_extruder",
    "support_material_interface_extruder",
    "wipe_tower_extruder",
];

impl ToolRemap {
    /// One-based old tool -> new tool.
    fn map(self) -> BTreeMap<usize, usize> {
        match self {
            Self::Swap(a, b) => BTreeMap::from([(a + 1, b + 1), (b + 1, a + 1)]),
            Self::Move(a, b) => BTreeMap::from([(a + 1, b + 1)]),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Swap(..) => "Swap tools",
            Self::Move(..) => "Move to tool",
        }
    }
}

impl App {
    /// Applies `remap` to the whole project as one undo step (named `undo_label`, else after
    /// the action).
    pub(crate) fn remap_tool(
        &mut self,
        remap: ToolRemap,
        undo_label: Option<&str>,
    ) -> Result<(), String> {
        let (ToolRemap::Swap(a, b) | ToolRemap::Move(a, b)) = remap;
        let physical = self.filament_count();
        if a == b || a >= physical || b >= physical {
            return Err(format!("no tools {} and {} to remap", a + 1, b + 1));
        }
        let map = remap.map();
        let one = |t: usize| map.get(&t).copied().unwrap_or(t);
        // Mixed colors use the new tools; a mix of both tools would mix a tool with itself.
        let mut mixes = self.mixes.list.clone();
        for (k, m) in mixes.iter_mut().enumerate() {
            for c in &mut m.components {
                c.extruder = one(c.extruder);
            }
            let mut tools: Vec<usize> = m.components.iter().map(|c| c.extruder).collect();
            tools.sort_unstable();
            tools.dedup();
            if tools.len() < m.components.len() {
                return Err(format!(
                    "{} mixes T{} and T{}: it would mix T{} with itself. Change or delete it first",
                    self.mix_name(k),
                    a + 1,
                    b + 1,
                    b + 1
                ));
            }
        }
        FullSpectrum::new(mixes.clone(), physical)?;
        let print: Vec<(&str, String)> = self
            .effective_config()
            .into_iter()
            .flat_map(|c| {
                PRINT_KEYS.iter().filter_map(|&k| {
                    let t = c.raw(k)?.trim().parse::<usize>().ok()?;
                    map.get(&t).map(|n| (k, n.to_string()))
                })
            })
            .collect();

        self.record(undo_label.unwrap_or(remap.label()));
        self.renumber_tools(&map);
        self.mixes.list = mixes;
        if let Some(d) = &mut self.mixes.draft {
            for t in &mut d.tools {
                *t = one(*t + 1) - 1;
            }
        }
        let colours = self.tool_colours();
        for plate in &mut self.plates.plates {
            for item in &mut plate.custom_gcode.items {
                if item.extruder == 0 || !map.contains_key(&item.extruder) {
                    continue;
                }
                item.extruder = one(item.extruder);
                if item.kind == Kind::ToolChange
                    && let Some(&c) = colours.get(item.extruder - 1)
                {
                    item.color = fdm_mixer::rgb_to_hex(c);
                }
            }
        }
        let settings = !print.is_empty();
        for (k, v) in print {
            self.overlays.print.insert(k.into(), v);
        }
        if settings {
            self.settings_changed();
        }
        self.plates.invalidate_all();
        self.changed();
        self.status = match remap {
            ToolRemap::Swap(..) => format!(
                "{} and {} swapped",
                self.filament_label(a),
                self.filament_label(b)
            ),
            ToolRemap::Move(..) => format!(
                "{} moved to {}",
                self.filament_label(a),
                self.filament_label(b)
            ),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use mica_core::custom_gcode::Item;
    use mica_core::painting::{FacetPainting, selector::encode_state};
    use std::sync::Arc;

    /// The XL 5T with the 20 mm cube.
    fn xl_app(ctx: &egui::Context) -> Option<App> {
        use std::sync::atomic::{AtomicU64, Ordering};
        if !mica_testkit::require_prusa_profiles() {
            return None;
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mica-tool-remap-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::create(
            ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(dir)),
                files: vec![],
                headless: true,
            },
        );
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == "Original Prusa XL - 5T Input Shaper 0.4 nozzle")
            .expect("the XL 5T among the loaded printers");
        app.profiles.select_printer(vi, &name);
        app.settings_changed();
        let model = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/models/cube20.stl"
        ));
        let loaded = crate::app::load_files(vec![model], &app.config, app.bed.center());
        app.finish_load(loaded);
        Some(app)
    }

    /// Facet 0 painted with tool `t` (one-based), facet 1 with `u`.
    fn painting(t: usize, u: usize) -> FacetPainting {
        let mut p = FacetPainting::default();
        for (facet, state) in [(0, t), (1, u)] {
            let mut codes = vec![];
            encode_state(state as u8, &mut codes);
            p.material.push_codes(facet, &codes).unwrap();
        }
        p
    }

    fn painted(app: &App) -> FacetPainting {
        (*app.plates.current().objects[0].painting).clone()
    }

    #[test]
    fn swapping_moves_parts_paint_mixes_and_layer_changes() {
        let ctx = egui::Context::default();
        let Some(mut app) = xl_app(&ctx) else {
            return;
        };
        app.add_mix_spec("1:50,2:50").unwrap();
        {
            let o = &mut app.plates.current_mut().objects[0];
            o.set_tool(0);
            o.tools.0[3] = Some(2);
            o.overrides.insert("infill_extruder".into(), "3".into());
            o.painting = Arc::new(painting(3, 6));
            o.painting_edited = true;
        }
        let plate = app.plates.current_mut();
        plate
            .custom_gcode
            .items
            .push(Item::new(5., Kind::ToolChange, 3));
        plate
            .custom_gcode
            .items
            .push(Item::new(6., Kind::ColorChange, 2));
        app.remap_tool(ToolRemap::Swap(0, 2), None).unwrap();

        let o = &app.plates.current().objects[0];
        assert_eq!(&o.tools.0[..4], [Some(2), Some(2), Some(2), Some(0)]);
        assert_eq!(o.overrides["infill_extruder"], "1");
        assert_eq!(painted(&app), painting(1, 6), "the mix keeps its number");
        let tools: Vec<usize> = app.mixes.list[0]
            .components
            .iter()
            .map(|c| c.extruder)
            .collect();
        assert_eq!(tools, [3, 2]);
        let items = &app.plates.current().custom_gcode.items;
        assert_eq!((items[0].extruder, items[1].extruder), (1, 2));
        assert_eq!(items[0].color, fdm_mixer::rgb_to_hex(app.tool_colours()[0]));

        app.undo();
        let o = &app.plates.current().objects[0];
        assert_eq!(o.tools.0[0], Some(0), "one undo step");
        assert_eq!(painted(&app), painting(3, 6));
        assert_eq!(app.plates.current().custom_gcode.items[0].extruder, 3);
    }

    #[test]
    fn moving_leaves_the_tool_unused_and_follows_the_print_settings() {
        let ctx = egui::Context::default();
        let Some(mut app) = xl_app(&ctx) else {
            return;
        };
        app.overlays
            .print
            .insert("support_material_extruder".into(), "2".into());
        app.settings_changed();
        {
            let o = &mut app.plates.current_mut().objects[0];
            o.set_tool(1);
            o.painting = Arc::new(painting(2, 4));
            o.painting_edited = true;
        }
        app.remap_tool(ToolRemap::Move(1, 3), None).unwrap();
        let o = &app.plates.current().objects[0];
        assert_eq!(&o.tools.0[..3], [Some(3); 3]);
        assert_eq!(painted(&app), painting(4, 4));
        assert_eq!(app.overlays.print["support_material_extruder"], "4");
    }

    #[test]
    fn a_mix_of_both_tools_blocks_a_move() {
        let ctx = egui::Context::default();
        let Some(mut app) = xl_app(&ctx) else {
            return;
        };
        app.add_mix_spec("1:50,2:50").unwrap();
        app.plates.current_mut().objects[0].set_tool(0);
        let e = app.remap_tool(ToolRemap::Move(0, 1), None).unwrap_err();
        assert!(e.contains("with itself"), "{e}");
        assert_eq!(
            app.plates.current().objects[0].tools.0[0],
            Some(0),
            "nothing changed"
        );
        app.remap_tool(ToolRemap::Swap(0, 1), None).unwrap();
        assert!(
            app.remap_tool(ToolRemap::Swap(0, 5), None).is_err(),
            "T6 is a mix, not a tool"
        );
    }
}
