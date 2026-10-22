// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The quick tour of the main window (Welcome › Take a quick tour, Help › Show quick tour):
//! seven steps, each pointing at one part of the window. The window dims except a rounded
//! cutout around the step's control, and a card beside it says what the control is for.
//!
//! Controls are found by [`Anchor`]: where each one is drawn, it registers its rect for the
//! frame ([`anchor`]); the tour, drawn last, reads them back. A step whose control is not on
//! screen shows its card in the middle of the window without a cutout.
//!
//! The texts live in [`TOUR_STEPS`]. Next / Back glide the cutout to the next control (a short
//! ease-out move) while the card fades out and in at its new place; with the system's Reduce
//! motion setting on, the step changes as a crossfade instead. While the tour runs, the window
//! behind it takes no clicks, keys or menu commands. It starts only when asked, never by
//! itself (tests, screenshots and the monkey test start it only through `--tour`).

use crate::app::App;
use crate::theme;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, LayerId, Mesh, Order, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Vec2, text::LayoutJob,
};

/// A control a step can point at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// The Library · Prepare · Preview · Machine switch in the header.
    Tabs,
    /// The sidebar's Printer and Filament cards.
    PrinterFilament,
    /// The Process card from "Settings for" down: topic rail and Essentials.
    Settings,
    /// The plate bar at the bottom of the 3D view.
    PlateBar,
    /// The tool bar above the bed.
    Toolbar,
    /// The header's right side: Slice (or the totals), Export and Print….
    SliceExport,
    /// The Machine segment of the header switch.
    MachineTab,
}

/// Which part of the window a step shows; the tour switches to it before moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Prepare,
    /// Preview of the sliced plate (the step slices it first when needed).
    Preview,
    Machine,
}

/// One step of the tour. `body` paragraphs mark key words with `**bold**`; `{slice_all}`
/// stands for the Slice all plates shortcut as this platform writes it (the bundled fonts have
/// no ⌘ or ⋯ glyphs, so shortcuts are spelled out as in the menus).
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub anchor: Anchor,
    pub view: View,
    pub title: &'static str,
    pub body: &'static [&'static str],
}

/// The tour's steps, in order. All of the tour's wording is here.
pub const TOUR_STEPS: [Step; 7] = [
    Step {
        anchor: Anchor::Tabs,
        view: View::Prepare,
        title: "Four tabs, one project",
        body: &[
            "**Library** finds the models on your disk, grouped by project.",
            "**Prepare** is where you place models and pick settings.",
            "**Preview** shows the sliced layers before you print.",
            "**Machine** shows your printers, their cameras and jobs.",
        ],
    },
    Step {
        anchor: Anchor::PrinterFilament,
        view: View::Prepare,
        title: "Printer and filaments",
        body: &[
            "Pick the **printer** you print on; its bed and nozzle come with it.",
            "Each **filament slot** has its own material and color. On a multi-tool printer, \
             objects and painted areas use these slots.",
        ],
    },
    Step {
        anchor: Anchor::Settings,
        view: View::Prepare,
        title: "The settings you change most",
        body: &[
            "**Essentials** holds the dozen settings most prints need. The topics on the left \
             lead to the rest.",
            "**Search all topics** finds any setting by name. **Changed** lists only what \
             differs from the profile.",
            "Settings for **All objects** can be switched to one object to give it its own \
             values.",
        ],
    },
    Step {
        anchor: Anchor::PlateBar,
        view: View::Prepare,
        title: "Plates",
        body: &[
            "A project can hold several **plates**. Click the name to rename it; the \
             **three-dot menu** adds, duplicates and reorders plates.",
        ],
    },
    Step {
        anchor: Anchor::Toolbar,
        view: View::Prepare,
        title: "Tools for the selected object",
        body: &[
            "**Move**, **rotate**, **scale**, **cut**, **paint** and add **text**.",
            "**Right-click** an object for everything else.",
        ],
    },
    Step {
        anchor: Anchor::SliceExport,
        view: View::Preview,
        title: "Slice, then print or export",
        body: &[
            "**Slice** turns the plate into G-code and opens Preview. **{slice_all}** slices \
             every plate at once.",
            "**Print…** sends the job to a printer you added; **Export** saves the file \
             instead.",
            "If something stops a slice, the reason shows on the plate bar and in a \
             notification.",
        ],
    },
    Step {
        anchor: Anchor::MachineTab,
        view: View::Machine,
        title: "Your printers",
        body: &[
            "**Add a printer** to send jobs, watch its camera and see what is loaded.",
            "**Help › Show quick tour** brings this tour back.",
        ],
    },
];

