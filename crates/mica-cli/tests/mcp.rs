// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `mica mcp` end to end through rmcp's own client over an in-process duplex pipe: open the
//! Benchy for the U1, render, paint hull, cabin and chimney by pointing at pixels, check the
//! area per tool, slice, save a 3MF and reopen it. Set `MICA_MCP_RENDER_DIR` to keep the
//! before/after renders.

use mica_cli::mcp::{MicaMcp, render::Options, session::Profiles};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn model(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/models")
        .join(name)
}

/// The pinned Orca profiles with the Snapmaker U1 (a reported skip without them).
fn u1_available() -> bool {
    mica_testkit::require_orca_profiles()
}

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mica-mcp-test-{tag}-{}", std::process::id()));
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

async fn connect() -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(service) = MicaMcp::new().serve(server_io).await {
            let _ = service.waiting().await;
        }
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    client.call_tool(params).await.expect("transport works")
}

fn text_of(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The JSON of a successful call.
fn ok(r: &CallToolResult) -> Value {
    assert_ne!(r.is_error, Some(true), "tool failed: {}", text_of(r));
    let t = r
        .content
        .iter()
        .rev()
        .find_map(|c| c.as_text())
        .expect("a text result");
    serde_json::from_str(&t.text).expect("JSON text")
}

fn err(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "expected an error: {}", text_of(r));
    text_of(r)
}

fn png_of(r: &CallToolResult) -> Vec<u8> {
    let img = r
        .content
        .iter()
        .find_map(|c| c.as_image())
        .expect("an image");
    assert_eq!(img.mime_type, "image/png");
    decode64(&img.data)
}

fn decode64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
    let mut out = vec![];
    for chunk in bytes.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &c)| n | (u32::from(val(c)) << (18 - 6 * i)));
        for k in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * k)) as u8);
        }
    }
    out
}

fn keep(name: &str, png: &[u8]) {
    if let Some(dir) = std::env::var_os("MICA_MCP_RENDER_DIR") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(name), png).unwrap();
    }
}

fn tool_pct(info: &Value, tool: u64) -> f64 {
    info["objects"][0]["area_by_tool"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == tool)
        .map_or(0., |t| t["area_pct"].as_f64().unwrap())
}

