// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The monkey's actions for the features of October 2026: built-in shapes ("Add shape"),
//! textures on all surfaces with per-slot depth and "Bake texture into mesh", ColorMix mixes
//! (the editor's add / delete / move, object tools on mixes), Brick layers unlocked by hand,
//! profile import from a fake home with OrcaSlicer, Snapmaker Orca and PrusaSlicer presets
//! (odd and broken ones too), and Mica MCP live calls (`create_mix`, painting, `add_shape`,
//! `undo`, …) sent while the user keeps editing: the answer of a call must come within
//! [`MCP_TIMEOUT`], whatever happened in the window meanwhile.

use super::Monkey;
use crate::builtin::{AddAs, Placement};
use crate::mcp_live::Incoming;
use crate::overlays::Target;
use crate::panels::editor::Edit;
use mica_cli::mcp::live::{PROTOCOL, Request, Response};
use mica_core::builtin_models::BuiltinModel;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A live call must be answered within this long (slices for `slice_summary` included).
pub(super) const MCP_TIMEOUT: Duration = Duration::from_secs(120);

/// A Mica MCP call sent to the window and not answered yet.
pub(super) struct Pending {
    pub sent: Instant,
    pub step: usize,
    pub tool: String,
    pub answer: mpsc::Receiver<Response>,
}

/// Mica MCP live mode in process (the window's request queue without a socket).
#[derive(Default)]
pub(super) struct Mcp {
    pub tx: Option<mpsc::Sender<Incoming>>,
    pub pending: Vec<Pending>,
    /// View ids `render_view` answered with (for `paint_at` and `pick`).
    pub views: Vec<u32>,
    pub next_id: u64,
}

fn write(path: &Path, text: &[u8]) {
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(path, text);
}