/// How long the cutout takes to glide to the next control, in seconds.
pub const MOVE_S: f64 = 0.25;
/// The old card fades out during the first part of a move, the new one in during the rest.
const FADE_OUT_S: f64 = 0.1;
const CARD_W: f32 = 400.;
const PAD: f32 = 20.;
/// Room around the control inside the cutout, and the cutout's corner radius.
const CUT_PAD: f32 = 6.;
const CUT_RADIUS: f32 = 10.;
/// Space between the cutout and the card (the pointer sits in it).
const GAP: f32 = 16.;
const POINTER: f32 = 9.;
/// Closest the card comes to the window's edges.
const MARGIN: f32 = 16.;
/// The dimmed window: black at about 62 % opacity.
const DIM_ALPHA: u8 = 158;

/// The controls drawn this frame (`anchor`), by pass number.
#[derive(Clone, Default)]
struct Anchors {
    pass: u64,
    rects: Vec<(Anchor, Rect)>,
    /// Scroll this control into view when it is next drawn (a step inside the sidebar).
    scroll: Option<Anchor>,
}

fn anchors_id() -> egui::Id {
    egui::Id::unique("mica tour anchors")
}

/// Records where `anchor`'s control was drawn this frame, cut to what `ui` shows (a control
/// half scrolled out of the sidebar keeps its visible part). Drawn twice in a frame, the
/// rects are joined.
pub fn anchor(ui: &egui::Ui, anchor: Anchor, rect: Rect) {
    let ctx = ui.ctx();
    let pass = ctx.cumulative_pass_nr();
    let scroll = ctx.data_mut(|d| {
        let a = d.get_temp_mut_or_default::<Anchors>(anchors_id());
        if a.pass != pass {
            a.pass = pass;
            a.rects.clear();
        }
        let wanted = a.scroll == Some(anchor);
        if wanted {
            a.scroll = None;
        }
        let shown = rect.intersect(ui.clip_rect());
        if shown.width() >= 8. && shown.height() >= 8. {
            match a.rects.iter_mut().find(|(k, _)| *k == anchor) {
                Some((_, r)) => *r = r.union(shown),
                None => a.rects.push((anchor, shown)),
            }
        }
        wanted
    });
    if scroll {
        // Its top into view (most of a tall card is enough).
        let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), rect.height().min(220.)));
        ui.scroll_to_rect(top, None);
    }
}

/// Where `anchor`'s control was drawn this frame, if it was.
pub fn anchored(ctx: &egui::Context, anchor: Anchor) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| {
        d.get_temp::<Anchors>(anchors_id())
            .filter(|a| a.pass == pass)
            .and_then(|a| a.rects.iter().find(|(k, _)| *k == anchor).map(|(_, r)| *r))
    })
}

/// Where `anchor`'s control was drawn in the frame that just ended (tests, between frames).
#[cfg(test)]
pub fn drawn_last_frame(ctx: &egui::Context, anchor: Anchor) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| {
        d.get_temp::<Anchors>(anchors_id())
            .filter(|a| a.pass + 1 == pass)
            .and_then(|a| a.rects.iter().find(|(k, _)| *k == anchor).map(|(_, r)| *r))
    })
}

fn request_scroll(ctx: &egui::Context, anchor: Anchor) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Anchors>(anchors_id()).scroll = Some(anchor));
}

