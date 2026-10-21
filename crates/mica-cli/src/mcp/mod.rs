// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `mica mcp`, Mica MCP: a local Model Context Protocol server over stdio for AI assistants
//! (Claude Code, Claude Desktop, …). Its first capability: look at a model and colour-paint it
//! for multi-tool printing.
//! It renders views with a facet buffer behind every pixel, paints where the AI points (smart
//! fill, connected surface, single facet, sphere brush), by height band or by facing
//! direction, assigns part tools, undoes, slices for a summary and saves a 3MF with Mica's own
//! painting format. No network listener; an opened input file is never written. See
//! docs/mcp.md.
//!
//! Two modes: **headless**, one scene of the server's own (opened from a file, saved to a new
//! 3MF), and **live**, the project of the open Mica window, reached over a local socket
//! ([`live`]): the window runs the same tools ([`tools`]) on its project, so the user watches
//! every edit and can undo it there. By default the server uses the window when one answers.

pub mod colors;
pub mod live;
pub mod render;
pub mod session;
pub mod tools;

pub use tools::{
    AddShapeArgs, ClearArgs, ColorsArgs, CreateMixArgs, DirectionArgs, HeightBandArgs, OpenArgs,
    PaintAtArgs, PartToolArgs, PickArgs, RenderArgs, SaveArgs,
};

use live::Content;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Serialize;
use serde_json::{Value, json};
use session::{Profiles, Scene, Session};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const USAGE: &str = "\
Usage: mica mcp [--live | --headless] [<model.stl|model.3mf> [slice profile options]]

Runs Mica MCP, Mica's MCP server, on stdin/stdout for an AI assistant (Claude Code, Claude
Desktop). Its first capability: render and color-paint a model.
  (default)   live when a Mica window answers on its local socket (the window's project,
              every edit visible there and undoable with Edit › Undo), else headless
  --live      always the open Mica window; calls fail while none answers
  --headless  never the window: one scene of its own, opened from a file and saved to a new
              3MF
A model on the command line opens a headless scene at start, with the profile options `mica
slice` takes (--vendor, --printer, --print, --filament, --bed). Local only: stdio and a Unix
socket in Mica's settings folder, no network. Register it with:
claude mcp add mica -- /path/to/mica mcp";

const INSTRUCTIONS: &str = "\
Mica MCP: Mica Slicer's tools for AI assistants. Its first capability: look at a 3D model and
color-paint it for a multi-tool printer.
Workflow: open the model (printer \"U1\" for a 4-tool Snapmaker U1, \"XL\" for a 5-tool Prusa XL)
-> scene_info (parts, sizes, tools and filament colors; set_filament_colors to the colors actually
loaded) -> render_view (look at the picture) -> paint by pointing at pixels of that view
(paint_at), or by height (paint_height_band) or facing direction (paint_by_direction), or whole
parts (set_part_tool) -> render_view again to check -> undo mistakes -> slice_summary ->
save_project (a new .3mf in the user's folders; without a path it goes to the Desktop). add_shape
puts a ready-made model on the plate (the 3DBenchy, a cube, cylinder, sphere, ...). Tools are
numbered from 1 like the printer's slots; tool 0 erases paint back to the part's own tool.
create_mix finds a target color from loaded filaments with ColorMix. Its virtual tool number
works with every paint/assignment tool. Mixed colors alternate physical tools per layer and
add print time. Report colors by name; hex fields are machine-readable data.
Coordinates are bed millimetres, z up, -y is the front.
Fills: smart_fill only stops at edges sharper than its angle, so on a smooth model (a boat hull, a
figure) it runs over the whole part. Give max_distance (mm along the surface from the clicked
point) to keep a fill local; a fill that would cover more than 40% of a part is refused unless
allow_large is true.
Cost: every color boundary on a layer costs a tool change on every such layer (time, purge
filament, the prime tower). Keep colors reasonable: prefer whole parts (set_part_tool), height
bands that change color once and a few large fills over many small spots. After slice_summary,
tell the user the tool changes, the extra time and filament against one color (color_cost), and
offer cheaper alternatives when the cost is high.
Modes (see the status tool): in live mode the tools work on the project open in the user's Mica
window: the user sees every edit at once and can undo it there (each edit is one undo step named
\"Mica MCP: ...\"). The window's project may already hold objects: call scene_info first instead of
open. open only works when the window has no unsaved changes; save_project never replaces the
user's open project file unless asked (overwrite and replace_open_project). In headless mode the
server has a scene of its own, opened from a file.";

/// Which project the tools work on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The open Mica window when one answers, else headless.
    Auto,
    /// Always the open Mica window.
    Live,
    /// A scene of the server's own.
    Headless,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Live => "live",
            Self::Headless => "headless",
        }
    }
}

