// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Mica MCP live mode, the window's side: `mica mcp` connects to the open window over a Unix
//! socket in Mica's settings folder (`mica_cli::mcp::live`) and the AI works on the window's
//! project, so the user watches every edit appear and can undo it with Edit › Undo.
//!
//! - **Socket**: opened at start when the Mica MCP feature is on (`[features] mcp` in
//!   `mica.toml`), mode 0600, removed when the window closes; a stale file left by a crashed
//!   window is replaced, one another window answers on is left alone.
//! - **Threads**: one thread accepts, one per connection reads requests and waits for the
//!   answer; requests reach the UI thread through a channel and run between frames
//!   ([`App::poll_mcp`]), one at a time.
//! - **Same tools**: the painting, rendering and inspection tools run the CLI's own
//!   `mcp::tools::scene_tool` on a `Scene` opened from the project exactly as the window
//!   saves it (`App::build_project` written to a temporary 3MF, opened as `mica mcp` opens a
//!   3MF), on a worker thread, so a live call gives the result a headless call gives on the
//!   saved project. The scene is kept while the window's project is unchanged (views stay
//!   valid) and synced again when the user edited it.
//! - **Applying**: a call that changed the scene is applied back to the plates on the UI
//!   thread as one undo step, `Mica MCP: paint`: color painting (the model parts' streams
//!   joined like the object's mesh), object and part tools, and the slot colors. If the user
//!   changed the project while the call ran, nothing is applied and the AI is told to retry.
//! - **Window-only tools**: `open` (a new project, refused while there are unsaved changes),
//!   `save_project` (a copy; the open project's own file only with explicit confirmation),
//!   `slice_summary` (the summary slice on the worker, plus the window's normal slice of the
//!   plate so the user sees it), `undo` (the window's undo, only for a Mica MCP step) and
//!   `status`; `add_shape` (a built-in model on the current plate, or as a part of an
//!   object, one undo step `Mica MCP: add shape`).

use crate::app::App;
use crate::save::DocState;
use eframe::egui;
#[cfg(unix)]
use mica_cli::mcp::live;
use mica_cli::mcp::live::{Content, Request, Response};
use mica_cli::mcp::session::{self, Profiles, Scene};
use mica_cli::mcp::tools;
use mica_core::painting::FacetStates;
use mica_core::threemf::{self, Flavour, PartKind, Project};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Undo labels of live edits start with this.
pub const UNDO_PREFIX: &str = "Mica MCP: ";

/// A request from a connection thread with where its answer goes.
pub(crate) struct Incoming {
    pub request: Request,
    pub reply: mpsc::Sender<Response>,
}

impl Incoming {
    fn answer(self, result: Result<Vec<Content>, String>) {
        let _ = self.reply.send(Response::new(self.request.id, result));
    }
}

/// What a running request waits for.
enum Job {
    /// A scene tool or the summary slice on the worker thread.
    Scene {
        incoming: Incoming,
        /// The project when the call started; the result is only applied to that state.
        key: Box<DocState>,
        done: mpsc::Receiver<Done>,
    },
    /// `open`: the window is loading the file; then `scene_info` runs.
    Load {
        incoming: Incoming,
        colors: Option<Vec<String>>,
        path: PathBuf,
    },
}

/// A worker's result: the scene back (`None` when it could not be synced) and the answer.
struct Done {
    synced: Option<Box<Synced>>,
    result: Result<Vec<Content>, String>,
    changed: bool,
}

/// One project object of the synced scene and where it lives in the window.
#[derive(Clone)]
struct Target {
    plate: usize,
    /// The plate's objects that are this object's instances (the first is the primary).
    members: Vec<usize>,
    /// Why its color painting cannot be written back, if it cannot.
    unwritable: Option<String>,
    /// The window's painting runs the other way round to the saved part's.
    flip: bool,
}

/// What the scene held when it last matched the window: per object, the color painting
/// joined like the window's mesh, the object's `extruder` and each model part's; the slot
/// colors.
#[derive(Clone, PartialEq)]
struct Baseline {
    paint: Vec<FacetStates>,
    object_tool: Vec<Option<String>>,
    part_tools: Vec<Vec<Option<String>>>,
    colors: Vec<[u8; 3]>,
    mixes: Option<String>,
}

/// The scene the tools run on, with the window state it mirrors.
struct Synced {
    scene: Scene,
    key: DocState,
    targets: Vec<Target>,
    baseline: Baseline,
}

/// What the UI thread hands the worker to (re)build the scene.
struct SyncInput {
    project: Project,
    flavour: Flavour,
    targets: Vec<Target>,
    /// The window's color painting per object, to check the mapping both ways.
    window_paint: Vec<FacetStates>,
    /// Objects whose winding flip cannot be told from their painting.
    flip_candidates: Vec<bool>,
    source: PathBuf,
}