/// The running tour.
#[derive(Clone, Debug)]
pub struct Tour {
    /// The current step (index into [`TOUR_STEPS`]).
    pub step: usize,
    /// The step being left and where its cutout was, while a move runs.
    from: Option<(usize, Option<Rect>)>,
    /// When the current move started (`egui` input time).
    started: f64,
    /// Crossfade instead of moving (the system's Reduce motion setting).
    pub reduce_motion: bool,
    /// The cutout as last drawn (the next move starts there).
    shown: Option<Rect>,
    /// Where the window was before the tour (restored at the end).
    back_to: (bool, bool, bool),
    /// The step's view was set up (selection, slicing) after the last move.
    entered: bool,
}

/// What a frame of the tour asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Next,
    Back,
    Skip,
}

impl Tour {
    fn new(reduce_motion: bool, back_to: (bool, bool, bool)) -> Self {
        Self {
            step: 0,
            from: None,
            started: f64::NEG_INFINITY,
            reduce_motion,
            shown: None,
            back_to,
            entered: false,
        }
    }

    /// 0 → 1 over the current move; 1 when settled.
    pub fn progress(&self, now: f64) -> f32 {
        if self.from.is_none() {
            return 1.;
        }
        ((now - self.started) / MOVE_S).clamp(0., 1.) as f32
    }

    pub fn moving(&self, now: f64) -> bool {
        self.progress(now) < 1.
    }
}

/// Ease-out (cubic): fast start, gentle arrival.
pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min.lerp(b.min, t), a.max.lerp(b.max, t))
}

/// The system's Reduce motion setting (macOS: Accessibility › Display). egui and winit don't
/// pass it on, so it is read from AppKit when the tour starts; elsewhere motion stays on.
pub fn system_reduce_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

impl App {
    /// The project has nothing to show yet: no objects on any plate, no G-code file open.
    fn tour_project_empty(&self) -> bool {
        self.gcode.is_none()
            && (0..self.plates.len()).all(|i| self.plates.plate(i).objects.is_empty())
    }

    /// The tour runs on a sample plate: an empty project gets the Benchy (as the Welcome
    /// window's "Try a Benchy"); a project with something in it is left as it is.
    pub(crate) fn tour_sample(&mut self) {
        if self.tour_project_empty() && self.loading.is_none() {
            self.add_builtin(
                mica_core::builtin_models::BuiltinModel::Benchy,
                crate::builtin::AddAs::Object,
            );
        }
    }

    /// Starts the tour on its first step (Welcome › Take a quick tour, Help › Show quick tour).
    pub(crate) fn start_tour(&mut self, ctx: &egui::Context) {
        if self.dialogs.welcome.is_some() {
            self.close_welcome();
        }
        self.tour_sample();
        if !self.prefs.tour_seen {
            self.prefs.tour_seen = true;
            self.prefs_dirty = true;
        }
        egui::Popup::close_all(ctx);
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        let reduce = !self.headless && system_reduce_motion();
        self.tour = Some(Tour::new(
            reduce,
            (self.library.shown, self.machine, self.preview),
        ));
        self.tour_enter(ctx);
    }

    /// Shows step `step` at once, settled (screenshots and tests).
    pub(crate) fn tour_jump(&mut self, step: usize, ctx: &egui::Context) {
        if let Some(t) = &mut self.tour {
            t.step = step.min(TOUR_STEPS.len() - 1);
            t.from = None;
            t.entered = false;
        }
        self.tour_enter(ctx);
    }

    /// Ends the tour (Done, Skip tour, Esc) and returns to where the window was.
    pub(crate) fn end_tour(&mut self) {
        if let Some(t) = self.tour.take() {
            let (library, machine, preview) = t.back_to;
            self.library.shown = library;
            self.machine = machine;
            self.preview = preview && self.finished().is_some();
        }
    }

    /// Next, Back or Skip.
    pub(crate) fn tour_nav(&mut self, nav: Nav, ctx: &egui::Context) {
        let Some(t) = &mut self.tour else {
            return;
        };
        let to = match nav {
            Nav::Skip => None,
            Nav::Next if t.step + 1 >= TOUR_STEPS.len() => None,
            Nav::Next => Some(t.step + 1),
            Nav::Back if t.step == 0 => return,
            Nav::Back => Some(t.step - 1),
        };
        let Some(to) = to else {
            self.end_tour();
            return;
        };
        let now = ctx.input(|i| i.time);
        t.from = Some((t.step, t.shown));
        t.step = to;
        t.started = now;
        t.entered = false;
        // The tab first, then the move.
        self.tour_enter(ctx);
        ctx.request_repaint();
    }