/// Where calls go: the server's own session, or the window behind the socket.
struct Backend {
    mode: Mode,
    socket: PathBuf,
    session: Mutex<Session>,
    client: Mutex<Option<live::Client>>,
}

/// The MCP server: tools as below, on the headless session or the open window.
#[derive(Clone)]
pub struct MicaMcp {
    backend: Arc<Backend>,
    tool_router: ToolRouter<Self>,
}

impl Default for MicaMcp {
    fn default() -> Self {
        Self::new()
    }
}

fn block(c: Content) -> ContentBlock {
    match c {
        Content::Text { text } => ContentBlock::text(text),
        Content::Image { data, mime } => ContentBlock::image(data, mime),
    }
}

fn to_args<T: Serialize>(a: &T) -> Value {
    serde_json::to_value(a).unwrap_or(Value::Null)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl MicaMcp {
    /// A headless server (never the window): what tests and embedders get.
    pub fn new() -> Self {
        Self::with_mode(Mode::Headless, live::socket_path())
    }

    /// A server in `mode`, reaching the window on `socket`.
    pub fn with_mode(mode: Mode, socket: PathBuf) -> Self {
        Self {
            backend: Arc::new(Backend {
                mode,
                socket,
                session: Mutex::new(Session::default()),
                client: Mutex::new(None),
            }),
            tool_router: Self::tool_router(),
        }
    }

    /// A headless server with a model already open (`mica mcp model.stl …`).
    pub fn with_scene(scene: Scene) -> Self {
        let s = Self::new();
        lock(&s.backend.session).scene = Some(scene);
        s
    }

    /// Runs a tool off the async threads; errors become tool errors the AI reads.
    async fn run(&self, tool: &'static str, args: Value) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        let result = tokio::task::spawn_blocking(move || this.call(tool, args)).await;
        Ok(match result {
            Ok(Ok(content)) => CallToolResult::success(content.into_iter().map(block).collect()),
            Ok(Err(message)) => CallToolResult::error(vec![ContentBlock::text(message)]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!(
                "internal error in Mica (the session is kept; undo may help): {e}"
            ))]),
        })
    }

    /// One tool call, blocking: to the window or the headless session (see [`Mode`]).
    pub fn call(&self, tool: &str, args: Value) -> Result<Vec<Content>, String> {
        let live = self.route(tool)?;
        if tool == "status" {
            return Ok(vec![tools::text(&self.status_json(live))]);
        }
        if live {
            self.live_call(tool, args)
        } else {
            self.headless_call(tool, args)
        }
    }

    /// Whether this call goes to the window. Auto mode keeps a headless scene the AI opened
    /// (until the next open) and otherwise uses the window whenever one answers.
    fn route(&self, tool: &str) -> Result<bool, String> {
        let b = &self.backend;
        match b.mode {
            Mode::Headless => Ok(false),
            Mode::Live => self.connect().map(|()| true),
            Mode::Auto => {
                if lock(&b.client).is_some() {
                    return Ok(true);
                }
                if lock(&b.session).scene.is_some() && tool != "open" {
                    return Ok(false);
                }
                let live = self.connect().is_ok();
                if live && tool == "open" {
                    // The window takes over; the headless scene would only confuse later
                    // calls if the window closes.
                    lock(&b.session).scene = None;
                }
                Ok(live)
            }
        }
    }

    fn connect(&self) -> Result<(), String> {
        let b = &self.backend;
        let mut client = lock(&b.client);
        if client.is_none() {
            *client = Some(live::Client::connect(&b.socket).map_err(|e| e.to_string())?);
        }
        Ok(())
    }

    fn live_call(&self, tool: &str, args: Value) -> Result<Vec<Content>, String> {
        let b = &self.backend;
        let mut guard = lock(&b.client);
        let client = guard
            .as_mut()
            .ok_or("not connected to a Mica window; try again")?;
        match client.call(tool, args) {
            Ok(result) => result,
            Err(e) => {
                *guard = None;
                Err(format!(
                    "{e}. The Mica window is gone (closed?); nothing more was changed. {}",
                    match b.mode {
                        Mode::Live => "Start Mica again; the next call reconnects.",
                        _ => "The next call reconnects, or works headless when no window answers.",
                    }
                ))
            }
        }
    }

    fn status_json(&self, live: bool) -> Value {
        let b = &self.backend;
        let mut out = json!({
            "mode": if live { "live" } else { "headless" },
            "requested_mode": b.mode.name(),
            "socket": b.socket.display().to_string(),
        });
        if live {
            match self.live_call("status", Value::Null) {
                Ok(content) => {
                    out["window"] = content
                        .first()
                        .and_then(Content::as_text)
                        .and_then(|t| serde_json::from_str(t).ok())
                        .unwrap_or(Value::Null);
                }
                Err(e) => out["window_error"] = json!(e),
            }
            if let Some(c) = lock(&b.client).as_ref() {
                out["window_app"] = c.hello.clone();
            }
            out["note"] = json!(
                "Tools work on the project open in the Mica window; every edit is one undo step \
                 there (\"Mica MCP: ...\")."
            );
        } else {
            let scene = lock(&b.session)
                .scene
                .as_ref()
                .map(|s| s.source.display().to_string());
            out["scene"] = json!(scene);
            if b.mode != Mode::Headless {
                out["window_available"] = json!(live::Client::connect(&b.socket).is_ok());
            }
            out["note"] = json!(match b.mode {
                Mode::Headless =>
                    "Started with --headless: the server's own scene, never the window.",
                _ =>
                    "No Mica window was used: the server's own scene. Calls switch to the window \
                     when one answers and no headless scene is open (or on the next open).",
            });
        }
        out
    }

    fn headless_call(&self, tool: &str, args: Value) -> Result<Vec<Content>, String> {
        let mut s = lock(&self.backend.session);
        match tool {
            "open" => {
                let a: OpenArgs = tools::parse(tool, args)?;
                let profiles = Profiles {
                    printer: a.printer,
                    vendor: a.vendor,
                    print: a.print,
                    filament: a.filament,
                    bed: a.bed,
                };
                let mut scene = profiles.open(&PathBuf::from(&a.path))?;
                if let Some(colors) = &a.filament_colors {
                    scene.set_filament_colors(colors)?;
                }
                let info = scene.info();
                s.scene = Some(scene);
                Ok(vec![tools::text(&info)])
            }
            "undo" => {
                let scene = s.scene()?;
                let label = scene.undo()?;
                Ok(vec![tools::text(
                    &json!({"undone": label, "undo_steps": scene.undo_steps()}),
                )])
            }
            "add_shape" => {
                let a: AddShapeArgs = tools::parse(tool, args)?;
                let (model, size, kind) = tools::shape_request(&a)?;
                if s.scene.is_none() {
                    if kind.is_some() {
                        return Err("no model is open: add a shape as an object first (kind \
                                    object), or open a model"
                            .into());
                    }
                    let scene = shape_scene(model, size, a.position)?;
                    let info = scene.info();
                    s.scene = Some(scene);
                    return Ok(vec![tools::text(&json!({
                        "added": model.name(),
                        "kind": "object",
                        "object": 1,
                        "note": "A new scene with this shape on the default printer (CORE One); \
                                 open a model or another printer to start over.",
                        "scene": info,
                    }))]);
                }
                let scene = s.scene()?;
                let (result, _) = scene.edit(&format!("add_shape {}", model.id()), |sc| {
                    sc.add_shape(model, size, a.position, kind, a.object)
                })?;
                let mut result = result;
                result["undo_steps"] = json!(scene.undo_steps());
                result["next"] = json!("render_view to look at it");
                Ok(vec![tools::text(&result)])
            }
            "slice_summary" => Ok(vec![tools::text(&s.scene()?.slice_summary()?)]),
            "save_project" => {
                let a: SaveArgs = tools::parse(tool, args)?;
                let scene = s.scene()?;
                let flavour = tools::flavour(a.flavour.as_deref())?;
                let path = scene.save_path(a.path.as_deref())?;
                let summary = scene.save(&path, flavour, a.overwrite.unwrap_or(false))?;
                scene.edited = false;
                Ok(vec![tools::text(
                    &json!({"saved": path.display().to_string(), "summary": summary}),
                )])
            }
            other => tools::scene_tool(s.scene()?, other, args)
                .unwrap_or_else(|| Err(format!("unknown tool '{other}'"))),
        }
    }

    /// The server instructions with the mode this server starts in.
    fn instructions(&self) -> String {
        let b = &self.backend;
        let now = match b.mode {
            Mode::Headless => "Mode now: headless (started with --headless).".to_string(),
            Mode::Live => format!(
                "Mode now: live (started with --live): the open Mica window on {}.",
                b.socket.display()
            ),
            Mode::Auto => {
                if self.connect().is_ok() {
                    "Mode now: live, connected to the open Mica window.".to_string()
                } else {
                    "Mode now: headless (no Mica window answers); calls use the window as soon \
                     as one answers."
                        .to_string()
                }
            }
        };
        format!("{INSTRUCTIONS}\n{now} Call status to check.")
    }
}