/// A home folder with other slicers' user presets: plausible ones for the three printers,
/// extreme values, @-tagged and non-ASCII names, a preset inheriting from itself, from a
/// missing parent, and files that are not presets at all.
pub(super) fn fake_home(dir: &Path) -> PathBuf {
    let home = dir.join("home");
    let u1 = "Snapmaker U1 (0.4 nozzle)";
    let u1_process = "0.20 Standard @Snapmaker U1 (0.4 nozzle)";
    for (app, extra) in [
        ("OrcaSlicer", ""),
        ("Snapmaker_Orca", r#","version":"2.4.0.7""#),
    ] {
        let root = home.join(format!("Library/Application Support/{app}/user/default"));
        let j = |s: String| s.into_bytes();
        write(
            &root.join("machine/My U1.json"),
            &j(format!(
                r#"{{"from":"User","inherits":"{u1}","name":"My U1 @home","printer_extruder_id":["1","2","3","4"]{extra}}}"#
            )),
        );
        write(
            &root.join("process/Fine.json"),
            &j(format!(
                r#"{{"from":"User","inherits":"{u1_process}","name":"Fine ✓","sparse_infill_density":"0%","wall_loops":"0","top_shell_layers":"0","bottom_shell_layers":"0"{extra}}}"#
            )),
        );
        write(
            &root.join("process/Thick.json"),
            &j(format!(
                r#"{{"from":"User","inherits":"{u1_process}","name":"Thick @U1","layer_height":"0.6","wall_loops":"40","sparse_infill_density":"100%","mica_brick_layers":"1","mica_texture_skin":"all","mica_texture_skin_surfaces":"walls_and_tops","mica_texture_skin_amplitude":"2"}}"#
            )),
        );
        write(
            &root.join("process/Loop.json"),
            br#"{"from":"User","inherits":"Loop","name":"Loop"}"#,
        );
        write(
            &root.join("process/Orphan.json"),
            br#"{"from":"User","inherits":"0.20mm Standard @Nowhere 0.4","name":"Orphan"}"#,
        );
        write(
            &root.join("filament/base/My PLA.json"),
            br#"{"from":"User","inherits":"Snapmaker PLA @U1","name":"My PLA @U1","nozzle_temperature":["999"],"filament_max_volumetric_speed":["0"]}"#,
        );
        write(
            &root.join("filament/Weird.json"),
            br##"{"from":"User","inherits":"Snapmaker PLA @U1","name":"","filament_colour":["#GGHHII"],"nozzle_temperature":["-5"]}"##,
        );
        write(
            &root.join("filament/Broken.json"),
            b"{\"name\": \"Broken\",",
        );
        write(&root.join("filament/Array.json"), b"[1,2,3]");
        write(&root.join("machine/Empty.json"), b"");
        write(&root.join("process/Binary.json"), &[0xff, 0xfe, 0x00, 0x7b]);
    }
    let prusa = home.join("Library/Application Support/PrusaSlicer");
    write(&prusa.join("PrusaSlicer.ini"), b"[presets]\n");
    write(
        &prusa.join("print/Zero walls.ini"),
        b"inherits = 0.20mm SPEED @COREONE HF0.4\nperimeters = 0\nfill_density = 0%\ntop_solid_layers = 0\nbottom_solid_layers = 0\n",
    );
    write(
        &prusa.join("print/Huge layers.ini"),
        b"inherits = 0.20mm SPEED @XLIS 0.4\nlayer_height = 5\nfirst_layer_height = 5\n",
    );
    write(
        &prusa.join("filament/PLA Tweaked.ini"),
        b"inherits = Prusament PLA @COREONE HF0.4\ntemperature = 212\n",
    );
    write(
        &prusa.join("filament/Odd.ini"),
        b"inherits = No Such Filament\ntemperature = 212\n",
    );
    write(&prusa.join("filament/Junk.ini"), b"\x00\x01=\n[[[\n= =\n");
    write(
        &prusa.join("printer/My XL.ini"),
        b"inherits = Original Prusa XL - 5T Input Shaper 0.4 nozzle\nbed_shape = 0x0\n",
    );
    home
}

impl Monkey {
    /// Turns a Settings › Features switch on or off as its checkbox does: a change re-slices
    /// (GUI monkey test, seed 413: flipping it directly left a slice made with the feature off
    /// on its plate).
    fn feature_on(
        &mut self,
        switch: impl Fn(&mut mica_profiles::features::Features) -> &mut bool,
        on: bool,
    ) {
        let flag = switch(&mut self.app.prefs.features);
        if *flag != on {
            *flag = on;
            self.app.prefs_dirty = true;
            self.app.settings_changed();
        }
    }

    /// One of the new-feature actions.
    pub(super) fn new_features(&mut self) {
        match self.rng.below(12) {
            0 | 1 => self.add_shape(),
            2 | 3 => self.texture_everywhere(),
            4 => self.bake(),
            5 | 6 => self.mixes(),
            7 => self.brick_layers(),
            8 => self.import_profiles(),
            _ => self.mcp_call(),
        }
    }

    fn add_shape(&mut self) {
        let model = *self.rng.pick(&BuiltinModel::ALL);
        let add_as = *self.rng.pick(&AddAs::ALL);
        if add_as != AddAs::Object
            && self.rng.chance(0.7)
            && let Some(i) = self.random_object()
        {
            self.app.select(Some(i), false);
        }
        let size = match self.rng.below(5) {
            0 => Some([0.01, 0.01, 0.01]),
            1 => Some([500., 500., 500.]),
            2 => Some([self.rng.range(0.1, 80.) as f32, 1., 0.05]),
            3 => Some([0., 10., 10.]),
            _ => None,
        };
        let at = self.rng.chance(0.3).then(|| {
            glam::Vec2::new(
                self.rng.range(-500., 800.) as f32,
                self.rng.range(-500., 800.) as f32,
            )
        });
        self.note(format!(
            "add shape {model:?} as {add_as:?} size {size:?} at {at:?}"
        ));
        if self.rng.chance(0.5) {
            self.app.add_builtin(model, add_as);
        } else if let Err(e) =
            self.app
                .add_builtin_with(model, add_as, Placement { size, at }, None)
        {
            self.note(format!("  refused: {e}"));
        }
    }

    fn texture_value(&mut self) -> String {
        match self.rng.below(12) {
            0..=5 => {
                let p = self.rng.pick(mica_core::texture::procedural::ALL);
                format!("procedural:{}", p.id)
            }
            6..=10 => {
                let b = self.rng.pick(mica_core::texture::catalog::BUILTINS);
                format!("builtin:{}", b.id)
            }
            _ => {
                // A custom image the project does not hold.
                format!("sha256:{:064x}", self.rng.next_u64())
            }
        }
    }

    /// Textures on all surfaces: the print's, an object's, or per paint slot (texture paint
    /// slots with their own depth and image), at depths up to the field's limit.
    fn texture_everywhere(&mut self) {
        self.feature_on(|f| &mut f.textures, true);
        if self.rng.chance(0.3) {
            // The shapes Textures level 2 has to be practical on: a large slab, a tall cube,
            // the Benchy (texture-painted below).
            let (model, size) = match self.rng.below(3) {
                // A large slab at 0.05 mm point distance takes minutes per plate and cancels
                // late (an open issue): a smaller slab here.
                0 => (BuiltinModel::Cube, Some([60., 50., 4.])),
                1 => (BuiltinModel::Cube, Some([20., 20., 180.])),
                _ => (BuiltinModel::Benchy, None),
            };
            self.note(format!("  textured test shape {model:?} {size:?}"));
            let _ =
                self.app
                    .add_builtin_with(model, AddAs::Object, Placement { size, at: None }, None);
            if model == BuiltinModel::Benchy {
                use mica_core::painting::PaintKind;
                self.app.viewport.paint = Some(PaintKind::Texture);
            }
        }
        let texture = self.texture_value();
        let depth = *self.rng.pick(&["0", "0.05", "0.4", "1", "2"]);
        let point = *self.rng.pick(&["0.05", "0.25", "1", "5"]);
        let mut edits: Vec<(String, String)> = vec![
            (
                "mica_texture_skin".into(),
                self.rng.pick(&["external", "all", "none"]).to_string(),
            ),
            (
                "mica_texture_skin_surfaces".into(),
                if self.rng.chance(0.8) {
                    "walls_and_tops"
                } else {
                    "walls"
                }
                .into(),
            ),
            ("mica_texture_skin_texture".into(), texture),
            ("mica_texture_skin_amplitude".into(), depth.into()),
            ("mica_texture_skin_point_dist".into(), point.into()),
            (
                "mica_texture_skin_direction".into(),
                self.rng.pick(&["outward", "inward", "both"]).to_string(),
            ),
            (
                "mica_texture_skin_projection".into(),
                self.rng
                    .pick(&[
                        "triplanar",
                        "cubic",
                        "cylindrical",
                        "spherical",
                        "planar_xy",
                        "planar_xz",
                    ])
                    .to_string(),
            ),
            (
                "mica_texture_skin_first_layer".into(),
                self.rng.pick(&["0", "1"]).to_string(),
            ),
        ];
        if self.rng.chance(0.5) {
            let slots: Vec<String> = (0..self.rng.below(4) + 1)
                .map(|_| {
                    let n = self.rng.below(16) + 1;
                    let t = self.texture_value();
                    let d = *self
                        .rng
                        .pick(&["0", "0.3", "0.8", "2", "2", "0.1", "1.5", "5"]);
                    let s = if self.rng.chance(0.5) {
                        ",surfaces=walls_and_tops"
                    } else {
                        ""
                    };
                    format!("{n}:texture={t},amplitude={d}{s}")
                })
                .collect();
            edits.push(("mica_texture_paint_slots".into(), slots.join(";")));
        }
        let target = match self.rng.below(3) {
            0 | 1 => Some(Target::Print),
            _ => {
                if let Some(i) = self.random_object() {
                    self.app.select(Some(i), false);
                    self.app.editor.object_mode = true;
                }
                None
            }
        };
        self.note(format!("texture all surfaces on {target:?}: {edits:?}"));
        for (key, raw) in edits {
            self.app.apply_edit(target, Edit::Set { key, raw });
        }
        self.app.end_gesture();
        if self.rng.chance(0.3) {
            // A texture stroke with a high slot.
            self.paint();
        }
        if self.rng.chance(0.4) {
            let ctx = self.ctx.clone();
            self.note("  slice textured plate".into());
            self.app.slice_current(&ctx);
            match self.rng.below(3) {
                0 => {
                    let after = Duration::from_millis(self.rng.below(2000) as u64);
                    self.wait_for_slice(Some(after));
                }
                // Watched to the end (or the slice budget): textured slices must complete.
                1 => self.wait_for_slice(None),
                _ => {}
            }
        }
    }

    fn bake(&mut self) {
        self.feature_on(|f| &mut f.textures, true);
        if self.app.selected.is_none()
            && let Some(i) = self.random_object()
        {
            self.app.select(Some(i), false);
        }
        let Some(i) = self.app.selected else { return };
        let faces = self.app.plates.current().objects[i].mesh.faces.len();
        self.note(format!("bake texture into object {i} ({faces} facets)"));
        let started = Instant::now();
        let n = self.app.bake_texture();
        self.note(format!(
            "  baked: {n:?} facets in {:.1} s",
            started.elapsed().as_secs_f64()
        ));
        if started.elapsed() > Duration::from_secs(60) {
            self.problems.push(format!(
                "Bake texture held the window for {:.0} s ({faces} facets)",
                started.elapsed().as_secs_f64()
            ));
        }
    }

    /// ColorMix: add mixes (valid and broken specs), delete and move them, give objects a
    /// mix as their tool.
    fn mixes(&mut self) {
        self.feature_on(|f| &mut f.color_mix, true);
        let tools = self.app.tool_count();
        let mixes = self.app.mixes.list.len();
        match self.rng.below(6) {
            0..=2 => {
                let n = self.rng.below(4) + 1;
                let parts: Vec<String> = (0..n)
                    .map(|_| {
                        format!(
                            "{}:{}",
                            self.rng.below(tools + 2),
                            self.rng
                                .pick(&["0", "1", "33", "50", "99", "100", "-5", "1e9"])
                        )
                    })
                    .collect();
                let pattern =
                    *self
                        .rng
                        .pick(&["", "/outer", "/gradient:20,80", "/gradient:", "/x"]);
                let name = *self.rng.pick(&["", "@Orange", "@", "@ñ✓"]);
                let spec = format!("{}{pattern}{name}", parts.join(","));
                let r = self.app.add_mix_spec(&spec);
                self.note(format!("add mix {spec:?}: {r:?}"));
            }
            3 if mixes > 0 => {
                let id = self.app.mixes.list[self.rng.below(mixes)].id;
                self.note(format!("delete mix {id}"));
                self.app
                    .apply_mix_acts(vec![crate::mixes::MixAct::Delete(id)]);
            }
            4 if mixes > 1 => {
                let a = self.app.mixes.list[self.rng.below(mixes)].id;
                let b = self.app.mixes.list[self.rng.below(mixes)].id;
                self.note(format!("swap mixes {a} {b}"));
                self.app
                    .apply_mix_acts(vec![crate::mixes::MixAct::Swap(a, b)]);
            }
            _ => {
                let choices = self.app.tool_choices();
                if choices.is_empty() {
                    return;
                }
                let t = self.rng.below(choices.len() + 1);
                self.note(format!("object tool {t} of {} choices", choices.len()));
                self.handle(crate::viewport::Action::Command {
                    index: None,
                    command: crate::commands::Command::ChangeTool(t),
                });
            }
        }
    }

    /// Brick layers, unlocked as `brick_layers = true` under `[features]` would, with its
    /// settings at random (odd layer counts, start past the top, margins larger than the
    /// object), sometimes locked again.
    fn brick_layers(&mut self) {
        let on = !self.rng.chance(0.15);
        self.feature_on(|f| &mut f.brick_layers, on);
        let keys = [
            ("mica_brick_layers", vec!["1", "1", "0"]),
            (
                "mica_brick_layers_flow_ratio",
                vec!["100%", "105%", "150%", "0%", "1000%"],
            ),
            (
                "mica_brick_layers_start_layer",
                vec!["1", "2", "3", "7", "1000", "0"],
            ),
            (
                "mica_brick_layers_end_margin_layers",
                vec!["0", "1", "3", "50"],
            ),
            ("mica_brick_layers_travel_lift", vec!["0", "0.2", "5"]),
        ];
        let target = if self.rng.chance(0.7) {
            Some(Target::Print)
        } else {
            if let Some(i) = self.random_object() {
                self.app.select(Some(i), false);
                self.app.editor.object_mode = true;
            }
            None
        };
        let mut set = vec![];
        for (key, values) in keys {
            if key == "mica_brick_layers" || self.rng.chance(0.5) {
                let raw = self.rng.pick(&values).to_string();
                set.push(format!("{key}={raw}"));
                self.app.apply_edit(
                    target,
                    Edit::Set {
                        key: key.into(),
                        raw,
                    },
                );
            }
        }
        if self.rng.chance(0.4) {
            // Odd layer counts: layer heights that do not divide the object.
            let raw = self.rng.pick(&["0.07", "0.13", "0.3", "0.33"]).to_string();
            set.push(format!("layer_height={raw}"));
            self.app.apply_edit(
                target,
                Edit::Set {
                    key: "layer_height".into(),
                    raw,
                },
            );
        }
        self.app.end_gesture();
        self.app.settings_changed();
        self.note(format!("brick layers unlocked {on} on {target:?}: {set:?}"));
    }

    /// Profile import from the fake home: review (all, none or the default pick), import,
    /// use the imported presets, remove, merge duplicates, re-import.
    fn import_profiles(&mut self) {
        let home = self.home.clone();
        self.app.profiles.look_in(&home, None);
        match self.rng.below(6) {
            0 | 1 => {
                self.app.open_import_review();
                let pick = self.rng.below(3);
                let rows = self
                    .app
                    .import_ui
                    .review
                    .as_ref()
                    .map(|r| r.rows.clone())
                    .unwrap_or_default();
                let chosen: Vec<&crate::import::Candidate> = rows
                    .iter()
                    .filter(|r| r.importable())
                    .filter(|r| match pick {
                        0 => true,
                        1 => r.default_pick(),
                        _ => r.found.name.len() % 2 == 0,
                    })
                    .collect();
                self.note(format!(
                    "import {} of {} found profiles",
                    chosen.len(),
                    rows.len()
                ));
                let done = self.app.profiles.import(&chosen, &mut self.app.prefs);
                self.app.import_ui.review = None;
                self.app.after_profile_cleanup();
                self.note(format!(
                    "  imported {:?}, errors {:?}",
                    done.names, done.errors
                ));
            }
            2 => {
                let mut prints = self.app.profiles.processes.clone();
                prints.retain(|p| !p.is_empty());
                if prints.is_empty() {
                    return;
                }
                // Imported ones are more interesting than the system presets.
                let imported: Vec<String> = self
                    .app
                    .profiles
                    .imported()
                    .iter()
                    .map(|p| p.name.clone())
                    .filter(|n| prints.contains(n))
                    .collect();
                let name = if !imported.is_empty() && self.rng.chance(0.7) {
                    self.rng.pick(&imported).clone()
                } else {
                    self.rng.pick(&prints).clone()
                };
                self.note(format!("print profile {name:?}"));
                self.app.profiles.process = name;
                self.app.profiles.refresh();
                self.app.settings_changed();
            }
            3 => {
                let unused = self.app.profiles.unused_imported(&self.app.prefs);
                let which: Vec<_> = unused
                    .into_iter()
                    .filter(|_| self.rng.chance(0.5))
                    .collect();
                self.note(format!("remove imported profiles {which:?}"));
                let done = self
                    .app
                    .profiles
                    .remove_profiles(&which, &mut self.app.prefs);
                self.app.after_profile_cleanup();
                self.note(format!(
                    "  removed {:?}, errors {:?}",
                    done.names, done.errors
                ));
            }
            4 => {
                let groups = self.app.profiles.duplicate_groups();
                if groups.is_empty() {
                    return;
                }
                let g = self.rng.pick(&groups).clone();
                self.note(format!("merge duplicates {g:?}"));
                let done = self.app.profiles.merge_duplicates(&g, &mut self.app.prefs);
                self.app.after_profile_cleanup();
                self.note(format!("  merged: errors {:?}", done.errors));
            }
            _ => {
                // The other slicer changed a preset: re-import (theirs or ours).
                let p = home
                    .join("Library/Application Support/OrcaSlicer/user/default/process/Fine.json");
                let walls = self.rng.below(5);
                write(
                    &p,
                    format!(
                        r#"{{"from":"User","inherits":"0.20 Standard @Snapmaker U1 (0.4 nozzle)","name":"Fine ✓","wall_loops":"{walls}"}}"#
                    )
                    .as_bytes(),
                );
                let stale = self.app.profiles.stale_imports();
                self.note(format!("re-import {} changed profiles", stale.len()));
                let which: Vec<_> = stale.iter().map(|s| (s.kind, s.name.clone())).collect();
                let theirs = self.rng.chance(0.5);
                let done = self
                    .app
                    .profiles
                    .reimport(&which, theirs, &mut self.app.prefs);
                self.note(format!("  {:?}: errors {:?}", done.names, done.errors));
                self.app.after_profile_cleanup();
            }
        }
    }

    /// One Mica MCP live call. Most are not waited for: the window keeps being edited while
    /// it runs (the call must notice and answer anyway); some are waited for.
    pub(super) fn mcp_call(&mut self) {
        let Some(tx) = self.mcp.tx.clone() else {
            return;
        };
        let tools = self.app.tool_count().max(1);
        let objects = self.app.plates.current().objects.len();
        let object = || -> Value { Value::Null };
        let obj = |rng: &mut super::Rng| -> Value {
            match rng.below(4) {
                0 => object(),
                _ => json!(rng.below(objects + 2) as u32),
            }
        };
        let tool = self.rng.below(tools + 3) as u32;
        let view = self.mcp.views.last().copied().unwrap_or(1);
        let (name, args): (&str, Value) = match self.rng.below(16) {
            0 => ("status", json!({})),
            1 => ("scene_info", json!({})),
            2 | 3 => (
                "render_view",
                json!({
                    "view": self.rng.pick(&["iso", "front", "top", "left", "bogus"]),
                    "size": self.rng.pick(&[16u32, 64, 256, 4096]),
                    "object": obj(&mut self.rng),
                    "zoom": self.rng.pick(&[0.0f32, 1., 10., -1.]),
                }),
            ),
            4 | 5 => (
                "paint_at",
                json!({
                    "view_id": view,
                    "x": self.rng.below(300) as u32,
                    "y": self.rng.below(300) as u32,
                    "tool": tool,
                    "mode": self.rng.pick(&["smart_fill", "connected", "triangle", "sphere", "bogus"]),
                    "radius": self.rng.pick(&[0.0f32, 2., 50.]),
                    "max_distance": self.rng.pick(&[1.0f32, 30., 1e6]),
                    "allow_large": self.rng.chance(0.5),
                }),
            ),
            6 => (
                "pick",
                json!({"view_id": view, "x": self.rng.below(300) as u32, "y": self.rng.below(300) as u32}),
            ),
            7 => {
                let a = self.rng.range(-10., 100.) as f32;
                let b = self.rng.range(-10., 100.) as f32;
                (
                    "paint_height_band",
                    json!({"object": obj(&mut self.rng), "z_min": a, "z_max": b, "tool": tool}),
                )
            }
            8 => (
                "paint_by_direction",
                json!({
                    "object": obj(&mut self.rng),
                    "direction": self.rng.pick(&["up", "down", "side", "front", "0,0,0", "1,2", "nan,0,1"]),
                    "max_angle": self.rng.pick(&[0.0f32, 30., 180.]),
                    "tool": tool,
                }),
            ),
            9 => (
                "set_part_tool",
                json!({"object": obj(&mut self.rng), "part": obj(&mut self.rng), "tool": tool}),
            ),
            10 => {
                let n = self.rng.below(tools + 2);
                let colors: Vec<String> = (0..n)
                    .map(|_| {
                        self.rng
                            .pick(&["red", "#00FF00", "yellow", "#123", "nonsense", "#FFFFFF"])
                            .to_string()
                    })
                    .collect();
                ("set_filament_colors", json!({"colors": colors}))
            }
            11 | 12 => (
                "create_mix",
                json!({
                    "target": self.rng.pick(&["orange", "#808080", "purple", "#7F3FBF", "pink", "x"]),
                    "tools": if self.rng.chance(0.5) { Value::Null } else {
                        json!((0..self.rng.below(4) + 1).map(|_| self.rng.below(tools + 2)).collect::<Vec<_>>())
                    },
                    "max_components": self.rng.pick(&[0usize, 1, 2, 3, 9]),
                    "pattern": self.rng.pick(&["layers", "outer_wall_first", "gradient", "x"]),
                }),
            ),
            13 => ("clear_paint", json!({"object": obj(&mut self.rng)})),
            14 => (
                "add_shape",
                json!({
                    "shape": self.rng.pick(&["benchy", "cube", "torus", "disc", "dodecahedron"]),
                    "size": self.rng.pick(&[json!(null), json!(0.0), json!(20.0), json!([1.0, 2.0, 3.0]), json!(1e6)]),
                    "kind": self.rng.pick(&["object", "part", "modifier", "negative_volume", "support_blocker", "x"]),
                    "object": obj(&mut self.rng),
                }),
            ),
            _ => match self.rng.below(3) {
                0 => ("undo", json!({})),
                1 => ("slice_summary", json!({})),
                _ => (
                    "save_project",
                    json!({"path": self.dir.join(format!("mcp-{}.3mf", self.step)).display().to_string()}),
                ),
            },
        };
        self.mcp.next_id += 1;
        let (reply, answer) = mpsc::channel();
        let request = Request {
            v: PROTOCOL,
            id: self.mcp.next_id,
            tool: name.into(),
            args: args.clone(),
        };
        if tx.send(Incoming { request, reply }).is_err() {
            return;
        }
        self.stats.mcp_calls += 1;
        self.note(format!("mcp {name} {args}"));
        self.mcp.pending.push(Pending {
            sent: Instant::now(),
            step: self.step,
            tool: name.into(),
            answer,
        });
        if self.rng.chance(0.4) {
            // Wait for it, pumping the window like a user watching.
            let deadline = Instant::now() + MCP_TIMEOUT;
            while !self.mcp.pending.is_empty() && Instant::now() < deadline {
                self.frame(vec![]);
                self.poll();
                self.collect_mcp();
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    /// Takes the answers that arrived; a call unanswered past [`MCP_TIMEOUT`] is a hang.
    pub(super) fn collect_mcp(&mut self) {
        let mut still = vec![];
        for p in std::mem::take(&mut self.mcp.pending) {
            match p.answer.try_recv() {
                Ok(r) => {
                    self.stats.mcp_answered += 1;
                    if r.ok {
                        for c in &r.content {
                            if let Some(t) = c.as_text()
                                && let Ok(v) = serde_json::from_str::<Value>(t)
                                && let Some(id) = v.get("view_id").and_then(Value::as_u64)
                            {
                                self.mcp.views.push(id as u32);
                            }
                        }
                    } else {
                        self.note(format!(
                            "  mcp {} answered: {}",
                            p.tool,
                            r.error.unwrap_or_default()
                        ));
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if p.sent.elapsed() > MCP_TIMEOUT {
                        self.problems.push(format!(
                            "Mica MCP call {} sent at step {} still running after {:.0} s",
                            p.tool,
                            p.step,
                            p.sent.elapsed().as_secs_f64()
                        ));
                    } else {
                        still.push(p);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.problems.push(format!(
                        "Mica MCP call {} sent at step {} was dropped without an answer",
                        p.tool, p.step
                    ));
                }
            }
        }
        self.mcp.pending = still;
        if self.mcp.views.len() > 16 {
            self.mcp.views.drain(..8);
        }
    }
}