    /// Switches to the current step's view; on entering it, also sets up what it shows (a
    /// slice for Preview, the selection for the settings and tool steps).
    fn tour_enter(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.tour else {
            return;
        };
        let step = TOUR_STEPS[t.step];
        let entering = !t.entered;
        self.library.shown = false;
        match step.view {
            View::Prepare => {
                self.machine = false;
                if !self.viewing_gcode() {
                    self.preview = false;
                }
            }
            View::Preview => {
                self.machine = false;
                if self.finished().is_some() {
                    self.preview = true;
                } else if entering
                    && self.worker.is_none()
                    && self.config.is_ok()
                    && !self.plates.current().objects.is_empty()
                {
                    // Opens Preview once done (a manual slice).
                    self.slice_current(ctx);
                }
            }
            View::Machine => self.machine = true,
        }
        if entering {
            match step.anchor {
                // "Settings for All objects": the scope line, not one object's scope bar.
                Anchor::Settings if self.selected.is_some() => self.select(None, false),
                // The tools act on a selected object.
                Anchor::Toolbar
                    if self.selected.is_none() && !self.plates.current().objects.is_empty() =>
                {
                    self.select(Some(0), false)
                }
                _ => {}
            }
            if matches!(step.anchor, Anchor::PrinterFilament | Anchor::Settings) {
                request_scroll(ctx, step.anchor);
            }
            if let Some(t) = &mut self.tour {
                t.entered = true;
            }
        }
    }

    /// Start of a frame with the tour running: its keys (taken so nothing behind sees them),
    /// and the step's view kept (a slice finishing on another step must not switch tabs).
    /// False when no tour runs.
    pub(crate) fn tour_begin_frame(&mut self, ctx: &egui::Context) -> bool {
        if self.tour.is_none() {
            return false;
        }
        let nav = ctx.input_mut(|i| {
            let mut nav = None;
            for (key, n) in [
                (Key::ArrowRight, Nav::Next),
                (Key::Enter, Nav::Next),
                (Key::ArrowLeft, Nav::Back),
                (Key::Escape, Nav::Skip),
            ] {
                if i.consume_key(egui::Modifiers::NONE, key) {
                    nav = nav.or(Some(n));
                }
            }
            // Nothing behind the tour reacts to the keyboard.
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            });
            nav
        });
        if let Some(n) = nav {
            self.tour_nav(n, ctx);
        }
        if self.tour.is_some() {
            self.tour_enter(ctx);
        }
        self.tour.is_some()
    }

    /// Draws the tour over the whole window (last in the frame, so every anchor is known).
    pub(crate) fn tour_ui(&mut self, ctx: &egui::Context) {
        let Some(tour) = self.tour.clone() else {
            return;
        };
        let p = theme::of_ctx(ctx);
        let screen = ctx.content_rect();
        let now = ctx.input(|i| i.time);
        let step = TOUR_STEPS[tour.step];
        let pad = |r: Rect| r.expand(CUT_PAD).intersect(screen.shrink(2.));
        let target = anchored(ctx, step.anchor).map(pad);
        let t = tour.progress(now);
        let (from_step, from_rect) = tour.from.unwrap_or((tour.step, None));
        let id = egui::Id::unique("mica tour");
        let layer = LayerId::new(Order::Foreground, id);
        let mut nav = None;
        egui::Area::new(id)
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .constrain(false)
            .show(ctx, |ui| {
                // Clicks anywhere outside the card land here and do nothing.
                ui.allocate_rect(screen, Sense::click_and_drag());
                let painter = ui.painter();
                let plan = plan(from_rect, target, t, tour.reduce_motion);
                match plan {
                    Cutout::Crossfade { from, to, k } => {
                        dim(painter, screen, from, 1. - k, p.accent);
                        dim(painter, screen, to, k, p.accent);
                    }
                    Cutout::At(cut) => dim(painter, screen, cut, 1., p.accent),
                }
                if let Some(tour) = &mut self.tour {
                    tour.shown = match plan {
                        Cutout::At(cut) => cut,
                        Cutout::Crossfade { to, .. } => to,
                    };
                }
                // The card: the old one fades out at its place, the new one in at its own.
                let fade_out = (FADE_OUT_S / MOVE_S) as f32;
                if t < fade_out && from_step != tour.step {
                    let a = 1. - t / fade_out;
                    let old = TOUR_STEPS[from_step];
                    let _ = card(ui, &old, from_step, from_rect, screen, a, false);
                } else {
                    let a = if from_step != tour.step {
                        ((t - fade_out) / (1. - fade_out)).clamp(0., 1.)
                    } else {
                        1.
                    };
                    nav = card(ui, &step, tour.step, target, screen, a, true);
                }
            });
        // Above everything else in the window (menus, notifications, windows).
        ctx.move_to_top(layer);
        if tour.moving(now) {
            ctx.request_repaint();
        }
        if let Some(n) = nav {
            self.tour_nav(n, ctx);
        }
    }
}