/// A headless scene of one built-in model: its STL written to a fresh temporary folder and
/// opened as `open` opens an STL (default printer), the object named after the model.
fn shape_scene(
    model: mica_core::builtin_models::BuiltinModel,
    size: [f32; 3],
    position: Option<[f32; 2]>,
) -> Result<Scene, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mesh = model.mesh_sized(size)?;
    let dir = std::env::temp_dir().join(format!(
        "mica-mcp-shape-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.stl", model.name()));
    let mut out = Vec::new();
    mica_core::mesh_edit::write_stl(&mesh, model.name(), &mut out)
        .and_then(|()| std::fs::write(&path, &out))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let opened = Profiles::default().open(&path);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    let mut scene = opened?;
    scene.rename_object(0, model.name());
    if let Some(at) = position {
        scene.move_object_to(0, at)?;
    }
    Ok(scene)
}

#[tool_router]
impl MicaMcp {
    #[tool(
        description = "Open an .stl or .3mf model for painting (the file is only read). Pick a multi-tool printer to paint colors: printer \"U1\" (Snapmaker U1, 4 tools) or \"XL\" (Prusa XL, 5 tools). A 3MF keeps its own printer and painting. Headless: replaces the server's scene. Live (the Mica window): opens it in the window as a new project, refused while the window has unsaved changes. Returns the scene_info summary. Next: render_view."
    )]
    async fn open(&self, Parameters(a): Parameters<OpenArgs>) -> Result<CallToolResult, McpError> {
        self.run("open", to_args(&a)).await
    }

    #[tool(
        description = "Describe the open scene: printer and its tools with filament colors (name and hex), objects and parts with sizes, bounding boxes (bed mm, z up, -y front), triangle counts, each part's own tool, and painted area per tool. Use the bounding boxes to plan paint_height_band heights."
    )]
    async fn scene_info(&self) -> Result<CallToolResult, McpError> {
        self.run("scene_info", Value::Null).await
    }

    #[tool(
        description = "Render the model as a PNG with its current paint (filament colors) and return it with a view_id. Look at the image, then point at its pixels with paint_at/pick using that view_id. Lines mark silhouettes and sharp edges (where smart_fill stops); a faint grid every 100 px with labels helps reading pixel positions. The text lists each visible part's bbox_px and center_px. Use several views (front/back/left/right/top/iso...) to see all sides, zoom+center for small details, highlight_paint to check what is painted. Re-render after painting to verify."
    )]
    async fn render_view(
        &self,
        Parameters(a): Parameters<RenderArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("render_view", to_args(&a)).await
    }

    #[tool(
        description = "Paint where you point: the surface under pixel (x, y) of a rendered view gets `tool`. Modes: smart_fill (default; spreads over the surface until edges sharper than `angle` degrees, default 30, like Mica's Fill brush), connected (the whole connected shell), triangle (one facet), sphere (within `radius` mm). smart_fill and connected take max_distance (mm along the surface from the point; 0 = no limit): on smooth surfaces a fill only stops at sharp edges, so limit it. A fill that would cover more than 40% of the part is refused (nothing painted) with its area and a suggested max_distance; pass allow_large: true only when the whole region is meant. Returns the object/part hit, the point in mm and the area painted. One call is one undo step. Re-render to check; a fill that spread too far: undo, then retry with max_distance, a smaller angle, a height band or sphere mode. Each extra color region costs tool changes: prefer few, large regions."
    )]
    async fn paint_at(
        &self,
        Parameters(a): Parameters<PaintAtArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("paint_at", to_args(&a)).await
    }

    #[tool(
        description = "Report what is under pixel (x, y) of a rendered view without painting: object, part, facet, the 3D point in bed mm (its z helps plan paint_height_band), the facet normal and the tool(s) on it."
    )]
    async fn pick(&self, Parameters(a): Parameters<PickArgs>) -> Result<CallToolResult, McpError> {
        self.run("pick", to_args(&a)).await
    }

    #[tool(
        description = "Paint a horizontal band of an object, from z_min to z_max mm above the bed, all the way around (e.g. stripes, a base, a top cap). Facets crossing the band edges are split so the band edges are straight. One undo step."
    )]
    async fn paint_height_band(
        &self,
        Parameters(a): Parameters<HeightBandArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("paint_height_band", to_args(&a)).await
    }

    #[tool(
        description = "Paint every facet of an object (or part) that faces a direction: up (top surfaces), down, side (walls), front/back/left/right, or a vector \"x,y,z\", within max_angle degrees (default 30), optionally only between z_min and z_max. Whole facets. One undo step."
    )]
    async fn paint_by_direction(
        &self,
        Parameters(a): Parameters<DirectionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("paint_by_direction", to_args(&a)).await
    }

    #[tool(
        description = "Set the tool a whole object, or one part of it, prints with where it is not painted (its base color). Paint stays on top. One undo step."
    )]
    async fn set_part_tool(
        &self,
        Parameters(a): Parameters<PartToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("set_part_tool", to_args(&a)).await
    }

    #[tool(
        description = "Tell Mica which filament colors are loaded, per tool in slot order (names like \"red\" or \"#RRGGBB\"; \"\" keeps a slot). Renders and the saved project use them. One undo step."
    )]
    async fn set_filament_colors(
        &self,
        Parameters(a): Parameters<ColorsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("set_filament_colors", to_args(&a)).await
    }

    #[tool(
        description = "Create a ColorMix mixed color closest to target (color name or RGB hex) using loaded physical tools and up to 2 (default) or 3 components. Searches printable ratios with the Prusa color predictor and CIEDE2000. Returns a virtual tool number usable with all paint tools and set_part_tool, components/ratios, predicted color name plus machine-readable hex, delta_e and quality. Same recipe/pattern returns the existing mix without an undo step. Requires ColorMix in Settings › Features. Patterns: layers (default), outer_wall_first, gradient (target at bottom, reversed shares at top). Mixed colors alternate tools per layer and cost time: report slice_summary color_cost. One undo step, including live: Mica MCP: create mix."
    )]
    async fn create_mix(
        &self,
        Parameters(a): Parameters<CreateMixArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("create_mix", to_args(&a)).await
    }

    #[tool(
        description = "Remove color paint from an object or part (default: everything), back to each part's own tool. One undo step."
    )]
    async fn clear_paint(
        &self,
        Parameters(a): Parameters<ClearArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("clear_paint", to_args(&a)).await
    }

    #[tool(
        description = "Undo the last painting, tool or color change (one tool call per step). Live: undoes the window's last step when it is a Mica MCP edit (the user's own edits are never undone)."
    )]
    async fn undo(&self) -> Result<CallToolResult, McpError> {
        self.run("undo", Value::Null).await
    }

    #[tool(
        description = "Slice the current project with its printer settings (a temporary copy; nothing is sent to a printer) and summarise: layers, estimated time, filament per tool (meters, grams), tool changes, purge and tower filament, warnings, and (when it uses several tools) the same model sliced in one color with color_cost: the extra time and filament the colors take. Takes seconds to a minute on large models. Live: the window also slices its plate, so the user sees the preview. Use it to confirm the paint prints with the intended tools, then tell the user the tool changes and the cost against one color, and offer cheaper alternatives (whole parts, height bands, fewer regions) when it is high."
    )]
    async fn slice_summary(&self) -> Result<CallToolResult, McpError> {
        self.run("slice_summary", Value::Null).await
    }

    #[tool(
        description = "Save the painted project as a new .3mf (painting, part tools, printer settings and filament colors included); it opens in Mica, and in OrcaSlicer (flavour orca) or PrusaSlicer (flavour prusa) with the painting intact. Without a path it saves a new file on the user's Desktop (Documents without one) and returns where. Refuses the opened input file and paths inside Mica's source folder; an existing file needs overwrite: true. Live: the window saves its whole project there and it becomes the window's document; the project file already open in the window is only replaced with overwrite: true and replace_open_project: true, which needs the user's explicit go-ahead. Tell the user where it was saved."
    )]
    async fn save_project(
        &self,
        Parameters(a): Parameters<SaveArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("save_project", to_args(&a)).await
    }

    #[tool(
        description = "Put a built-in model on the plate: the original #3DBenchy (benchy) or a generated shape (cube, cylinder, cone, sphere, torus, pyramid, wedge, tube, disc), at its own size or `size` mm. kind object (default) adds a new object at `position` [x, y] or a free spot; part, modifier (starts at 100% infill), negative_volume, support_enforcer or support_blocker joins `object` instead (centred in it; a part stands beside it). Saved projects keep it as a plain mesh object. Headless without an open model it starts a scene with the shape. Live: on the window's current plate, one undo step there. One undo step. Returns the object (and part) number."
    )]
    async fn add_shape(
        &self,
        Parameters(a): Parameters<AddShapeArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run("add_shape", to_args(&a)).await
    }

    #[tool(
        description = "Which mode this server is in: live (the project open in the user's Mica window: edits show there at once and are undoable there) or headless (a scene of the server's own), with the window's project, unsaved changes and printer in live mode."
    )]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        self.run("status", Value::Null).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MicaMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mica", mica_core::VERSION))
            .with_instructions(self.instructions())
    }
}