/// Pixels of the iso view (as the server renders it) whose surface point satisfies `want`;
/// returns the one nearest the middle of them.
fn pixel_where(
    scene: &mica_cli::mcp::session::Scene,
    view: u32,
    want: impl Fn([f32; 3], [f32; 3]) -> bool,
) -> (u32, u32) {
    let mut hits = vec![];
    for y in (0..768).step_by(2) {
        for x in (0..768).step_by(2) {
            if let Some((p, n)) = scene.pixel_point(view, x, y)
                && want(p, n)
            {
                hits.push((x, y));
            }
        }
    }
    assert!(!hits.is_empty(), "no pixel matches");
    let n = hits.len() as f64;
    let cx = hits.iter().map(|h| f64::from(h.0)).sum::<f64>() / n;
    let cy = hits.iter().map(|h| f64::from(h.1)).sum::<f64>() / n;
    *hits
        .iter()
        .min_by(|a, b| {
            let d = |h: &&(u32, u32)| (f64::from(h.0) - cx).powi(2) + (f64::from(h.1) - cy).powi(2);
            d(a).total_cmp(&d(b))
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_paints_the_benchy_for_the_u1() {
    if !u1_available() {
        return;
    }
    let tmp = Temp::new("benchy");
    let benchy = model("3DBenchy.stl");
    let before_bytes = std::fs::read(&benchy).unwrap();
    let client = connect().await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for want in [
        "open",
        "scene_info",
        "render_view",
        "paint_at",
        "pick",
        "paint_height_band",
        "paint_by_direction",
        "set_part_tool",
        "set_filament_colors",
        "clear_paint",
        "undo",
        "slice_summary",
        "save_project",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }

    // Errors are tool results with a message, not protocol failures.
    assert!(err(&call(&client, "scene_info", json!({})).await).contains("no model is open"));
    assert!(
        err(&call(&client, "open", json!({"path": "/no/such.stl"})).await).contains("no such file")
    );

    let info = ok(&call(
        &client,
        "open",
        json!({"path": benchy, "printer": "U1",
               "filament_colors": ["white", "red", "blue", "black"]}),
    )
    .await);
    assert_eq!(info["printer"]["tools"], 4);
    assert_eq!(info["filaments"][1]["color"], "red");
    assert_eq!(info["filaments"][1]["hex"], "#D32F2F");
    assert_eq!(info["objects"][0]["triangles"], 225102);
    assert_eq!(tool_pct(&info, 1), 100.);

    let iso = json!({"view": "iso", "size": 768});
    let r = call(&client, "render_view", iso.clone()).await;
    let view = ok(&r);
    let png = png_of(&r);
    assert!(png.starts_with(b"\x89PNG"));
    assert!(png.len() < 1_000_000, "{} bytes", png.len());
    keep("benchy-before-iso.png", &png);
    let view_id = view["view_id"].as_u64().unwrap() as u32;
    assert!(!view["visible_parts"].as_array().unwrap().is_empty());

    // The same view in a scene of our own gives the facet buffer to choose pixels from (the
    // renderer is deterministic, so its pixels are the server's).
    let mut mine = Profiles {
        printer: Some("U1".into()),
        ..Default::default()
    }
    .open(&benchy)
    .unwrap();
    let opts = Options {
        highlight_paint: false,
        grid: true,
    };
    let (mine_id, _, _) = mine
        .render_view(45., 30., 768, None, 1., None, opts)
        .unwrap();
    let info_now = ok(&call(&client, "scene_info", json!({})).await);
    let lo = &info_now["objects"][0]["bbox_min_mm"];
    let hi = &info_now["objects"][0]["bbox_max_mm"];
    let (z0, z1) = (
        lo[2].as_f64().unwrap() as f32,
        hi[2].as_f64().unwrap() as f32,
    );
    let (x0, x1) = (
        lo[0].as_f64().unwrap() as f32,
        hi[0].as_f64().unwrap() as f32,
    );
    let h = z1 - z0;
    // Hull side: low on the boat; cabin wall: mid height towards the stern; chimney: top.
    let hull = pixel_where(&mine, mine_id, |p, _| {
        p[2] > z0 + 0.12 * h && p[2] < z0 + 0.22 * h
    });
    let chimney = pixel_where(&mine, mine_id, |p, _| {
        p[2] > z0 + 0.85 * h && p[2] < z0 + 0.95 * h
    });
    // The cabin's stern-side outer wall (facing +x), below its window.
    let cabin = pixel_where(&mine, mine_id, |p, n| {
        n[0] > 0.95
            && p[2] > z0 + 0.36 * h
            && p[2] < z0 + 0.46 * h
            && p[0] > x0 + 0.6 * (x1 - x0)
            && p[0] < x0 + 0.8 * (x1 - x0)
    });
    eprintln!("pixels: hull {hull:?}, cabin {cabin:?}, chimney {chimney:?}");

    // The hull is half the boat: one fill over it is refused unless asked for.
    let refused = err(&call(
        &client,
        "paint_at",
        json!({"view_id": view_id, "x": hull.0, "y": hull.1, "tool": 2}),
    )
    .await);
    assert!(
        refused.contains("would cover") && refused.contains("max_distance"),
        "{refused}"
    );
    assert_eq!(
        ok(&call(&client, "scene_info", json!({})).await)["undo_steps"],
        0
    );
    let mut painted = vec![];
    for (tool, (x, y)) in [(2, hull), (3, cabin), (4, chimney)] {
        let v = ok(&call(
            &client,
            "paint_at",
            json!({"view_id": view_id, "x": x, "y": y, "tool": tool, "mode": "smart_fill",
                   "allow_large": tool == 2}),
        )
        .await);
        assert_eq!(v["changed"], true, "{v}");
        assert!(v["area_painted_mm2"].as_f64().unwrap() > 1., "{v}");
        painted.push(v);
    }
    let info = ok(&call(&client, "scene_info", json!({})).await);
    let pct: Vec<f64> = (1..=4).map(|t| tool_pct(&info, t)).collect();
    eprintln!("area per tool after painting: {pct:?}");
    for (t, p) in pct.iter().enumerate() {
        assert!(*p > 0.5, "tool {} has {p}%: {info}", t + 1);
    }
    assert!((pct.iter().sum::<f64>() - 100.).abs() < 0.1);
    assert_eq!(info["undo_steps"], 3);

    let r = call(&client, "render_view", iso.clone()).await;
    keep("benchy-after-iso.png", &png_of(&r));
    let after = ok(&r);
    assert!(
        after["visible_tools"].as_array().unwrap().len() >= 3,
        "{after}"
    );
    let r = call(
        &client,
        "render_view",
        json!({"view": "iso_back_left", "highlight_paint": true}),
    )
    .await;
    keep("benchy-after-back-highlight.png", &png_of(&r));

    // Undo takes one step back; painting it again restores it.
    let undone = ok(&call(&client, "undo", json!({})).await);
    assert!(undone["undone"].as_str().unwrap().contains("tool 4"));
    assert_eq!(
        tool_pct(&ok(&call(&client, "scene_info", json!({})).await), 4),
        0.
    );
    ok(&call(
        &client,
        "paint_at",
        json!({"view_id": view_id, "x": chimney.0, "y": chimney.1, "tool": 4}),
    )
    .await);
    assert!(
        (tool_pct(&ok(&call(&client, "scene_info", json!({})).await), 4) - pct[3]).abs() < 1e-6
    );

    // Bad input is explained.
    assert!(
        err(&call(
            &client,
            "paint_at",
            json!({"view_id": 999, "x": 1, "y": 1, "tool": 2})
        )
        .await)
        .contains("not available")
    );
    assert!(
        err(&call(
            &client,
            "paint_at",
            json!({"view_id": view_id, "x": 1, "y": 1, "tool": 2})
        )
        .await)
        .contains("background")
    );
    assert!(
        err(&call(
            &client,
            "paint_at",
            json!({"view_id": view_id, "x": hull.0, "y": hull.1, "tool": 9})
        )
        .await)
        .contains("tools 1..=4")
    );

    let slice = ok(&call(&client, "slice_summary", json!({})).await);
    eprintln!("slice summary: {slice}");
    let used = slice["filament_by_tool"].as_array().unwrap();
    assert!(used.len() >= 3, "{slice}");
    assert!(slice["tool_changes"].as_u64().unwrap() > 10, "{slice}");
    assert!(slice["layers"].as_u64().unwrap() > 200);
    assert_eq!(slice["gcode_check"]["errors"], 0, "{slice}");
    // What the colors cost against the same model in one color.
    let one = slice["single_color"]["estimated_time_s"].as_f64().unwrap();
    let all = slice["estimated_time_s"].as_f64().unwrap();
    assert!(one > 60. && one < all, "{slice}");
    let cost = &slice["color_cost"];
    assert!(
        (cost["extra_time_s"].as_f64().unwrap() - (all - one)).abs() <= 1.,
        "{slice}"
    );
    assert!(cost["extra_filament_g"].as_f64().unwrap() > 0., "{slice}");
    assert!(
        slice["purge_and_tower_filament_g"].as_f64().unwrap() > 0.,
        "{slice}"
    );
    assert!(
        slice["advice"].as_str().unwrap().contains("tool changes"),
        "{slice}"
    );

    let out = tmp.0.join("benchy-painted.3mf");
    assert!(err(&call(&client, "save_project", json!({"path": benchy})).await).contains("3mf"));
    // Never into Mica's source folder (the test models live there).
    let in_repo = model("benchy-painted-by-a-test.3mf");
    assert!(
        err(&call(&client, "save_project", json!({"path": in_repo})).await)
            .contains("source checkout")
    );
    assert!(!in_repo.exists());
    ok(&call(&client, "save_project", json!({"path": out})).await);
    assert!(err(&call(&client, "save_project", json!({"path": out})).await).contains("exists"));
    assert_eq!(
        std::fs::read(&benchy).unwrap(),
        before_bytes,
        "the input is untouched"
    );

    // Reopen the saved project: printer, colors and paint come back.
    let reopened = ok(&call(&client, "open", json!({"path": out})).await);
    assert_eq!(reopened["printer"]["tools"], 4, "{reopened}");
    assert_eq!(reopened["filaments"][2]["color"], "blue");
    for t in 1..=4 {
        let again = tool_pct(&reopened, t);
        assert!(
            (again - pct[t as usize - 1]).abs() < 0.05,
            "tool {t}: {again} vs {pct:?}"
        );
    }
    let r = call(&client, "render_view", iso).await;
    keep("benchy-reopened-iso.png", &png_of(&r));
    client.cancel().await.unwrap();
}

/// A fill on the Benchy's smooth hull runs over the whole hull; `max_distance` keeps it to a
/// patch around the point, and the reply to a refused fill suggests a limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fill_spread_limit_on_the_benchy_hull() {
    if !u1_available() {
        return;
    }
    let benchy = model("3DBenchy.stl");
    let client = connect().await;
    ok(&call(
        &client,
        "open",
        json!({"path": benchy, "printer": "U1",
               "filament_colors": ["white", "red", "blue", "black"]}),
    )
    .await);
    let view = ok(&call(&client, "render_view", json!({"view": "iso"})).await);
    let view_id = view["view_id"].as_u64().unwrap() as u32;
    let mut mine = Profiles {
        printer: Some("U1".into()),
        ..Default::default()
    }
    .open(&benchy)
    .unwrap();
    let opts = Options {
        highlight_paint: false,
        grid: true,
    };
    let (mine_id, _, _) = mine
        .render_view(45., 30., 768, None, 1., None, opts)
        .unwrap();
    let info = ok(&call(&client, "scene_info", json!({})).await);
    let z0 = info["objects"][0]["bbox_min_mm"][2].as_f64().unwrap() as f32;
    let h = info["objects"][0]["bbox_max_mm"][2].as_f64().unwrap() as f32 - z0;
    // The hull's side (not the deck), low on the boat.
    let hull = pixel_where(&mine, mine_id, |p, n| {
        p[2] > z0 + 0.1 * h && p[2] < z0 + 0.2 * h && n[2].abs() < 0.5
    });
    let at = |extra: Value| {
        let mut args = json!({"view_id": view_id, "x": hull.0, "y": hull.1, "tool": 2});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };
    // Unlimited: refused, with the area it would cover and a suggested limit.
    let refused = err(&call(&client, "paint_at", at(json!({}))).await);
    eprintln!("{refused}");
    assert!(refused.contains("would cover"), "{refused}");
    assert!(refused.contains("suggested:"), "{refused}");
    // The same click with a limit paints a patch around the point.
    let limited = ok(&call(&client, "paint_at", at(json!({"max_distance": 12}))).await);
    assert_eq!(limited["max_distance"], 12., "{limited}");
    let patch = limited["area_painted_mm2"].as_f64().unwrap();
    // At most a 12 mm disc (452 mm²) where the surface is flat; sharp edges near the point
    // (the deck's rim) cut it shorter.
    assert!(patch > 100. && patch < 480., "{limited}");
    let r = call(&client, "render_view", json!({"view": "iso"})).await;
    keep("benchy-hull-fill-limited-12mm.png", &png_of(&r));
    ok(&call(&client, "undo", json!({})).await);
    let whole = ok(&call(&client, "paint_at", at(json!({"allow_large": true}))).await);
    let full = whole["area_painted_mm2"].as_f64().unwrap();
    assert!(full > 5. * patch, "{whole}");
    let r = call(&client, "render_view", json!({"view": "iso"})).await;
    keep("benchy-hull-fill-unlimited.png", &png_of(&r));
    eprintln!("hull fill: limited {patch} mm², unlimited {full} mm²");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bands_directions_tools_and_clearing() {
    if !u1_available() {
        return;
    }
    let client = connect().await;
    let cube = model("cube20.stl");
    ok(&call(&client, "open", json!({"path": cube, "printer": "U1"})).await);
    // A band through the middle third of the cube's sides: 1/3 of the 4 sides = 4/18 of the area.
    let v = ok(&call(
        &client,
        "paint_height_band",
        json!({"z_min": 20./3., "z_max": 40./3., "tool": 2}),
    )
    .await);
    assert!(
        (v["area_painted_mm2"].as_f64().unwrap() - 4. * 20. * 20. / 3.).abs() < 8.,
        "{v}"
    );
    // The top face, by direction.
    let v = ok(&call(
        &client,
        "paint_by_direction",
        json!({"direction": "up", "tool": 3}),
    )
    .await);
    assert!(
        (v["area_painted_mm2"].as_f64().unwrap() - 400.).abs() < 0.5,
        "{v}"
    );
    let info = ok(&call(&client, "scene_info", json!({})).await);
    assert!((tool_pct(&info, 2) - 100. * 4. / 18.).abs() < 0.5, "{info}");
    assert!((tool_pct(&info, 3) - 100. / 6.).abs() < 0.2, "{info}");
    // The whole object to tool 4: the unpainted rest follows.
    ok(&call(&client, "set_part_tool", json!({"tool": 4})).await);
    let info = ok(&call(&client, "scene_info", json!({})).await);
    assert_eq!(info["objects"][0]["parts"][0]["tool"], 4);
    assert!(tool_pct(&info, 1) < 1e-9);
    assert!(err(&call(&client, "set_part_tool", json!({"tool": 5})).await).contains("1..=4"));
    // Sides by direction, then clear everything; undo brings the paint back.
    ok(&call(
        &client,
        "paint_by_direction",
        json!({"direction": "side", "max_angle": 5, "tool": 1}),
    )
    .await);
    ok(&call(&client, "clear_paint", json!({})).await);
    let info = ok(&call(&client, "scene_info", json!({})).await);
    assert_eq!(tool_pct(&info, 4), 100.);
    ok(&call(&client, "undo", json!({})).await);
    assert!(tool_pct(&ok(&call(&client, "scene_info", json!({})).await), 1) > 60.);
    // Unknown colors and directions are explained.
    assert!(
        err(&call(
            &client,
            "set_filament_colors",
            json!({"colors": ["mauve-ish"]})
        )
        .await)
        .contains("#RRGGBB")
    );
    assert!(
        err(&call(
            &client,
            "paint_by_direction",
            json!({"direction": "sideways", "tool": 2})
        )
        .await)
        .contains("vector")
    );
    // A single-tool printer explains that painting needs more tools.
    ok(&call(&client, "open", json!({"path": cube})).await);
    assert!(
        err(&call(
            &client,
            "paint_by_direction",
            json!({"direction": "up", "tool": 2})
        )
        .await)
        .contains("multi-tool")
    );
    client.cancel().await.unwrap();
}

/// `add_shape` headless: a shape starts a scene, more shapes join as objects and as parts of
/// one, mistakes are explained, undo takes them away, and the saved 3MF holds plain mesh
/// objects named after the shapes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shapes_on_the_plate() {
    let client = connect().await;
    let v = ok(&call(&client, "add_shape", json!({"shape": "cube"})).await);
    assert_eq!(v["scene"]["objects"][0]["name"], "Cube", "{v}");
    let v = ok(&call(
        &client,
        "add_shape",
        json!({"shape": "Cylinder", "size": 30}),
    )
    .await);
    assert_eq!(v["object"], 2, "{v}");
    assert_eq!(v["size_mm"], json!([30., 30., 30.]));
    let v = ok(&call(
        &client,
        "add_shape",
        json!({"shape": "sphere", "kind": "modifier", "object": 1, "size": [10, 10, 10]}),
    )
    .await);
    assert_eq!(
        (v["object"].clone(), v["part"].clone()),
        (json!(1), json!(2))
    );
    let info = ok(&call(&client, "scene_info", json!({})).await);
    let objects = info["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[1]["name"], "Cylinder");
    assert_eq!(objects[0]["parts"][1]["kind"], "modifier");
    // The cylinder stands clear of the cube.
    let cube_max_x = objects[0]["bbox_min_mm"][0].as_f64().unwrap() + 20.;
    assert!(objects[1]["bbox_min_mm"][0].as_f64().unwrap() > cube_max_x);

    assert!(err(&call(&client, "add_shape", json!({"shape": "teapot"})).await).contains("benchy"));
    assert!(
        err(&call(
            &client,
            "add_shape",
            json!({"shape": "cube", "kind": "part"})
        )
        .await)
        .contains("pass object")
    );
    assert!(
        err(&call(
            &client,
            "add_shape",
            json!({"shape": "cube", "size": [1, 2]})
        )
        .await)
        .contains("[x, y, z]")
    );
    assert!(
        err(&call(
            &client,
            "add_shape",
            json!({"shape": "cube", "kind": "hole", "object": 1, "position": [10, 10]})
        )
        .await)
        .contains("new objects")
    );

    let dir = Temp::new("shapes");
    let out = dir.0.join("shapes.3mf");
    ok(&call(&client, "save_project", json!({"path": out})).await);
    let saved = mica_core::threemf::load_3mf(&out).unwrap();
    let names: Vec<&str> = saved.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Cube", "Cylinder"]);
    assert_eq!(
        saved.objects[0].parts[1].kind,
        mica_core::threemf::PartKind::Modifier
    );

    let undone = ok(&call(&client, "undo", json!({})).await);
    assert!(
        undone["undone"].as_str().unwrap().contains("sphere"),
        "{undone}"
    );
    ok(&call(&client, "undo", json!({})).await);
    let info = ok(&call(&client, "scene_info", json!({})).await);
    assert_eq!(info["objects"].as_array().unwrap().len(), 1);
    assert_eq!(info["objects"][0]["parts"].as_array().unwrap().len(), 1);
    // The Benchy, at half size.
    let v = ok(&call(&client, "add_shape", json!({"shape": "benchy", "size": 30})).await);
    assert_eq!(v["added"], "3DBenchy");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn create_mix_matches_reuses_paints_and_round_trips() {
    let dir = Temp::new("mix");
    let client = connect().await;
    let opened = ok(&call(
        &client,
        "open",
        json!({
            "path": model("cube20.stl"),
            "printer": "Original Prusa XL - 2T Input Shaper 0.4 nozzle",
            "print": "0.20mm SPEED @XLIS 0.4", "filament": "Prusament PLA @XLIS",
            "filament_colors": ["#FF0000", "#FFFF00"],
        }),
    )
    .await);
    assert_eq!(opened["printer"]["tools"], 2);
    let listed = client.list_all_tools().await.unwrap();
    assert!(listed.iter().any(|t| t.name == "create_mix"));
    let mix = ok(&call(&client, "create_mix", json!({"target": "orange"})).await);
    assert_eq!(mix["tool"], 3, "{mix}");
    assert!(mix["delta_e"].as_f64().unwrap() < 6., "{mix}");
    assert_eq!(mix["changed"], true);
    assert_eq!(mix["existing"], false);
    assert!(mix["predicted_color"]["name"].is_string());
    assert!(mix["predicted_color"]["hex"].is_string());
    let duplicate = ok(&call(
        &client,
        "create_mix",
        json!({
            "target": "orange", "tools": [2, 1, 1], "name": "Another name",
        }),
    )
    .await);
    assert_eq!(duplicate["tool"], mix["tool"]);
    assert_eq!(duplicate["existing"], true);
    assert_eq!(duplicate["changed"], false);
    assert_eq!(duplicate["undo_steps"], mix["undo_steps"]);
    ok(&call(&client, "set_part_tool", json!({"tool": 3})).await);
    ok(&call(
        &client,
        "paint_height_band",
        json!({"z_min": 5, "z_max": 10, "tool": 3}),
    )
    .await);
    let view = ok(&call(&client, "render_view", json!({"size": 128})).await);
    assert!(view["view_id"].is_number());
    let info = ok(&call(&client, "scene_info", Value::Null).await);
    assert_eq!(info["filaments"].as_array().unwrap().len(), 2);
    assert_eq!(info["mixes"][0]["tool"], 3);
    assert_eq!(info["objects"][0]["area_by_tool"][0]["tool"], 3);
    let path = dir.0.join("mixed.3mf");
    ok(&call(&client, "save_project", json!({"path": path})).await);
    let project = mica_core::threemf::load_3mf(&path).unwrap();
    let spectrum = mica_core::full_spectrum::FullSpectrum::from_json(
        project.full_spectrum_json.as_deref().unwrap(),
        2,
    )
    .unwrap();
    assert_eq!(spectrum.extruders().len(), 1);
    let reopened = ok(&call(&client, "open", json!({"path": path})).await);
    assert_eq!(reopened["mixes"], info["mixes"]);
    let unchanged = ok(&call(&client, "set_filament_colors", json!({"colors": []})).await);
    assert_eq!(unchanged["changed"], false);
    // The 2T preset's startup template references absent higher tool outputs when both
    // tools are active (an existing template-engine limitation). Check cost on the 5T XL.
    ok(&call(
        &client,
        "open",
        json!({"path": model("cube20.stl"), "printer": "XL",
        "filament_colors": ["red", "yellow"]}),
    )
    .await);
    ok(&call(
        &client,
        "create_mix",
        json!({"target": "orange", "tools": [1, 2]}),
    )
    .await);
    ok(&call(&client, "set_part_tool", json!({"tool": 6})).await);
    let summary = ok(&call(&client, "slice_summary", Value::Null).await);
    assert_eq!(summary["gcode_check"]["errors"], 0, "{summary}");
    assert_eq!(summary["mixes_used"][0]["tool"], 6, "{summary}");
    assert!(
        summary["mix_time_cost"]
            .as_str()
            .unwrap()
            .contains("per layer")
    );
    assert_eq!(summary["color_cost"]["mixes_used"], summary["mixes_used"]);
    client.cancel().await.unwrap();
}

#[test]
fn create_mix_feature_validation_patterns_limit_and_undo() {
    use mica_cli::mcp::tools::scene_tool;
    use mica_core::full_spectrum::{Component, FullSpectrum, VirtualExtruder};
    let mut scene = Profiles {
        printer: Some("XL".into()),
        ..Default::default()
    }
    .open(&model("cube20.stl"))
    .unwrap();
    let run = |scene: &mut mica_cli::mcp::session::Scene, args| {
        let out = scene_tool(scene, "create_mix", args).unwrap()?;
        Ok::<Value, String>(serde_json::from_str(out[0].as_text().unwrap()).unwrap())
    };
    scene.color_mix_enabled = false;
    let e = run(&mut scene, json!({"target": "orange"})).unwrap_err();
    assert!(e.contains("Settings › Features"), "{e}");
    assert_eq!(scene.undo_steps(), 0);
    scene.color_mix_enabled = true;
    for args in [
        json!({"target": "unknown"}),
        json!({"target": "orange", "tools": [0, 1]}),
        json!({"target": "orange", "tools": [1]}),
        json!({"target": "orange", "tools": [1, 6]}),
        json!({"target": "orange", "max_components": 4}),
        json!({"target": "orange", "pattern": "unknown"}),
    ] {
        assert!(run(&mut scene, args).is_err());
    }
    assert_eq!(scene.undo_steps(), 0);
    scene
        .set_filament_colors(&["red".into(), "yellow".into(), "blue".into()])
        .unwrap();
    for pattern in ["layers", "outer_wall_first", "gradient"] {
        let result = run(
            &mut scene,
            json!({"target": "light blue", "max_components": 3,
            "tools": [1, 2, 3], "pattern": pattern, "name": "Test mix"}),
        )
        .unwrap();
        assert_eq!(result["pattern"], pattern);
        assert!(result["components"].as_array().unwrap().len() <= 3);
        assert_eq!(result["name"], "Test mix");
        assert!(scene.mixes().unwrap().contains(6));
        scene.undo().unwrap();
        assert!(scene.mixes().unwrap().extruders().is_empty());
    }
    // Named shades are estimates, not a promise: these less saturated filaments cannot
    // match the bright orange target closely, and must report the real difference.
    let named = run(&mut scene, json!({"target": "orange", "tools": [1, 2]})).unwrap();
    assert_eq!(named["quality"], "rough");
    assert!(named["delta_e"].as_f64().unwrap() >= 6.);
    scene.undo().unwrap();
    // A target from all three colors must be found when max_components permits it.
    let parts = scene
        .colors
        .iter()
        .take(3)
        .map(|rgb| mica_core::fdm_mixer::Part {
            rgb: *rgb,
            ratio: 1. / 3.,
        })
        .collect::<Vec<_>>();
    let target = mica_cli::mcp::colors::hex(mica_core::fdm_mixer::mix_rgb(&parts).unwrap());
    let pair = run(&mut scene, json!({"target": target, "tools": [1, 2, 3]})).unwrap();
    scene.undo().unwrap();
    let three = run(
        &mut scene,
        json!({"target": target, "tools": [1, 2, 3], "max_components": 3}),
    )
    .unwrap();
    assert_eq!(three["components"].as_array().unwrap().len(), 3, "{three}");
    assert!(three["delta_e"].as_f64().unwrap() < 0.1, "{three}");
    assert!(three["delta_e"].as_f64().unwrap() < pair["delta_e"].as_f64().unwrap());
    scene.undo().unwrap();
    // The engine caps IDs at 255. A duplicate still works when no new ID is available.
    let result = run(&mut scene, json!({"target": "orange", "tools": [1, 2]})).unwrap();
    let mix = scene.mixes().unwrap().extruders()[0].clone();
    let last = VirtualExtruder {
        id: 255,
        components: vec![
            Component {
                extruder: 3,
                ratio: 0.5,
            },
            Component {
                extruder: 4,
                ratio: 0.5,
            },
        ],
        ..Default::default()
    };
    scene
        .set_mixes(&FullSpectrum::new(vec![mix, last], 5).unwrap())
        .unwrap();
    let duplicate = run(&mut scene, json!({"target": "orange", "tools": [1, 2]})).unwrap();
    assert_eq!(duplicate["tool"], result["tool"]);
    assert_eq!(duplicate["existing"], true);
    let e = run(&mut scene, json!({"target": "blue", "tools": [1, 2]})).unwrap_err();
    assert!(e.contains("Cannot create another"), "{e}");
}