/// What a frame of a step change draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cutout {
    /// One cutout (moving or settled; `None`: the whole window dimmed).
    At(Option<Rect>),
    /// The old cutout fading out while the new one fades in (`k`: 0 → 1).
    Crossfade {
        from: Option<Rect>,
        to: Option<Rect>,
        k: f32,
    },
}

/// The cutout at move progress `t` (0 → 1) from `from` to `to`: it glides (ease-out) between
/// two controls; with Reduce motion, or when either end has no cutout, it crossfades instead.
pub fn plan(from: Option<Rect>, to: Option<Rect>, t: f32, reduce_motion: bool) -> Cutout {
    if t >= 1. {
        return Cutout::At(to);
    }
    match (from, to) {
        (Some(a), Some(b)) if !reduce_motion => Cutout::At(Some(lerp_rect(a, b, ease_out(t)))),
        _ => Cutout::Crossfade {
            from,
            to,
            k: ease_out(t),
        },
    }
}

/// The window dimmed except `cut` (rounded, with an accent outline), at `alpha` × the full dim.
fn dim(painter: &egui::Painter, screen: Rect, cut: Option<Rect>, alpha: f32, accent: Color32) {
    if alpha <= 0. {
        return;
    }
    let color = Color32::from_black_alpha((DIM_ALPHA as f32 * alpha).round() as u8);
    let Some(cut) = cut else {
        painter.rect_filled(screen, 0., color);
        return;
    };
    let mut mesh = Mesh::default();
    // Four bands around the cutout…
    for band in [
        Rect::from_min_max(screen.min, Pos2::new(screen.max.x, cut.min.y)),
        Rect::from_min_max(Pos2::new(screen.min.x, cut.max.y), screen.max),
        Rect::from_min_max(
            Pos2::new(screen.min.x, cut.min.y),
            Pos2::new(cut.min.x, cut.max.y),
        ),
        Rect::from_min_max(
            Pos2::new(cut.max.x, cut.min.y),
            Pos2::new(screen.max.x, cut.max.y),
        ),
    ] {
        if band.is_positive() {
            mesh.add_colored_rect(band, color);
        }
    }
    // …and its four rounded corners, each a fan from the corner to its quarter arc.
    let r = CUT_RADIUS.min(cut.width() / 2.).min(cut.height() / 2.);
    let corners = [
        (cut.left_top(), Vec2::new(1., 1.), std::f32::consts::PI),
        (
            cut.right_top(),
            Vec2::new(-1., 1.),
            1.5 * std::f32::consts::PI,
        ),
        (cut.right_bottom(), Vec2::new(-1., -1.), 0.),
        (
            cut.left_bottom(),
            Vec2::new(1., -1.),
            0.5 * std::f32::consts::PI,
        ),
    ];
    const SEGMENTS: usize = 8;
    for (corner, inward, start) in corners {
        let centre = corner + inward * r;
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(corner, color);
        for s in 0..=SEGMENTS {
            let a = start + std::f32::consts::FRAC_PI_2 * s as f32 / SEGMENTS as f32;
            mesh.colored_vertex(centre + r * Vec2::new(a.cos(), a.sin()), color);
        }
        for s in 0..SEGMENTS as u32 {
            mesh.add_triangle(base, base + 1 + s, base + 2 + s);
        }
    }
    painter.add(Shape::mesh(mesh));
    painter.rect_stroke(
        cut,
        r,
        Stroke::new(2., accent.gamma_multiply(alpha)),
        StrokeKind::Outside,
    );
}