/// `mica mcp [--live|--headless] [model] [profile options]`: serves on stdin/stdout until the
/// client closes it.
pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let live = args.iter().any(|a| a == "--live");
    let headless = args.iter().any(|a| a == "--headless");
    let mode = match (live, headless) {
        (true, true) => return Err("mica mcp: --live and --headless exclude each other".into()),
        (true, false) => Mode::Live,
        (false, true) => Mode::Headless,
        (false, false) => Mode::Auto,
    };
    let rest: Vec<String> = args
        .iter()
        .filter(|a| *a != "--live" && *a != "--headless")
        .cloned()
        .collect();
    let server = if rest.is_empty() {
        MicaMcp::with_mode(mode, live::socket_path())
    } else if mode == Mode::Live {
        return Err(
            "mica mcp --live works on the Mica window's project; leave out the model and let \
             the AI call open (or use --headless with a model)"
                .into(),
        );
    } else {
        let a = crate::parse_slice(&rest)?;
        let server = MicaMcp::with_mode(mode, live::socket_path());
        lock(&server.backend.session).scene = Some(Scene::open(a)?);
        server
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("starting the MCP runtime: {e}"))?;
    runtime.block_on(async move {
        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| format!("MCP start: {e}"))?;
        service
            .waiting()
            .await
            .map_err(|e| format!("MCP session: {e}"))?;
        Ok(())
    })
}