/// The live endpoint of a window (default: off, no socket).
#[derive(Default)]
pub(crate) struct Host {
    inbox: Option<mpsc::Receiver<Incoming>>,
    clients: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    #[cfg(unix)]
    listener: Option<live::Listener>,
    queue: VecDeque<Incoming>,
    job: Option<Job>,
    synced: Option<Box<Synced>>,
    /// The next view id, kept across re-syncs.
    next_view: u32,
    /// The last error a call returned, shown next to the indicator for a while.
    pub(crate) last_error: Option<(Instant, String)>,
    /// What the last call did, for the indicator's tooltip.
    pub(crate) last_call: Option<String>,
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Host {
    /// Same request queue without a socket, for tests in sandboxes that deny Unix sockets
    /// and the monkey test.
    pub(crate) fn in_process() -> (Self, mpsc::Sender<Incoming>) {
        let (tx, rx) = mpsc::channel();
        let mut host = Self::default();
        host.inbox = Some(rx);
        (host, tx)
    }

    /// Listens on `path` and serves connections until [`Host::shutdown`]. Errors (another
    /// window serves Mica MCP, the folder is not writable, Windows) leave the host off.
    #[cfg(unix)]
    pub(crate) fn start(path: &std::path::Path, ctx: &egui::Context) -> Result<Self, String> {
        let listener = live::bind(path)?;
        let accept = listener
            .listener
            .try_clone()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let (tx, rx) = mpsc::channel::<Incoming>();
        let clients = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (clients, stop, ctx) = (clients.clone(), stop.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("mica-mcp-accept".into())
                .spawn(move || {
                    for stream in accept.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        let Ok(stream) = stream else {
                            continue;
                        };
                        let (tx, clients, ctx) = (tx.clone(), clients.clone(), ctx.clone());
                        let _ = std::thread::Builder::new()
                            .name("mica-mcp-client".into())
                            .spawn(move || serve(stream, &tx, &clients, &ctx));
                    }
                })
                .map_err(|e| format!("starting Mica MCP: {e}"))?;
        }
        let mut host = Self::default();
        host.inbox = Some(rx);
        host.clients = clients;
        host.stop = stop;
        host.listener = Some(listener);
        Ok(host)
    }

    #[cfg(not(unix))]
    pub(crate) fn start(_path: &std::path::Path, _ctx: &egui::Context) -> Result<Self, String> {
        Err("Mica MCP live mode is not supported on Windows yet".into())
    }

    /// Stops listening and removes the socket (the window is closing).
    pub(crate) fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        if let Some(l) = self.listener.take() {
            // Wakes the accept thread, which sees `stop` and ends.
            let _ = std::os::unix::net::UnixStream::connect(&l.path);
            drop(l);
        }
        for i in self.queue.drain(..) {
            i.answer(Err("the Mica window is closing".into()));
        }
    }

    /// Clients connected now.
    pub(crate) fn clients(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Counts the in-process caller as a connected client (screenshots with `--mcp`), so the
    /// window shows what it shows while an AI assistant is attached.
    pub(crate) fn attach_in_process_client(&self) {
        self.clients.fetch_add(1, Ordering::SeqCst);
    }
}

/// One connection: requests go to the UI thread, the thread waits for each answer.
#[cfg(unix)]
fn serve(
    stream: std::os::unix::net::UnixStream,
    tx: &mpsc::Sender<Incoming>,
    clients: &AtomicUsize,
    ctx: &egui::Context,
) {
    struct Count<'a>(&'a AtomicUsize, &'a egui::Context);
    impl Drop for Count<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
            self.1.request_repaint();
        }
    }
    clients.fetch_add(1, Ordering::SeqCst);
    let _count = Count(clients, ctx);
    ctx.request_repaint();
    live::serve_connection(stream, |request| {
        let id = request.id;
        let (reply, answer) = mpsc::channel();
        if tx.send(Incoming { request, reply }).is_err() {
            return Response::new(id, Err("the Mica window is closing".into()));
        }
        ctx.request_repaint();
        answer
            .recv()
            .unwrap_or_else(|_| Response::new(id, Err("the Mica window is closing".into())))
    });
}

/// Replaces fields of a JSON text result.
fn patch(content: &mut [Content], f: impl Fn(&mut Value)) {
    for c in content {
        if let Content::Text { text } = c
            && let Ok(mut v) = serde_json::from_str::<Value>(text)
            && v.is_object()
        {
            f(&mut v);
            *text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
        }
    }
}

fn extruder(s: &threemf::Settings) -> Option<String> {
    s.get("extruder").map(|v| v.trim().to_string())
}

/// A saved object's model parts with triangles (in order): the parts the window's mesh joins.
fn model_parts(o: &threemf::Object) -> impl Iterator<Item = (usize, &threemf::Part)> {
    o.parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == PartKind::Model && !p.mesh.faces.is_empty())
}

/// The object's color painting joined over its model parts, as the window keeps it.
fn joined_paint(o: &threemf::Object, flip: bool) -> Result<FacetStates, String> {
    let mut out = FacetStates::default();
    let mut offset = 0u32;
    for (_, p) in model_parts(o) {
        let mut m = p.painting.material.clone();
        if flip {
            let reversed = m.reversed();
            m = m.with_reversed(!reversed);
        }
        out.append_shifted(&m, offset)?;
        offset += p.mesh.faces.len() as u32;
    }
    Ok(out)
}