/// Which side of the cutout the card sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
    Below,
    Above,
}

/// Where a card of `size` goes next to `cut`: on the side with the most room (below or above a
/// wide control when it fits), lined up with the cutout and kept inside the window. `None` (no cutout): the window's middle.
pub fn place(screen: Rect, cut: Option<Rect>, size: Vec2) -> (Rect, Option<Side>) {
    let inner = screen.shrink(MARGIN);
    let Some(cut) = cut else {
        return (Rect::from_center_size(screen.center(), size), None);
    };
    let room = [
        (Side::Right, inner.right() - cut.right() - GAP - size.x),
        (Side::Left, cut.left() - inner.left() - GAP - size.x),
        (Side::Below, inner.bottom() - cut.bottom() - GAP - size.y),
        (Side::Above, cut.top() - inner.top() - GAP - size.y),
    ];
    let best = |sides: &[(Side, f32)]| {
        sides
            .iter()
            .copied()
            .fold((Side::Right, f32::NEG_INFINITY), |best, s| {
                if s.1 > best.1 { s } else { best }
            })
    };
    // A wide, flat control (the header's switch, a toolbar) reads best with the card under or
    // over it, when it fits there.
    let wide = cut.width() >= 3. * cut.height();
    let (side, _) = match best(&room[2..]) {
        vertical if wide && vertical.1 >= 0. => vertical,
        _ => best(&room),
    };
    let clamp = |v: f32, lo: f32, hi: f32| if hi < lo { lo } else { v.clamp(lo, hi) };
    let min = match side {
        Side::Right | Side::Left => {
            let x = if side == Side::Right {
                cut.right() + GAP
            } else {
                cut.left() - GAP - size.x
            };
            let y = clamp(
                cut.center().y - size.y / 2.,
                inner.top(),
                inner.bottom() - size.y,
            );
            Pos2::new(x, y)
        }
        Side::Below | Side::Above => {
            let y = if side == Side::Below {
                cut.bottom() + GAP
            } else {
                cut.top() - GAP - size.y
            };
            let x = clamp(
                cut.center().x - size.x / 2.,
                inner.left(),
                inner.right() - size.x,
            );
            Pos2::new(x, y)
        }
    };
    // Never outside the window, even when no side has room.
    let mut rect = Rect::from_min_size(min, size);
    let dx = (inner.left() - rect.left()).max(0.) - (rect.right() - inner.right()).max(0.);
    let dy = (inner.top() - rect.top()).max(0.) - (rect.bottom() - inner.bottom()).max(0.);
    rect = rect.translate(Vec2::new(dx, dy));
    (rect, Some(side))
}

/// A body paragraph with its placeholders filled in.
pub fn body_text(text: &str) -> String {
    text.replace(
        "{slice_all}",
        &crate::keymap::shortcut_text(crate::keymap::Act::SliceAll),
    )
}

/// The body text with `**bold**` key words.
fn body_job(text: &str, width: f32, p: &theme::Palette) -> LayoutJob {
    let mut job = LayoutJob {
        wrap: egui::text::TextWrapping::wrap_at_width(width),
        ..Default::default()
    };
    for (i, part) in text.split("**").enumerate() {
        if part.is_empty() {
            continue;
        }
        let bold = i % 2 == 1;
        job.append(
            part,
            0.,
            egui::TextFormat {
                font_id: if bold {
                    theme::semibold(13.5)
                } else {
                    theme::sans(13.5)
                },
                color: if bold { p.text } else { p.label },
                line_height: Some(20.),
                ..Default::default()
            },
        );
    }
    job
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Filled,
    Outlined,
    Text,
}

