// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica MCP live mode end to end in one process: a headless window (`App`) serving the live
//! socket in a temporary folder, and `mica mcp`'s own server (`MicaMcp::call`, the code path
//! the stdio server runs) talking to it on a client thread while the test pumps the window's
//! frames. Never the user's settings folder or a real window.

use crate::app::{App, Options};
use crate::mcp_live::{Host, UNDO_PREFIX};
use eframe::egui;
use mica_cli::mcp::live::Content;
use mica_cli::mcp::{MicaMcp, Mode};
use mica_profiles::user::Store;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
const XL: &str = "Original Prusa XL - 5T Input Shaper 0.4 nozzle";

fn model(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/models/")).join(name)
}

/// A temporary folder, removed on drop.
struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        // Short: a Unix socket path must stay under about 100 bytes.
        let p = std::env::temp_dir().join(format!(
            "mml-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A headless window on the XL with its live socket, and a `mica mcp` server for it.
struct Live {
    app: App,
    ctx: egui::Context,
    server: Option<MicaMcp>,
    dir: Temp,
    in_process: Option<std::sync::mpsc::Sender<crate::mcp_live::Incoming>>,
}

impl Live {
    fn new(models: &[&str]) -> Self {
        Self::with_transport(models, false)
    }

    fn with_transport(models: &[&str], in_process: bool) -> Self {
        let dir = Temp::new("w");
        let ctx = egui::Context::default();
        let mut app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir.0.join("profiles"))),
                files: vec![],
                headless: true,
            },
        );
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == XL)
            .expect("the XL preset");
        app.switch_printer(vi, &name);
        if !models.is_empty() {
            let paths = models.iter().map(|m| model(m)).collect();
            let loaded = crate::app::load_files(paths, &app.config, app.bed.center());
            app.finish_load(loaded);
        }
        let socket = dir.0.join("mcp.sock");
        let in_process = if in_process {
            let (host, tx) = Host::in_process();
            app.mcp = host;
            Some(tx)
        } else {
            app.mcp = Host::start(&socket, &ctx).expect("the live socket");
            None
        };
        let server = MicaMcp::with_mode(Mode::Live, socket);
        Self {
            app,
            ctx,
            server: Some(server),
            dir,
            in_process,
        }
    }

    fn socket(&self) -> PathBuf {
        self.dir.0.join("mcp.sock")
    }

    /// One tool call through `mica mcp`'s server on a client thread, the window's frames
    /// pumped meanwhile.
    fn call(&mut self, tool: &str, args: Value) -> Result<Vec<Content>, String> {
        if let Some(tx) = &self.in_process {
            use mica_cli::mcp::live::{PROTOCOL, Request};
            let (reply, answer) = std::sync::mpsc::channel();
            tx.send(crate::mcp_live::Incoming {
                request: Request {
                    v: PROTOCOL,
                    id: 1,
                    tool: tool.into(),
                    args,
                },
                reply,
            })
            .unwrap();
            let start = Instant::now();
            loop {
                self.app.poll(&self.ctx);
                self.app.poll_mcp(&self.ctx);
                if let Ok(response) = answer.try_recv() {
                    return if response.ok {
                        Ok(response.content)
                    } else {
                        Err(response.error.unwrap())
                    };
                }
                assert!(start.elapsed() < Duration::from_secs(300), "the call hangs");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let server = self.server.take().expect("server");
        let tool = tool.to_string();
        let worker = std::thread::spawn(move || {
            let r = server.call(&tool, args);
            (server, r)
        });
        let t0 = Instant::now();
        while !worker.is_finished() {
            self.app.poll(&self.ctx);
            self.app.poll_mcp(&self.ctx);
            assert!(t0.elapsed() < Duration::from_secs(300), "the call hangs");
            std::thread::sleep(Duration::from_millis(2));
        }
        let (server, r) = worker.join().expect("the client thread");
        self.server = Some(server);
        r
    }

    fn json(&mut self, tool: &str, args: Value) -> Value {
        let r = self
            .call(tool, args)
            .unwrap_or_else(|e| panic!("{tool}: {e}"));
        last_json(&r)
    }

    fn err(&mut self, tool: &str, args: Value) -> String {
        match self.call(tool, args) {
            Ok(r) => panic!("{tool} should fail: {r:?}"),
            Err(e) => e,
        }
    }
}

fn last_json(r: &[Content]) -> Value {
    let t = r
        .iter()
        .rev()
        .find_map(Content::as_text)
        .expect("a text block");
    serde_json::from_str(t).expect("JSON")
}

/// The headless server's answer to the same call, for comparison.
fn headless(server: &MicaMcp, tool: &str, args: Value) -> Value {
    last_json(
        &server
            .call(tool, args)
            .unwrap_or_else(|e| panic!("headless {tool}: {e}")),
    )
}

/// The center pixel of part 1 of object 1 in a render's info.
fn center_px(info: &Value) -> (u64, u64) {
    let parts = info["visible_parts"].as_array().expect("visible parts");
    let c = &parts[0]["center_px"];
    (c[0].as_u64().unwrap(), c[1].as_u64().unwrap())
}

#[cfg(unix)]
#[test]
fn live_paint_matches_headless_and_is_one_undo_step() {
    let mut live = Live::new(&["cube20.stl"]);
    // Status: live, the window's project.
    let status = live.json("status", Value::Null);
    assert_eq!(status["mode"], "live", "{status}");
    assert_eq!(status["window"]["objects"], 1, "{status}");
    assert_eq!(status["window"]["tools"], 5, "{status}");
    let info = live.json("scene_info", Value::Null);
    assert_eq!(info["printer"]["tools"], 5, "{info}");
    assert_eq!(info["objects"][0]["triangles"], 12, "{info}");
    assert!(info["mode"].as_str().unwrap().starts_with("live"));
    // The same model opened headless on the same printer.
    let solo = MicaMcp::with_mode(Mode::Headless, live.socket());
    let solo_info = headless(
        &solo,
        "open",
        json!({"path": model("cube20.stl"), "printer": "XL"}),
    );
    assert_eq!(
        info["objects"][0]["bbox_min_mm"], solo_info["objects"][0]["bbox_min_mm"],
        "same placement: {info} / {solo_info}"
    );
    // Render the same view in both and paint the same pixel.
    let view = json!({"view": "iso", "size": 256});
    let r = live.call("render_view", view.clone()).unwrap();
    assert!(matches!(r[0], Content::Image { .. }));
    let live_view = last_json(&r);
    let solo_view = headless(&solo, "render_view", view);
    assert_eq!(
        live_view["visible_parts"], solo_view["visible_parts"],
        "identical renders"
    );
    let (x, y) = center_px(&live_view);
    let paint = |id: &Value| json!({"view_id": id, "x": x, "y": y, "tool": 3});
    let live_paint = live.json("paint_at", paint(&live_view["view_id"]));
    let solo_paint = headless(&solo, "paint_at", paint(&solo_view["view_id"]));
    for key in [
        "triangles_painted",
        "area_painted_mm2",
        "object_area_by_tool_pct",
        "hit",
    ] {
        assert_eq!(
            live_paint[key], solo_paint[key],
            "{key}: {live_paint} / {solo_paint}"
        );
    }
    assert!(live_paint["area_painted_mm2"].as_f64().unwrap() > 100.);
    // The window has it: painting on the object, one undo step named for Mica MCP.
    let o = &live.app.plates.current().objects[0];
    assert!(!o.painting.material.is_empty());
    assert!(o.painting_edited);
    assert_eq!(
        live.app.undo.undo_label(),
        Some(format!("{UNDO_PREFIX}paint").as_str())
    );
    assert_eq!(live_paint["undo_steps"], "the window's Edit › Undo");
    // A second render on the kept scene shows the paint (tool 3 on the hit face).
    let again = live.json("render_view", json!({"view": "iso", "size": 256}));
    assert!(again["view_id"].as_u64() > live_view["view_id"].as_u64());
    // Edit › Undo in the window takes it away, as for the user's own strokes.
    live.app.undo();
    assert!(
        live.app.plates.current().objects[0]
            .painting
            .material
            .is_empty()
    );
    // The scene follows the window: a new sync, and the next view id never repeats.
    let synced = live.json("render_view", json!({"view": "iso", "size": 256}));
    assert!(synced["view_id"].as_u64() > again["view_id"].as_u64());
    let pct = live.json("scene_info", Value::Null)["objects"][0]["area_by_tool"].clone();
    assert!(!pct.to_string().contains("\"tool\":3"), "{pct}");
    // Paint again, then the undo tool undoes it in the window.
    let id = synced["view_id"].clone();
    live.json("paint_at", paint(&id));
    let undone = live.json("undo", Value::Null);
    assert_eq!(undone["undone"], format!("{UNDO_PREFIX}paint"));
    assert!(
        live.app.plates.current().objects[0]
            .painting
            .material
            .is_empty()
    );
    // The user's own step is never undone by the AI.
    live.app.record("Move");
    live.app.plates.current_mut().objects[0].position.x += 5.;
    live.app.changed();
    let e = live.err("undo", Value::Null);
    assert!(e.contains("user's own ('Move')"), "{e}");
    // Tools and colors go to the window too.
    live.json("set_part_tool", json!({"tool": 2}));
    assert_eq!(
        live.app.plates.current().objects[0].tools.0[0],
        Some(1),
        "tool 2 (zero based 1)"
    );
    live.json("set_filament_colors", json!({"colors": ["white", "red"]}));
    assert_eq!(live.app.tool_colours()[1], [0xD3, 0x2F, 0x2F]);
    assert_eq!(
        live.app.undo.undo_label(),
        Some(format!("{UNDO_PREFIX}filament colors").as_str())
    );
    // Errors come back as tool errors (and show by the indicator).
    let e = live.err(
        "paint_at",
        json!({"view_id": 9999, "x": 1, "y": 1, "tool": 2}),
    );
    assert!(e.contains("view 9999 is not available"), "{e}");
    assert!(live.app.mcp.last_error.is_some());
}

#[cfg(unix)]
#[test]
fn live_open_and_save_protect_the_users_project() {
    let mut live = Live::new(&["cube20.stl"]);
    // An imported model that was never saved counts as unsaved work.
    let e = live.err("open", json!({"path": model("cube20.stl")}));
    assert!(e.contains("unsaved changes"), "{e}");
    assert_eq!(live.app.plates.current().objects.len(), 1);
    // Saving a copy leaves the window's document alone.
    let out = Temp::new("s");
    let copy = out.0.join("copy.3mf");
    let saved = live.json("save_project", json!({"path": copy}));
    assert_eq!(saved["saved"], copy.display().to_string());
    assert!(copy.is_file());
    assert!(live.app.document.path.is_none());
    // The copy exists now: replacing it needs overwrite.
    let e = live.err("save_project", json!({"path": copy}));
    assert!(e.contains("overwrite"), "{e}");
    // Make the copy the window's document (as Save As would), then saving over it needs
    // the explicit confirmation.
    live.app
        .save_project_to(&copy, mica_core::threemf::Flavour::Prusa)
        .unwrap();
    let e = live.err("save_project", json!({"path": copy, "overwrite": true}));
    assert!(e.contains("replace_open_project"), "{e}");
    let saved = live.json(
        "save_project",
        json!({"path": copy, "overwrite": true, "replace_open_project": true}),
    );
    assert!(saved["note"].as_str().unwrap().contains("in place"));
    // Nothing unsaved now: open replaces the project with the new model.
    let opened = live.json("open", json!({"path": model("taper.stl")}));
    assert_eq!(live.app.plates.current().objects.len(), 1);
    assert!(
        opened["objects"][0]["name"]
            .as_str()
            .unwrap()
            .contains("taper"),
        "{opened}"
    );
    // A file that does not exist.
    let e = live.err("open", json!({"path": "/no/such/model.stl"}));
    assert!(e.contains("no such file"), "{e}");
}

#[cfg(unix)]
#[test]
fn a_user_edit_during_a_call_is_never_overwritten() {
    let mut live = Live::new(&["cube20.stl"]);
    let view = live.json("render_view", json!({"view": "iso", "size": 256}));
    let (x, y) = center_px(&view);
    // The user moves the object after the render: the scene is synced again, the old view
    // id is unknown and nothing is painted.
    live.app.record("Move");
    live.app.plates.current_mut().objects[0].position.x += 10.;
    live.app.changed();
    let e = live.err(
        "paint_at",
        json!({"view_id": view["view_id"], "x": x, "y": y, "tool": 2}),
    );
    assert!(e.contains("not available"), "{e}");
    assert!(
        live.app.plates.current().objects[0]
            .painting
            .material
            .is_empty()
    );
    assert_eq!(live.app.undo.undo_label(), Some("Move"));
}

#[cfg(unix)]
#[test]
fn without_a_window_auto_mode_works_headless_and_live_mode_says_why() {
    let dir = Temp::new("n");
    let socket = dir.0.join("mcp.sock");
    let auto = MicaMcp::with_mode(Mode::Auto, socket.clone());
    let status = last_json(&auto.call("status", Value::Null).unwrap());
    assert_eq!(status["mode"], "headless", "{status}");
    assert_eq!(status["window_available"], false);
    let info = last_json(
        &auto
            .call(
                "open",
                json!({"path": model("cube20.stl"), "printer": "XL"}),
            )
            .unwrap(),
    );
    assert_eq!(info["printer"]["tools"], 5);
    let live = MicaMcp::with_mode(Mode::Live, socket.clone());
    let e = live.call("scene_info", Value::Null).unwrap_err();
    assert!(e.contains("no Mica window answers"), "{e}");
    // A stale socket file (a crashed window) is no window either.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let e = live.call("scene_info", Value::Null).unwrap_err();
    assert!(e.contains("no Mica window answers"), "{e}");
    // A window starting later replaces the stale file and is used from then on.
    let ctx = egui::Context::default();
    let host = Host::start(&socket, &ctx).expect("replaces the stale socket");
    assert!(socket.exists());
    drop(host);
    assert!(!socket.exists(), "the socket goes with the window");
}

#[cfg(unix)]
#[test]
fn painted_projects_keep_their_paint_and_take_more() {
    // A PrusaSlicer project with color paint: the window's painting maps both ways, so a live
    // edit adds to it instead of being refused or replacing it.
    let mut live = Live::new(&["painted-cube.3mf"]);
    let before = live.app.plates.current().objects[0]
        .painting
        .material
        .clone();
    assert!(!before.is_empty(), "the project is painted");
    let info = live.json("scene_info", Value::Null);
    assert_eq!(info["objects"][0]["parts"][0]["painted"], true, "{info}");
    let view = live.json("render_view", json!({"view": "top", "size": 256}));
    let (x, y) = center_px(&view);
    let r = live.json(
        "paint_at",
        json!({"view_id": view["view_id"], "x": x, "y": y, "tool": 4, "mode": "triangle"}),
    );
    assert_eq!(r["changed"], true, "{r}");
    assert_ne!(
        live.app.plates.current().objects[0].painting.material,
        before
    );
    // The headless server reads the window's saved copy the same way.
    let out = Temp::new("p");
    let copy = out.0.join("painted.3mf");
    live.json("save_project", json!({"path": copy}));
    let solo = MicaMcp::with_mode(Mode::Headless, live.socket());
    let solo_info = headless(&solo, "open", json!({"path": copy}));
    let live_info = live.json("scene_info", Value::Null);
    assert_eq!(
        solo_info["objects"][0]["area_by_tool"], live_info["objects"][0]["area_by_tool"],
        "{solo_info} / {live_info}"
    );
}

#[cfg(unix)]
#[test]
fn live_slice_summary_is_the_headless_one_and_the_window_slices_too() {
    let mut live = Live::new(&["cube20.stl"]);
    live.json(
        "paint_height_band",
        json!({"z_min": 10, "z_max": 20, "tool": 2}),
    );
    let out = Temp::new("c");
    let copy = out.0.join("band.3mf");
    live.json("save_project", json!({"path": copy}));
    let summary = live.json("slice_summary", Value::Null);
    // The window started its own slice of the plate for the user to see.
    assert!(
        live.app.worker.is_some() || live.app.finished().is_some(),
        "the window slices too"
    );
    live.app.stop_worker();
    let solo = MicaMcp::with_mode(Mode::Headless, live.socket());
    headless(&solo, "open", json!({"path": copy}));
    let solo_summary = headless(&solo, "slice_summary", Value::Null);
    for key in [
        "layers",
        "tool_changes",
        "estimated_time_s",
        "filament_by_tool",
    ] {
        assert_eq!(summary[key], solo_summary[key], "{key}");
    }
    assert!(summary["tool_changes"].as_u64().unwrap() > 0, "{summary}");
}

#[cfg(unix)]
#[test]
fn objects_on_every_plate_map_back_to_their_plate() {
    if !mica_testkit::require_orca_profiles() {
        return;
    }
    // An Orca project for the U1 with two plates (and an object beside them).
    let mut live = Live::new(&["plates-orca.3mf"]);
    let info = live.json("scene_info", Value::Null);
    let objects = info["objects"].as_array().expect("objects").clone();
    let total: usize = live.app.plates.plates.iter().map(|p| p.objects.len()).sum();
    assert_eq!(objects.len(), total, "{info}");
    let lid = objects
        .iter()
        .find(|o| o["name"] == "lid")
        .and_then(|o| o["object"].as_u64())
        .expect("the lid");
    live.json("set_part_tool", json!({"object": lid, "tool": 4}));
    let (plate, index) = live
        .app
        .plates
        .plates
        .iter()
        .enumerate()
        .find_map(|(p, plate)| {
            plate
                .objects
                .iter()
                .position(|o| o.name == "lid")
                .map(|i| (p, i))
        })
        .expect("the lid in the window");
    assert_eq!(
        live.app.plates.plates[plate].objects[index].tools.0[0],
        Some(3)
    );
    // Only the lid changed.
    let again = live.json("scene_info", Value::Null);
    for (a, b) in objects.iter().zip(again["objects"].as_array().unwrap()) {
        if a["name"] == "lid" {
            assert_eq!(b["tool"], 4, "{b}");
        } else {
            assert_eq!(a["tool"], b["tool"], "{a} / {b}");
        }
    }
}

#[cfg(unix)]
#[test]
fn live_add_shape_puts_shapes_in_the_window_as_undo_steps() {
    let mut live = Live::new(&["cube20.stl"]);
    let v = live.json("add_shape", json!({"shape": "cylinder"}));
    assert_eq!(v["added"], "Cylinder", "{v}");
    assert_eq!(v["object"], 2, "{v}");
    assert_eq!(live.app.plates.current().objects.len(), 2);
    assert_eq!(
        live.app.undo.undo_label(),
        Some(format!("{UNDO_PREFIX}add shape").as_str())
    );
    // A negative volume in the cube; object 1 is the cube.
    let v = live.json(
        "add_shape",
        json!({"shape": "cube", "kind": "negative_volume", "object": 1, "size": 8}),
    );
    assert_eq!(v["object"], 1, "{v}");
    let cube = &live.app.plates.current().objects[0];
    assert_eq!(cube.volumes.len(), 1);
    assert_eq!(
        cube.volumes[0].kind,
        mica_core::threemf::PartKind::NegativeVolume
    );
    // scene_info sees both (the scene syncs with the window).
    let info = live.json("scene_info", Value::Null);
    assert_eq!(info["objects"].as_array().unwrap().len(), 2, "{info}");
    assert!(
        live.err(
            "add_shape",
            json!({"shape": "cube", "kind": "part", "object": 9})
        )
        .contains("does not exist")
    );
    // Undo through Mica MCP takes them away again.
    live.json("undo", Value::Null);
    live.json("undo", Value::Null);
    assert_eq!(live.app.plates.current().objects.len(), 1);
    assert!(live.app.plates.current().objects[0].volumes.is_empty());
}

#[cfg(unix)]
#[test]
fn live_create_mix_is_one_undo_step_and_matches_gui_project_and_slice() {
    use crate::mixes::{Draft, MixAct};
    use mica_core::full_spectrum::Pattern;
    let mut live = Live::with_transport(
        &["cube20.stl"],
        std::env::var_os("MICA_MCP_TEST_IN_PROCESS").is_some(),
    );
    let out = Temp::new("mix");
    live.json("set_filament_colors", json!({"colors": ["red", "yellow"]}));
    let base = out.0.join("base.3mf");
    live.json("save_project", json!({"path": base}));
    let solo = MicaMcp::with_mode(Mode::Headless, out.0.join("none.sock"));
    headless(&solo, "open", json!({"path": base}));
    for pattern in Pattern::ALL {
        let args = json!({"target": "orange", "tools": [1, 2], "pattern": pattern.key(), "name": "Orange mix"});
        let created = live.json("create_mix", args.clone());
        let headless_created = headless(&solo, "create_mix", args.clone());
        for key in [
            "tool",
            "components",
            "predicted_color",
            "delta_e",
            "pattern",
            "end_ratios",
        ] {
            assert_eq!(created[key], headless_created[key], "{key}");
        }
        assert_eq!(live.app.mixes.list.len(), 1);
        assert_eq!(live.app.undo.undo_label(), Some("Mica MCP: create mix"));
        let label = live.app.undo.undo_label().unwrap().to_string();
        let duplicate = live.json("create_mix", args);
        assert_eq!(duplicate["existing"], true);
        assert_eq!(duplicate["changed"], false);
        assert_eq!(live.app.undo.undo_label(), Some(label.as_str()));
        // Headless and live both save the identical ColorMix archive entry.
        let headless_file = out.0.join(format!("headless-{}.3mf", pattern.key()));
        headless(&solo, "save_project", json!({"path": headless_file}));
        let saved = mica_core::threemf::load_3mf(&headless_file).unwrap();
        assert_eq!(saved.full_spectrum_json, live.app.mixes_json());
        let mcp_json = live.app.mixes_json();
        // Undo removes exactly this one mix, leaving the filament colors in place.
        live.json("undo", Value::Null);
        assert!(live.app.mixes.list.is_empty());
        assert_eq!(
            live.app.tool_colours()[0],
            mica_cli::mcp::colors::parse("red").unwrap()
        );
        // Use the GUI's real Draft/Add path for the same recipe.
        let components = created["components"].as_array().unwrap();
        let percents: Vec<f64> = components
            .iter()
            .map(|c| c["ratio"].as_f64().unwrap() * 100.)
            .collect();
        let end = created["end_ratios"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_f64().unwrap() * 100.)
            .collect();
        live.app.apply_mix_acts(vec![MixAct::Save(Draft {
            editing: None,
            tools: components
                .iter()
                .map(|c| c["tool"].as_u64().unwrap() as usize - 1)
                .collect(),
            percents,
            end,
            pattern,
            name: "Orange mix".into(),
            named: true,
        })]);
        assert_eq!(
            live.app.mixes_json(),
            mcp_json,
            "GUI mix section bytes: {pattern:?}"
        );
        // Both assignments use the same virtual number, and the GUI worker's G-code is
        // byte-identical to a CLI slice of the MCP-created project (fixed clock in gates).
        live.app.plates.current_mut().objects[0].set_tool(5);
        headless(&solo, "set_part_tool", json!({"tool": 6}));
        let assigned = out.0.join(format!("assigned-{}.3mf", pattern.key()));
        headless(&solo, "save_project", json!({"path": assigned}));
        let inputs = live.app.slice_inputs(0).unwrap();
        let crate::slice_cache::Job::Objects(objects, custom) = inputs.job else {
            panic!("object job");
        };
        let sliced = crate::worker::slice_with_custom_gcode(
            objects,
            custom,
            inputs.config,
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .unwrap();
        let gui = std::fs::read(sliced.preview.job.path.join("print.gcode")).unwrap();
        let gcode = out.0.join(format!("{}.gcode", pattern.key()));
        mica_cli::run(&[
            "slice".into(),
            assigned.display().to_string(),
            "-o".into(),
            gcode.display().to_string(),
        ])
        .unwrap();
        let cli = std::fs::read(&gcode).unwrap();
        // Normal tests may cross a wall-clock second; the generated timestamp is the only
        // permitted difference. With SOURCE_DATE_EPOCH set this checks every byte.
        if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
            assert_eq!(gui, cli, "GUI/CLI G-code bytes: {pattern:?}");
        } else {
            let lines = |b: &[u8]| {
                String::from_utf8_lossy(b)
                    .lines()
                    .filter(|l| !l.starts_with("; generated by "))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            assert_eq!(lines(&gui), lines(&cli), "GUI/CLI G-code: {pattern:?}");
        }
        mica_cli::run(&[
            "validate".into(),
            gcode.display().to_string(),
            "--printer".into(),
            XL.into(),
        ])
        .unwrap();
        // Reset assignment and recipe for the next pattern.
        live.app.plates.current_mut().objects[0].set_tool(0);
        live.app.undo();
        headless(&solo, "undo", Value::Null);
        headless(&solo, "undo", Value::Null);
    }
    live.app.prefs.features.color_mix = false;
    assert!(
        live.err("create_mix", json!({"target": "orange"}))
            .contains("Settings › Features")
    );
}