fn baseline(scene: &Scene, targets: &[Target]) -> Baseline {
    let project = scene.project();
    Baseline {
        paint: project
            .objects
            .iter()
            .zip(targets)
            .map(|(o, t)| joined_paint(o, t.flip).unwrap_or_default())
            .collect(),
        object_tool: project
            .objects
            .iter()
            .map(|o| extruder(&o.settings))
            .collect(),
        part_tools: project
            .objects
            .iter()
            .map(|o| model_parts(o).map(|(_, p)| extruder(&p.settings)).collect())
            .collect(),
        colors: scene.colors.clone(),
        mixes: project.full_spectrum_json.clone(),
    }
}

/// Opens the scene from the window's project (on the worker thread).
fn open_scene(input: SyncInput, next_view: u32, key: DocState) -> Result<Box<Synced>, String> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mica-mcp-live-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join("project.3mf");
    let opened = threemf::write(&file, &input.project, input.flavour)
        .map_err(|e| format!("writing the window's project for Mica MCP: {e}"))
        .and_then(|()| Profiles::default().open(&file));
    let _ = std::fs::remove_dir_all(&dir);
    let mut scene =
        opened.map_err(|e| format!("Mica MCP could not read the window's project: {e}"))?;
    scene.source = input.source;
    scene.continue_view_ids(next_view);
    let mut targets = input.targets;
    if scene.project().objects.len() != targets.len() {
        return Err(format!(
            "the window's project has {} objects but its saved copy {}; Mica MCP cannot map \
             them back",
            targets.len(),
            scene.project().objects.len()
        ));
    }
    // The painting must map back to exactly what the window holds; an object where it does
    // not (either winding) is left alone rather than repainted wrongly.
    for (i, (t, o)) in targets.iter_mut().zip(&scene.project().objects).enumerate() {
        let window = &input.window_paint[i];
        let straight = joined_paint(o, false).ok();
        let flipped = joined_paint(o, true).ok();
        let ambiguous = input.flip_candidates[i] && window.is_empty();
        if ambiguous {
            t.unwritable = Some(
                "it is a mirrored object from an Orca project without color painting yet; \
                 paint it in Mica itself"
                    .into(),
            );
        } else if straight.as_ref() == Some(window) {
            t.flip = false;
        } else if flipped.as_ref() == Some(window) {
            t.flip = true;
        } else {
            t.unwritable = Some(
                "its painting in the window does not match the saved project's (mixed part \
                 windings?)"
                    .into(),
            );
        }
    }
    let baseline = baseline(&scene, &targets);
    Ok(Box::new(Synced {
        scene,
        key,
        targets,
        baseline,
    }))
}

/// The scene operation a worker runs.
enum Op {
    Tool(String, Value),
    Summary,
}

fn run_op(synced: &mut Synced, op: Op) -> Result<Vec<Content>, String> {
    match op {
        Op::Tool(tool, args) => tools::scene_tool(&mut synced.scene, &tool, args)
            .unwrap_or_else(|| Err(format!("unknown tool '{tool}'"))),
        Op::Summary => Ok(vec![tools::text(&synced.scene.slice_summary()?)]),
    }
}

/// What applying a changed scene does to the window.
#[derive(Default)]
struct Changes {
    paint: Vec<(usize, FacetStates)>,
    object_tool: Vec<(usize, usize)>,
    /// (object, model part index among the object's model parts, tool).
    part_tool: Vec<(usize, usize, usize)>,
    colors: Option<Vec<[u8; 3]>>,
    mixes: Option<Option<String>>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.paint.is_empty()
            && self.object_tool.is_empty()
            && self.part_tool.is_empty()
            && self.colors.is_none()
            && self.mixes.is_none()
    }
}

fn tool_number(v: &Option<String>) -> Option<usize> {
    v.as_deref()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|t| *t > 0)
}

/// The differences between the scene now and its baseline, checked against what the window
/// can take.
fn changes(synced: &Synced) -> Result<Changes, String> {
    let now = baseline(&synced.scene, &synced.targets);
    let was = &synced.baseline;
    let mut out = Changes::default();
    for (i, t) in synced.targets.iter().enumerate() {
        if now.paint[i] != was.paint[i] {
            if let Some(why) = &t.unwritable {
                return Err(format!(
                    "object {} cannot be painted in live mode: {why}; nothing was changed",
                    i + 1
                ));
            }
            let o = &synced.scene.project().objects[i];
            out.paint.push((i, joined_paint(o, t.flip)?));
        }
        if now.object_tool[i] != was.object_tool[i]
            && let Some(tool) = tool_number(&now.object_tool[i])
        {
            out.object_tool.push((i, tool));
        }
        for (k, (a, b)) in now.part_tools[i].iter().zip(&was.part_tools[i]).enumerate() {
            if a != b
                && let Some(tool) = tool_number(a)
            {
                out.part_tool.push((i, k, tool));
            }
        }
    }
    if now.colors != was.colors {
        out.colors = Some(now.colors);
    }
    if now.mixes != was.mixes {
        out.mixes = Some(now.mixes);
    }
    Ok(out)
}