const BUTTON_H: f32 = 30.;

fn button_width(painter: &egui::Painter, label: &str) -> f32 {
    painter
        .layout_no_wrap(label.into(), theme::semibold(13.), Color32::WHITE)
        .size()
        .x
        + 28.
}

/// A footer button, painted at `rect`; true when clicked.
fn button(ui: &egui::Ui, rect: Rect, label: &str, kind: ButtonKind, live: bool) -> bool {
    let p = theme::of(ui);
    let response = ui.interact(
        rect,
        egui::Id::unique(("mica tour button", label)),
        if live { Sense::click() } else { Sense::hover() },
    );
    let hovered = live && response.hovered();
    if hovered {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    let (fill, stroke, color) = match kind {
        ButtonKind::Filled => {
            let f = if hovered { p.accent_bright } else { p.accent };
            (f, f, p.on_accent)
        }
        ButtonKind::Outlined => (
            if hovered {
                p.hover
            } else {
                Color32::TRANSPARENT
            },
            p.border_raised,
            p.text,
        ),
        ButtonKind::Text => (
            if hovered {
                p.hover
            } else {
                Color32::TRANSPARENT
            },
            Color32::TRANSPARENT,
            if hovered { p.text } else { p.muted_alt },
        ),
    };
    painter.rect_filled(rect, 6., fill);
    if stroke != Color32::TRANSPARENT {
        painter.rect_stroke(rect, 6., Stroke::new(1., stroke), StrokeKind::Inside);
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        if kind == ButtonKind::Filled {
            theme::semibold(13.)
        } else {
            theme::medium(13.)
        },
        color,
    );
    live && response.clicked()
}

/// The step's card beside `cut`, at opacity `alpha`; `live` cards take clicks. Returns the
/// button pressed.
fn card(
    ui: &mut egui::Ui,
    step: &Step,
    index: usize,
    cut: Option<Rect>,
    screen: Rect,
    alpha: f32,
    live: bool,
) -> Option<Nav> {
    if alpha <= 0. {
        return None;
    }
    let p = theme::of(ui);
    let inner_w = CARD_W - 2. * PAD;
    let painter = ui.painter().clone();
    let mut label = LayoutJob::simple_singleline(
        format!("STEP {} OF {}", index + 1, TOUR_STEPS.len()),
        theme::semibold(11.),
        p.accent_bright,
    );
    label.sections[0].format.extra_letter_spacing = 0.8;
    let label = painter.layout_job(label);
    let title = painter.layout(step.title.into(), theme::semibold(20.), p.text, inner_w);
    let body: Vec<_> = step
        .body
        .iter()
        .map(|t| painter.layout_job(body_job(&body_text(t), inner_w, p)))
        .collect();
    let body_h: f32 = body.iter().map(|g| g.size().y).sum::<f32>() + 8. * (body.len() as f32 - 1.);
    let height = PAD + label.size().y + 8. + title.size().y + 10. + body_h + 20. + BUTTON_H + PAD;
    let (rect, side) = place(screen, cut, Vec2::new(CARD_W, height));
    ui.multiply_opacity(alpha);
    let painter = ui.painter().clone();
    let shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    painter.add(shadow.as_shape(rect, CornerRadius::same(12)));
    painter.rect_filled(rect, 12., p.panel);
    // The pointer toward the cutout, the card's colour, its outer edges outlined.
    if let (Some(cut), Some(side)) = (cut, side) {
        let r = 12. + POINTER + 2.;
        let (base_a, base_b, tip) = match side {
            Side::Right | Side::Left => {
                let y = cut.center().y.clamp(rect.top() + r, rect.bottom() - r);
                let x = if side == Side::Right {
                    rect.left()
                } else {
                    rect.right()
                };
                let out = if side == Side::Right {
                    -POINTER
                } else {
                    POINTER
                };
                (
                    Pos2::new(x, y - POINTER),
                    Pos2::new(x, y + POINTER),
                    Pos2::new(x + out, y),
                )
            }
            Side::Below | Side::Above => {
                let x = cut.center().x.clamp(rect.left() + r, rect.right() - r);
                let y = if side == Side::Below {
                    rect.top()
                } else {
                    rect.bottom()
                };
                let out = if side == Side::Below {
                    -POINTER
                } else {
                    POINTER
                };
                (
                    Pos2::new(x - POINTER, y),
                    Pos2::new(x + POINTER, y),
                    Pos2::new(x, y + out),
                )
            }
        };
        painter.rect_stroke(
            rect,
            12.,
            Stroke::new(1., p.border_soft),
            StrokeKind::Inside,
        );
        painter.add(Shape::convex_polygon(
            vec![base_a, tip, base_b],
            p.panel,
            Stroke::NONE,
        ));
        let edge = Stroke::new(1., p.border_soft);
        painter.line_segment([base_a, tip], edge);
        painter.line_segment([tip, base_b], edge);
        // The card's border under the pointer's base is covered.
        let inset = |a: Pos2, b: Pos2| match side {
            Side::Right => (a + Vec2::X * 0.5, b + Vec2::X * 0.5),
            Side::Left => (a - Vec2::X * 0.5, b - Vec2::X * 0.5),
            Side::Below => (a + Vec2::Y * 0.5, b + Vec2::Y * 0.5),
            Side::Above => (a - Vec2::Y * 0.5, b - Vec2::Y * 0.5),
        };
        let (a, b) = inset(base_a, base_b);
        painter.line_segment([a, b], Stroke::new(1.5, p.panel));
    } else {
        painter.rect_stroke(
            rect,
            12.,
            Stroke::new(1., p.border_soft),
            StrokeKind::Inside,
        );
    }
    let x = rect.left() + PAD;
    let mut y = rect.top() + PAD;
    let label_h = label.size().y;
    painter.galley(Pos2::new(x, y), label, p.accent_bright);
    y += label_h + 8.;
    let title_h = title.size().y;
    painter.galley(Pos2::new(x, y), title, p.text);
    y += title_h + 10.;
    for g in body {
        let h = g.size().y;
        painter.galley(Pos2::new(x, y), g, p.label);
        y += h + 8.;
    }
    // Footer: progress dots, then Skip tour · Back · Next.
    let row = Rect::from_min_max(
        Pos2::new(x, rect.bottom() - PAD - BUTTON_H),
        Pos2::new(rect.right() - PAD, rect.bottom() - PAD),
    );
    let mut dx = row.left();
    for i in 0..TOUR_STEPS.len() {
        let w = if i == index { 18. } else { 6. };
        let dot = Rect::from_min_size(Pos2::new(dx, row.center().y - 3.), Vec2::new(w, 6.));
        let color = if i == index {
            p.accent
        } else if i < index {
            p.accent.gamma_multiply(0.45)
        } else {
            p.toggle_off
        };
        painter.rect_filled(dot, 3., color);
        dx += w + 6.;
    }
    let last = index + 1 == TOUR_STEPS.len();
    let next_label = if last { "Done" } else { "Next" };
    let mut right = row.right();
    let mut slot = |label: &str, gap: f32| {
        let w = button_width(&painter, label);
        let r = Rect::from_min_max(
            Pos2::new(right - w, row.top()),
            Pos2::new(right, row.bottom()),
        );
        right -= w + gap;
        r
    };
    let next_r = slot(next_label, 8.);
    let back_r = (index > 0).then(|| slot("Back", 4.));
    let skip_r = (!last).then(|| slot("Skip tour", 0.));
    let mut nav = None;
    if button(ui, next_r, next_label, ButtonKind::Filled, live) {
        nav = Some(Nav::Next);
    }
    if let Some(r) = back_r
        && button(ui, r, "Back", ButtonKind::Outlined, live)
    {
        nav = Some(Nav::Back);
    }
    if let Some(r) = skip_r
        && button(ui, r, "Skip tour", ButtonKind::Text, live)
    {
        nav = Some(Nav::Skip);
    }
    nav
}