/// The printer presets the `open` shortcuts name, as (vendor, printer).
fn printer_shortcut(name: &str) -> Option<(&'static str, &'static str)> {
    let key: String = name
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    match key.as_str() {
        "u1" | "snapmakeru1" => Some(("Snapmaker", "Snapmaker U1 (0.4 nozzle)")),
        "xl" | "prusaxl" | "xl5t" | "prusaxl5t" => {
            Some(("Prusa", "Original Prusa XL - 5T Input Shaper 0.4 nozzle"))
        }
        "coreone" | "prusacoreone" => Some(("Prusa", "Prusa CORE One HF0.4 nozzle")),
        _ => None,
    }
}

impl App {
    /// Takes requests from the connections and runs them, one at a time, between frames.
    pub(crate) fn poll_mcp(&mut self, ctx: &egui::Context) {
        if let Some(inbox) = &self.mcp.inbox {
            while let Ok(i) = inbox.try_recv() {
                self.mcp.queue.push_back(i);
            }
        }
        self.poll_mcp_job(ctx);
        while self.mcp.job.is_none()
            && let Some(incoming) = self.mcp.queue.pop_front()
        {
            self.start_mcp(incoming, ctx);
        }
        if self.mcp.job.is_some() {
            // The worker wakes the window when it is done; this keeps a load moving.
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Whether the project holds work that `open` would replace.
    fn mcp_unsaved(&self) -> bool {
        self.is_modified() || (self.document.clean.is_none() && !self.plates.is_blank())
    }

    /// Answers a request now, or starts the job that will.
    fn start_mcp(&mut self, incoming: Incoming, ctx: &egui::Context) {
        let tool = incoming.request.tool.clone();
        let args = incoming.request.args.clone();
        self.mcp.last_call = Some(tool.clone());
        let result = match tool.as_str() {
            "status" => Ok(vec![tools::text(&self.mcp_status())]),
            "undo" => self.mcp_undo(),
            "save_project" => self.mcp_save(args),
            "add_shape" => self.mcp_add_shape(args),
            "open" => match self.mcp_open(args, ctx) {
                Ok(job) => {
                    self.mcp.job = Some(job(incoming));
                    return;
                }
                Err(e) => Err(e),
            },
            "slice_summary" => {
                if let Err(e) = self.start_scene_job(incoming, Op::Summary, ctx) {
                    self.mcp_error(&e);
                }
                // The window's own slice, so the user sees what the summary describes.
                if self.mcp.job.is_some() {
                    self.slice_current(ctx);
                }
                return;
            }
            t if tools::SCENE_TOOLS.contains(&t) => {
                let op = Op::Tool(tool.clone(), args);
                if let Err(e) = self.start_scene_job(incoming, op, ctx) {
                    self.mcp_error(&e);
                }
                return;
            }
            other => Err(format!(
                "unknown tool '{other}' (this Mica window supports {})",
                tools::SCENE_TOOLS.join(", ")
            )),
        };
        self.finish_mcp(incoming, result);
    }

    fn finish_mcp(&mut self, incoming: Incoming, result: Result<Vec<Content>, String>) {
        if let Err(e) = &result {
            self.mcp_error(e);
        }
        incoming.answer(result);
    }

    /// An error the AI got: shown by the indicator, never silent.
    fn mcp_error(&mut self, e: &str) {
        self.mcp.last_error = Some((Instant::now(), e.to_string()));
    }

    /// Starts a scene tool (or the summary) on the worker, syncing the scene first when the
    /// window's project changed. Errors are answered here (the request is consumed).
    fn start_scene_job(
        &mut self,
        incoming: Incoming,
        op: Op,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let key = self.document_state();
        let kept = self.mcp.synced.take().filter(|s| s.key == key);
        let input = match &kept {
            Some(_) => None,
            None => match self.mcp_sync_input() {
                Ok(input) => Some(input),
                Err(e) => {
                    let msg = e.clone();
                    self.finish_mcp(incoming, Err(e));
                    return Err(msg);
                }
            },
        };
        let color_mix_enabled = self.prefs.features.color_mix;
        let next_view = self.mcp.next_view;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let job_key = key.clone();
        std::thread::Builder::new()
            .name("mica-mcp-scene".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut synced = match (kept, input) {
                        (Some(s), _) => s,
                        (None, Some(input)) => match open_scene(input, next_view, job_key) {
                            Ok(s) => s,
                            Err(e) => {
                                return Done {
                                    synced: None,
                                    result: Err(e),
                                    changed: false,
                                };
                            }
                        },
                        (None, None) => unreachable!("a scene or what to sync it from"),
                    };
                    synced.scene.color_mix_enabled = color_mix_enabled;
                    let before = synced.scene.revision();
                    let result = run_op(&mut synced, op);
                    let changed = synced.scene.revision() != before;
                    Done {
                        synced: Some(synced),
                        result,
                        changed,
                    }
                }));
                let done = result.unwrap_or_else(|p| Done {
                    synced: None,
                    result: Err(format!(
                        "internal error in Mica MCP ({}); the window's project is unchanged",
                        p.downcast_ref::<&str>()
                            .copied()
                            .or_else(|| p.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("panic")
                    )),
                    changed: false,
                });
                let _ = tx.send(done);
                ctx.request_repaint();
            })
            .map_err(|e| format!("starting Mica MCP's worker: {e}"))?;
        self.mcp.job = Some(Job::Scene {
            incoming,
            key: Box::new(key),
            done: rx,
        });
        Ok(())
    }

    /// The window's project as the scene is opened from it.
    fn mcp_sync_input(&self) -> Result<SyncInput, String> {
        if self.plates.is_blank() {
            return Err(
                "the Mica window has no objects; call open with a model, or ask the user to add \
                 one"
                .into(),
            );
        }
        if self.viewing_gcode() {
            return Err("the Mica window shows a G-code file; ask the user to close it".into());
        }
        let flavour = self.default_flavour();
        let project = self.build_project(flavour)?;
        let mut targets = vec![];
        let mut window_paint = vec![];
        let mut flip_candidates = vec![];
        for (pi, plate) in self.plates.plates.iter().enumerate() {
            for members in crate::save::object_groups(plate) {
                let o = &plate.objects[members[0]];
                window_paint.push(o.painting.material.clone());
                flip_candidates.push(
                    flavour == Flavour::Orca
                        && o.source.as_ref().is_some_and(|(_, t)| t.determinant() < 0.),
                );
                targets.push(Target {
                    plate: pi,
                    members,
                    unwritable: None,
                    flip: false,
                });
            }
        }
        Ok(SyncInput {
            project,
            flavour,
            targets,
            window_paint,
            flip_candidates,
            source: self.document.path.clone().unwrap_or_else(|| {
                PathBuf::from(format!("{} (Mica window)", self.document_name()))
            }),
        })
    }

    fn poll_mcp_job(&mut self, ctx: &egui::Context) {
        match self.mcp.job.take() {
            None => {}
            Some(Job::Scene {
                incoming,
                key,
                done,
            }) => match done.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {
                    self.mcp.job = Some(Job::Scene {
                        incoming,
                        key,
                        done,
                    });
                }
                Err(mpsc::TryRecvError::Disconnected) => self.finish_mcp(
                    incoming,
                    Err("Mica MCP's worker stopped; the window's project is unchanged".into()),
                ),
                Ok(done) => self.complete_scene_job(incoming, key, done),
            },
            Some(Job::Load {
                incoming,
                colors,
                path,
            }) => {
                if self.loading.is_some() {
                    self.mcp.job = Some(Job::Load {
                        incoming,
                        colors,
                        path,
                    });
                    return;
                }
                if self.plates.is_blank() {
                    let e = format!(
                        "Mica could not open {} (the window shows why)",
                        path.display()
                    );
                    self.finish_mcp(incoming, Err(e));
                    return;
                }
                if let Some(colors) = colors {
                    match self.mcp_parse_colors(&colors) {
                        Ok(c) => self.mcp_apply_colors(&c),
                        Err(e) => {
                            self.finish_mcp(incoming, Err(format!("opened, but {e}")));
                            return;
                        }
                    }
                }
                let op = Op::Tool("scene_info".into(), Value::Null);
                if let Err(e) = self.start_scene_job(incoming, op, ctx) {
                    self.mcp_error(&e);
                }
            }
        }
    }

    fn complete_scene_job(&mut self, incoming: Incoming, key: Box<DocState>, done: Done) {
        let Done {
            synced,
            mut result,
            changed,
        } = done;
        let Some(mut synced) = synced else {
            self.finish_mcp(incoming, result);
            return;
        };
        self.mcp.next_view = synced.scene.next_view_id();
        let same = self.document_state() == *key;
        let tool = incoming.request.tool.clone();
        if changed && result.is_ok() {
            if !same {
                // The user changed the project meanwhile: the scene is stale and the edit
                // would undo theirs.
                self.finish_mcp(
                    incoming,
                    Err(
                        "the project changed in the Mica window while this call ran, so nothing \
                         was applied; call render_view again and retry"
                            .into(),
                    ),
                );
                return;
            }
            match changes(&synced).and_then(|c| self.mcp_apply(&tool, c, &synced.targets)) {
                Ok(()) => {
                    synced.baseline = baseline(&synced.scene, &synced.targets);
                    synced.key = self.document_state();
                }
                Err(e) => {
                    self.finish_mcp(incoming, Err(e));
                    return;
                }
            }
        } else if !same {
            // Read-only calls stay valid answers, but the scene is not kept for the next one.
            self.mcp.synced = None;
            self.finish_mcp(incoming, result);
            return;
        }
        if let Ok(content) = &mut result {
            let undo = self
                .undo
                .undo_label()
                .is_some_and(|l| l.starts_with(UNDO_PREFIX));
            let unsaved = self.mcp_unsaved();
            patch(content, |v| {
                if v.get("undo_steps").is_some() {
                    v["undo_steps"] = json!(if undo {
                        "the window's Edit › Undo"
                    } else {
                        "none"
                    });
                }
                if tool == "scene_info" {
                    v["unsaved_changes"] = json!(unsaved);
                    v["mode"] = json!("live: the project open in the Mica window");
                }
            });
        }
        self.mcp.synced = Some(synced);
        self.finish_mcp(incoming, result);
    }

    /// Applies a scene's changes to the plates as one undo step.
    fn mcp_apply(&mut self, tool: &str, c: Changes, targets: &[Target]) -> Result<(), String> {
        if c.is_empty() {
            return Ok(());
        }
        // Everything is checked before the step is recorded.
        let mut regions = vec![];
        for &(i, k, tool) in &c.part_tool {
            let t = &targets[i];
            let o = &self.plates.plates[t.plate].objects[t.members[0]];
            if let Some(region) = &o.region {
                let index = region
                    .volumes
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.kind == PartKind::Model && !v.mesh.faces.is_empty())
                    .nth(k)
                    .map(|(n, _)| n)
                    .ok_or_else(|| {
                        format!(
                            "part {} of object {} is not in the window's object",
                            k + 1,
                            i + 1
                        )
                    })?;
                regions.push((i, index, tool));
            }
        }
        let mixes = c
            .mixes
            .map(|text| {
                text.map(|text| {
                    mica_core::full_spectrum::FullSpectrum::from_json(&text, self.tool_count())
                        .map(|s| (s, text))
                })
                .transpose()
            })
            .transpose()?;
        let label = tools::edit_label(tool).unwrap_or("edit");
        self.record(&format!("{UNDO_PREFIX}{label}"));
        let mut plates = std::collections::BTreeSet::new();
        for (i, paint) in &c.paint {
            let t = &targets[*i];
            let plate = &mut self.plates.plates[t.plate];
            let mut painting = (*plate.objects[t.members[0]].painting).clone();
            painting.material = paint.clone();
            let painting = Arc::new(painting);
            for &m in &t.members {
                plate.objects[m].painting = painting.clone();
                plate.objects[m].painting_edited = true;
            }
            plates.insert(t.plate);
        }
        for &(i, tool) in &c.object_tool {
            let t = &targets[i];
            for &m in &t.members {
                self.plates.plates[t.plate].objects[m].set_tool(tool - 1);
            }
            plates.insert(t.plate);
        }
        for &(i, _, tool) in &c.part_tool {
            // A one-part object's part is the object.
            let t = &targets[i];
            if self.plates.plates[t.plate].objects[t.members[0]]
                .region
                .is_none()
            {
                for &m in &t.members {
                    self.plates.plates[t.plate].objects[m].set_tool(tool - 1);
                }
                plates.insert(t.plate);
            }
        }
        for (i, index, tool) in regions {
            let t = &targets[i];
            let plate = &mut self.plates.plates[t.plate];
            let Some(region) = &plate.objects[t.members[0]].region else {
                continue;
            };
            let mut region = (**region).clone();
            region.volumes[index]
                .config
                .set_raw("extruder", &tool.to_string())
                .map_err(|e| format!("setting the part's tool: {e:?}"))?;
            let region = Arc::new(region);
            for &m in &t.members {
                plate.objects[m].region = Some(region.clone());
            }
            plates.insert(t.plate);
        }
        if let Some(colors) = &c.colors {
            self.mcp_apply_colors(colors);
        }
        if let Some(mixes) = mixes {
            self.load_mixes(mixes);
            self.plates.invalidate_all();
            self.stop_worker();
            self.slice_queue.clear();
        }
        for p in plates {
            self.plates.plates[p].invalidate();
        }
        self.changed();
        self.status = format!("{UNDO_PREFIX}{label}");
        Ok(())
    }

    fn mcp_parse_colors(&self, wanted: &[String]) -> Result<Vec<[u8; 3]>, String> {
        let tools = self.tool_count();
        if wanted.len() > tools {
            return Err(format!(
                "{} colors given, but this printer has {tools} tool(s)",
                wanted.len()
            ));
        }
        let mut colors = self.tool_colours();
        colors.resize(tools, [0xF5; 3]);
        for (i, w) in wanted.iter().enumerate() {
            if w.trim().is_empty() {
                continue;
            }
            colors[i] = mica_cli::mcp::colors::parse(w)
                .ok_or_else(|| format!("'{w}' is not a color; use #RRGGBB or a color name"))?;
        }
        Ok(colors)
    }

    /// Slot colors as project colors (none where it is the preset's own), as the color
    /// picker sets them.
    fn mcp_apply_colors(&mut self, colors: &[[u8; 3]]) {
        let n = self.tool_count();
        self.overlays.resize(n);
        for (tool, c) in colors.iter().enumerate().take(n) {
            let preset = self
                .profiles
                .filaments
                .get(tool)
                .and_then(|f| self.profiles.filament_colour(f));
            let colour = (preset != Some(*c)).then(|| crate::project::hex(*c));
            self.overlays.set_colour(tool, colour);
        }
        self.colours_changed();
    }

    /// The window's side of `status`.
    fn mcp_status(&self) -> Value {
        let objects: usize = self.plates.plates.iter().map(|p| p.objects.len()).sum();
        json!({
            "project": self.document.path.as_ref().map_or_else(
                || format!("{} (not saved yet)", self.document_name()),
                |p| p.display().to_string(),
            ),
            "unsaved_changes": self.mcp_unsaved(),
            "printer": self.profiles.printer,
            "tools": self.tool_count(),
            "plates": self.plates.len(),
            "current_plate": self.plates.current + 1,
            "objects": objects,
            "last_undo_step": self.undo.undo_label(),
        })
    }

    /// `add_shape`: a built-in model as a new object on the current plate or as a part of
    /// object `object` (numbered as `scene_info` numbers them: plate by plate, instances
    /// together), one undo step.
    fn mcp_add_shape(&mut self, args: Value) -> Result<Vec<Content>, String> {
        let a: tools::AddShapeArgs = tools::parse("add_shape", args)?;
        let (model, size, kind) = tools::shape_request(&a)?;
        if self.viewing_gcode() {
            return Err("the Mica window shows a G-code file; ask the user to close it".into());
        }
        let numbered = |app: &App| -> Vec<(usize, usize)> {
            app.plates
                .plates
                .iter()
                .enumerate()
                .flat_map(|(pi, plate)| {
                    crate::save::object_groups(plate)
                        .into_iter()
                        .map(move |m| (pi, m[0]))
                })
                .collect()
        };
        let add_as = match &kind {
            None => crate::builtin::AddAs::Object,
            Some(PartKind::Model) => crate::builtin::AddAs::Part,
            Some(PartKind::Modifier) => crate::builtin::AddAs::Modifier,
            Some(PartKind::NegativeVolume) => crate::builtin::AddAs::Negative,
            Some(PartKind::SupportEnforcer) => crate::builtin::AddAs::Enforcer,
            Some(PartKind::SupportBlocker) => crate::builtin::AddAs::Blocker,
            Some(PartKind::Unknown(_)) => return Err("unknown part kind".into()),
        };
        if kind.is_some() {
            let all = numbered(self);
            let (plate, index) = match a.object {
                Some(n) => *all.get((n as usize).wrapping_sub(1)).ok_or_else(|| {
                    format!("object {n} does not exist; objects are 1..={}", all.len())
                })?,
                None if all.len() == 1 => all[0],
                None => match self.selected {
                    Some(i) => (self.plates.current, i),
                    None => {
                        return Err(format!(
                            "the window has {} objects and none is selected; pass object \
                             (1..={}, see scene_info)",
                            all.len(),
                            all.len()
                        ));
                    }
                },
            };
            if plate != self.plates.current {
                self.select_plate(plate);
            }
            self.select(Some(index), false);
        }
        let label = format!("{UNDO_PREFIX}add shape");
        let placement = crate::builtin::Placement {
            size: Some(size),
            at: a.position.map(|[x, y]| glam::Vec2::new(x, y)),
        };
        let name = self.add_builtin_with(model, add_as, placement, Some(&label))?;
        let object = self.selected.and_then(|i| {
            numbered(self)
                .iter()
                .position(|&(p, o)| p == self.plates.current && o == i)
                .map(|k| k + 1)
        });
        Ok(vec![tools::text(&json!({
            "added": name,
            "kind": a.kind.as_deref().unwrap_or("object"),
            "object": object,
            "plate": self.plates.current + 1,
            "size_mm": size,
            "undo_step": label,
            "next": "render_view to look at it",
        }))])
    }

    /// `undo`: the window's last step, when it is a Mica MCP edit.
    fn mcp_undo(&mut self) -> Result<Vec<Content>, String> {
        let label = match self.undo.undo_label() {
            None => return Err("nothing to undo in the Mica window".into()),
            Some(l) if !l.starts_with(UNDO_PREFIX) => {
                return Err(format!(
                    "the last step in the Mica window is the user's own ('{l}'), not a Mica MCP \
                     edit; it was not undone. Ask the user if it should be."
                ));
            }
            Some(l) => l.to_string(),
        };
        self.undo();
        Ok(vec![tools::text(&json!({
            "undone": label,
            "next_undo_step": self.undo.undo_label(),
        }))])
    }

    /// `save_project`: a copy of the window's project; its own file only when confirmed.
    fn mcp_save(&mut self, args: Value) -> Result<Vec<Content>, String> {
        let a: tools::SaveArgs = tools::parse("save_project", args)?;
        if self.plates.is_blank() {
            return Err("the Mica window has nothing to save yet".into());
        }
        let flavour = tools::flavour(a.flavour.as_deref())?.unwrap_or(self.default_flavour());
        let path = session::resolve_save_path(a.path.as_deref(), &self.document_name())?;
        let overwrite = a.overwrite.unwrap_or(false);
        let is_open = self
            .document
            .path
            .as_ref()
            .is_some_and(|d| Scene::same_file(&path, d));
        if is_open && !(overwrite && a.replace_open_project.unwrap_or(false)) {
            return Err(format!(
                "{} is the project open in the Mica window; it is only replaced with overwrite: \
                 true and replace_open_project: true, after the user said so. Otherwise save a \
                 copy under another name (or let the user press Cmd+S).",
                path.display()
            ));
        }
        session::check_save_target(&path, None, overwrite)?;
        if is_open {
            self.save_project_to(&path, flavour)?;
            return Ok(vec![tools::text(&json!({
                "saved": path.display().to_string(),
                "note": "the window's open project was saved in place",
            }))]);
        }
        let project = self.build_project(flavour)?;
        threemf::write(&path, &project, flavour).map_err(|e| format!("{}: {e}", path.display()))?;
        self.status = format!("{UNDO_PREFIX}saved a copy as {}", path.display());
        Ok(vec![tools::text(&json!({
            "saved": path.display().to_string(),
            "format": crate::save::flavour_label(flavour),
            "note": "a copy; the window's own project file and its unsaved state are unchanged",
        }))])
    }

    /// `open`: a new project in the window, only when nothing unsaved would be lost. Returns
    /// the job that waits for the load.
    fn mcp_open(
        &mut self,
        args: Value,
        ctx: &egui::Context,
    ) -> Result<Box<dyn FnOnce(Incoming) -> Job>, String> {
        let a: tools::OpenArgs = tools::parse("open", args)?;
        let path = PathBuf::from(&a.path);
        if !path.is_file() {
            return Err(format!("{}: no such file", path.display()));
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "stl" && ext != "3mf" {
            return Err("open expects an .stl or .3mf file".into());
        }
        if self.loading.is_some() {
            return Err("the Mica window is loading a file; try again in a moment".into());
        }
        if self.mcp_unsaved() {
            return Err(format!(
                "the Mica window has unsaved changes ('{}'); open would replace them, so nothing \
                 was opened. Work on the open project (scene_info), or ask the user to save or \
                 discard their changes in Mica first.",
                self.document_name()
            ));
        }
        if a.bed.is_some() {
            return Err(
                "live mode: choose the bed type in the Mica window; call open without bed".into(),
            );
        }
        if let Some(printer) = &a.printer {
            let (vendor, name) = printer_shortcut(printer)
                .map(|(v, n)| (Some(v.to_string()), n.to_string()))
                .unwrap_or((a.vendor.clone(), printer.clone()));
            let (vi, _, name) = self
                .profiles
                .all_printers()
                .into_iter()
                .find(|(_, v, n)| *n == name && vendor.as_ref().is_none_or(|w| v == w))
                .ok_or_else(|| format!("printer '{printer}' is not known to this Mica window"))?;
            self.switch_printer(vi, &name);
        }
        if let Some(print) = &a.print {
            if !self.profiles.processes.contains(print) {
                return Err(format!(
                    "print profile '{print}' does not exist for {}",
                    self.profiles.printer
                ));
            }
            self.profiles.process = print.clone();
            self.settings_changed();
        }
        if let Some(filament) = &a.filament {
            if !self.profiles.materials.contains(filament) {
                return Err(format!(
                    "filament profile '{filament}' does not exist for {}",
                    self.profiles.printer
                ));
            }
            self.profiles.filaments.fill(filament.clone());
            self.settings_changed();
        }
        if !self.plates.is_blank() {
            self.new_project_now();
        }
        self.load_now(vec![path.clone()], ctx);
        let colors = a.filament_colors;
        Ok(Box::new(move |incoming| Job::Load {
            incoming,
            colors,
            path,
        }))
    }

    /// "Mica MCP connected" in the 3D view's corner while a client is attached, with the last
    /// error the AI got (hover for the whole text).
    pub(crate) fn mcp_indicator(&mut self, ctx: &egui::Context) {
        let clients = self.mcp.clients();
        let recent_error = self
            .mcp
            .last_error
            .as_ref()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(12))
            .map(|(_, e)| e.clone());
        if clients == 0 && recent_error.is_none() {
            return;
        }
        let p = crate::theme::of_ctx(ctx);
        let rect = self.viewport_rect;
        egui::Area::new(egui::Id::unique("mica-mcp-indicator"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom() + egui::vec2(12., -34.))
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(p.panel)
                    .stroke(egui::Stroke::new(1., p.border))
                    .corner_radius(10.)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, text) = if clients > 0 {
                                (p.accent, "Mica MCP connected".to_string())
                            } else {
                                (p.muted, "Mica MCP".to_string())
                            };
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(8., 8.), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, dot);
                            let busy = if self.mcp.busy() {
                                " · working…"
                            } else {
                                ""
                            };
                            let label = ui.label(
                                egui::RichText::new(format!("{text}{busy}"))
                                    .font(crate::theme::sans(11.))
                                    .color(p.muted),
                            );
                            let label = match &self.mcp.last_call {
                                Some(c) => label.on_hover_text(format!(
                                    "An AI assistant works on this project. Last call: {c}. Its \
                                     edits are undo steps named \"Mica MCP: …\"."
                                )),
                                None => label,
                            };
                            let _ = label;
                            if let Some(e) = &recent_error {
                                let short: String = e.chars().take(60).collect();
                                ui.label(
                                    egui::RichText::new(format!(
                                        "· {short}{}",
                                        if e.chars().count() > 60 { "…" } else { "" }
                                    ))
                                    .font(crate::theme::sans(11.))
                                    .color(ui.visuals().warn_fg_color),
                                )
                                .on_hover_text(e);
                            }
                        });
                    });
            });
        if recent_error.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}
