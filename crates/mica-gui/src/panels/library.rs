// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Library view (fourth header tab): library folders and filters on the left, the
//! search bar and the thumbnail grid or list in the middle, the selected model's details on
//! the right. Also the Library section of Preferences.

use super::widgets;
use crate::app::App;
use crate::library::{self, DATES, Entry, LibraryView, OpenPrompt, SIZES};
use crate::preset_names;
use crate::theme;
use eframe::egui::{self, Align, Align2, Layout, Pos2, Rect, Sense, Stroke, Vec2};
use mica_library::group::{self, GroupBy};
use mica_library::{Colours, FolderState, FolderStatus, Grouping, Kind, Model, Sliced, Sort};
use std::path::{Path, PathBuf};

const LEFT_W: f32 = 270.;
const DETAIL_W: f32 = 320.;
const CARD_W: f32 = 184.;
const CARD_H: f32 = 214.;
const THUMB_H: f32 = 140.;
const GAP: f32 = 12.;
const ROW_H: f32 = 30.;
/// A section header in the grid.
const HEADER_H: f32 = 36.;

/// What a click in the view asks for; applied after drawing.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Select(Option<String>),
    /// ⌘ or Shift-click: adds a model to the picked ones (dragged together) or takes it out.
    Pick(String),
    /// Double-click / the Open button.
    Open(String),
    AddToPlate(PathBuf),
    OpenProject(PathBuf),
    /// A G-code file: shown read-only in Preview over the plate.
    OpenGcode(PathBuf),
    NewProjectWith(PathBuf),
    Reveal(PathBuf),
    Favourite(String),
    AddTag(String, String),
    RemoveTag(String, String),
    AddFolder(PathBuf),
    PickFolder,
    RemoveFolder(PathBuf),
    Rescan(Option<PathBuf>),
    Retry,
    FolderFilter(Option<PathBuf>),
    ClosePrompt,
    /// Show a project's files (its folder), or the whole library again.
    OpenFolder(Option<PathBuf>),
    /// Add several files (a project's parts) to the plate.
    AddAllToPlate(Vec<PathBuf>),
    /// Override the project detection for a folder (`None`: automatic again).
    SetGrouping(PathBuf, Option<Grouping>),
}

fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn muted(ui: &mut egui::Ui, text: impl Into<String>, size: f32) -> egui::Response {
    let p = theme::of(ui);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .font(theme::sans(size))
                .color(p.muted),
        )
        .wrap(),
    )
}

fn library_error(ui: &mut egui::Ui, error: &str, acts: &mut Vec<Act>) {
    ui.label(
        egui::RichText::new(error)
            .font(theme::semibold(16.))
            .color(theme::DANGER),
    );
    muted(
        ui,
        "Resolve the access problem, then try again. You can keep working in Mica.",
        12.,
    );
    if widgets::button(ui, "Retry", true).clicked() {
        acts.push(Act::Retry);
    }
}

/// A row of mutually exclusive pills; the chosen index when clicked.
/// One rounded pill; `on` draws the selected variant.
fn pill(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    let p = theme::of(ui);
    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::sans(12.), p.text);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 18., 24.), Sense::click());
    let fill = if on {
        p.accent_surface
    } else if resp.hovered() {
        p.hover
    } else {
        p.raised
    };
    ui.painter().rect_filled(r, 12., fill);
    ui.painter().rect_stroke(
        r,
        12.,
        Stroke::new(1., if on { p.accent } else { p.border_raised }),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        theme::sans(12.),
        if on { p.accent_hi } else { p.label },
    );
    if resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    resp
}

/// A row of mutually exclusive pills; the chosen index when clicked.
fn pills(ui: &mut egui::Ui, labels: &[&str], current: usize) -> Option<usize> {
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
        for (i, l) in labels.iter().enumerate() {
            if pill(ui, l, i == current).clicked() {
                out = Some(i);
            }
        }
    });
    out
}

/// A toggling pill (several may be on); call inside a wrapped row.
fn pill_toggle(ui: &mut egui::Ui, label: &str, on: bool) -> bool {
    pill(ui, label, on).clicked()
}

fn swatches(ui: &mut egui::Ui, colours: &[String], size: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.;
        for c in colours.iter().take(8) {
            if let Some(c) = library::parse_colour(c) {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
                ui.painter().circle_filled(r.center(), size / 2., c);
                ui.painter().circle_stroke(
                    r.center(),
                    size / 2.,
                    Stroke::new(1., egui::Color32::from_black_alpha(90)),
                );
            }
        }
    });
}

fn state_line(f: &FolderStatus) -> (String, egui::Color32, bool) {
    let p = theme::palette(true);
    match &f.state {
        FolderState::Waiting => ("Waiting to scan".into(), p.muted, false),
        FolderState::Scanning { done, total } => (
            if *total == 0 {
                "Listing files…".into()
            } else {
                format!("Scanning {done} of {total}")
            },
            theme::ACCENT,
            true,
        ),
        FolderState::Ready => (
            format!(
                "{} files{}",
                f.files,
                if f.watching { " · live" } else { "" }
            ),
            egui::Color32::from_rgb(0x56, 0xb4, 0x5b),
            false,
        ),
        FolderState::Offline => (
            format!("Offline · {} models kept", f.files),
            theme::WARNING,
            false,
        ),
        FolderState::Error(e) => (e.clone(), theme::DANGER, false),
    }
}

/// Draws the folder list and its controls (also used by Preferences).
fn folders_list(ui: &mut egui::Ui, v: &LibraryView, acts: &mut Vec<Act>, filterable: bool) {
    let p = theme::of(ui);
    let snap = v.snapshot().clone();
    for f in &snap.folders {
        let selected = filterable && v.filter.folder.as_ref() == Some(&f.path);
        let (row, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.), Sense::click());
        let fill = if selected {
            p.accent_surface
        } else if r.hovered() {
            p.hover
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(row, 6., fill);
        let (line, dot, busy) = state_line(f);
        let c = Pos2::new(row.left() + 12., row.top() + 15.);
        if busy {
            let t = ui.input(|i| i.time) as f32;
            ui.painter()
                .circle_stroke(c, 4.5, Stroke::new(1.5, dot.gamma_multiply(0.4)));
            let a = t * 4.;
            ui.painter().line_segment(
                [c, c + Vec2::new(a.cos(), a.sin()) * 4.5],
                Stroke::new(1.5, dot),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        } else {
            ui.painter().circle_filled(c, 4., dot);
        }
        let text_left = row.left() + 24.;
        ui.painter().text(
            Pos2::new(text_left, row.top() + 15.),
            Align2::LEFT_CENTER,
            folder_name(&f.path),
            theme::medium(13.),
            p.text,
        );
        ui.painter().text(
            Pos2::new(text_left, row.top() + 33.),
            Align2::LEFT_CENTER,
            line,
            theme::sans(11.),
            p.muted,
        );
        if let FolderState::Scanning { done, total } = f.state
            && total > 0
        {
            let bar = Rect::from_min_size(
                Pos2::new(text_left, row.bottom() - 3.),
                Vec2::new(row.right() - text_left - 10., 2.),
            );
            ui.painter().rect_filled(bar, 1., p.border_raised);
            let mut done_bar = bar;
            done_bar.set_width(bar.width() * done as f32 / total as f32);
            ui.painter().rect_filled(done_bar, 1., p.accent);
        }
        let r = r.on_hover_text(format!(
            "{}\n{}",
            f.path.display(),
            if filterable {
                "Click to show only this folder; right-click for more"
            } else {
                "Right-click for more"
            }
        ));
        if filterable && r.clicked() {
            acts.push(Act::FolderFilter(if selected {
                None
            } else {
                Some(f.path.clone())
            }));
        }
        r.context_menu(|ui| {
            if ui.button("Scan again").clicked() {
                acts.push(Act::Rescan(Some(f.path.clone())));
                ui.close();
            }
            if ui.button("Show in Finder").clicked() {
                acts.push(Act::Reveal(f.path.clone()));
                ui.close();
            }
            ui.separator();
            if ui
                .button("Remove from library")
                .on_hover_text("Mica forgets this folder; nothing in it is changed")
                .clicked()
            {
                acts.push(Act::RemoveFolder(f.path.clone()));
                ui.close();
            }
        });
    }
}

/// The "add a folder" row: a path field and a picker.
fn add_folder_row(ui: &mut egui::Ui, v: &mut LibraryView, acts: &mut Vec<Act>, headless: bool) {
    let p = theme::of(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.;
        let w = ui.available_width() - 74.;
        let r = ui.add(
            egui::TextEdit::singleline(&mut v.folder_input)
                .hint_text(
                    egui::RichText::new("~/3D Models or /Volumes/…")
                        .color(p.muted)
                        .font(theme::sans(12.)),
                )
                .font(theme::sans(12.))
                .desired_width(w),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (widgets::button(ui, "Add", !v.folder_input.trim().is_empty()).clicked() || enter)
            && !v.folder_input.trim().is_empty()
        {
            acts.push(Act::AddFolder(PathBuf::from(v.folder_input.trim())));
        }
    });
    if widgets::link(ui, "Choose a folder…").clicked() && !headless {
        acts.push(Act::PickFolder);
    }
    if let Some(n) = &v.notice {
        ui.label(
            egui::RichText::new(n)
                .font(theme::sans(11.5))
                .color(theme::WARNING),
        );
    }
}

fn filters(ui: &mut egui::Ui, v: &mut LibraryView) {
    let p = theme::of(ui);
    ui.spacing_mut().item_spacing.y = 6.;
    let label = |ui: &mut egui::Ui, t: &str| {
        ui.add_space(4.);
        ui.label(
            egui::RichText::new(t)
                .font(theme::medium(12.))
                .color(p.label),
        );
    };
    label(ui, "Type");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
        for k in [Kind::Stl, Kind::ThreeMf, Kind::Obj, Kind::Step, Kind::Gcode] {
            let name = if k == Kind::Gcode {
                "G-code"
            } else {
                k.label()
            };
            if pill_toggle(ui, name, v.kind_on(k)) {
                v.toggle_kind(k);
            }
        }
    });
    if !v.printers.is_empty() {
        label(ui, "Printer");
        let current = v.filter.printer.clone().unwrap_or_default();
        let mut items = vec![(String::new(), "Any printer".to_string())];
        items.extend(v.printers.iter().map(|x| (x.clone(), x.clone())));
        let shown = if current.is_empty() {
            "Any printer".to_string()
        } else {
            current.clone()
        };
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::hover());
        if let Some(pick) = widgets::dropdown_with(
            ui,
            egui::Id::unique("lib-printer"),
            rect,
            &current,
            &shown,
            None,
            &items,
            true,
            !current.is_empty(),
        ) {
            v.filter.printer = (!pick.is_empty()).then_some(pick);
        }
    }
    label(ui, "Colors");
    let c = match v.filter.colours {
        Colours::Any => 0,
        Colours::Some => 1,
        Colours::Multi => 2,
    };
    if let Some(i) = pills(ui, &["Any", "With colors", "Multi-color"], c) {
        v.filter.colours = [Colours::Any, Colours::Some, Colours::Multi][i];
    }
    label(ui, "Sliced");
    let s = match v.filter.sliced {
        Sliced::Any => 0,
        Sliced::Yes => 1,
        Sliced::No => 2,
    };
    if let Some(i) = pills(ui, &["Any", "Ready to print", "Not sliced"], s) {
        v.filter.sliced = [Sliced::Any, Sliced::Yes, Sliced::No][i];
    }
    label(ui, "Size (largest side)");
    let si = SIZES
        .iter()
        .position(|(_, lo, hi)| *lo == v.filter.min_size_mm && *hi == v.filter.max_size_mm)
        .unwrap_or(0);
    let labels: Vec<&str> = SIZES.iter().map(|s| s.0).collect();
    if let Some(i) = pills(ui, &labels, si) {
        v.filter.min_size_mm = SIZES[i].1;
        v.filter.max_size_mm = SIZES[i].2;
    }
    label(ui, "Modified");
    let di = DATES
        .iter()
        .position(|(_, d)| *d == v.filter.within_days)
        .unwrap_or(0);
    let labels: Vec<&str> = DATES.iter().map(|s| s.0).collect();
    if let Some(i) = pills(ui, &labels, di) {
        v.filter.within_days = DATES[i].1;
    }
    label(ui, "Favorites and tags");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
        if pill_toggle(ui, "Favorites", v.filter.favourites) {
            v.filter.favourites = !v.filter.favourites;
        }
        for t in v.tags.clone() {
            let on = v
                .filter
                .tag
                .as_deref()
                .is_some_and(|x| x.eq_ignore_ascii_case(&t));
            if pill_toggle(ui, &format!("# {t}"), on) {
                v.filter.tag = (!on).then_some(t);
            }
        }
    });
    if v.tags.is_empty() {
        muted(ui, "Tags you add to models show up here.", 11.);
    }
}

pub(crate) fn thumb_placeholder(ui: &egui::Ui, rect: Rect, m: &Model) {
    let p = theme::of(ui);
    if m.thumbnail_pending() {
        pending_thumb(ui, rect);
        return;
    }
    ui.painter().text(
        rect.center() - Vec2::new(0., 6.),
        Align2::CENTER_CENTER,
        m.kind.label(),
        theme::semibold(16.),
        p.muted.gamma_multiply(0.8),
    );
    let why = if m.kind == Kind::Step {
        "no preview"
    } else if !m.info.readable {
        "could not read"
    } else if m
        .info
        .no_render
        .as_deref()
        .is_some_and(|w| w.contains("too "))
    {
        "too detailed to preview"
    } else {
        "no picture"
    };
    ui.painter().text(
        rect.center() + Vec2::new(0., 14.),
        Align2::CENTER_CENTER,
        why,
        theme::sans(11.),
        p.muted.gamma_multiply(0.7),
    );
}

/// A thumbnail still being rendered: a faint cube outline that slowly breathes.
fn pending_thumb(ui: &egui::Ui, rect: Rect) {
    let p = theme::of(ui);
    let t = ui.input(|i| i.time) as f32;
    let a = 0.35 + 0.2 * (t * 2.4).sin();
    let c = rect.center() - Vec2::new(0., 4.);
    let s = (rect.height() * 0.16).min(22.);
    // An isometric cube: the hexagon and the three inner edges.
    let hex: Vec<Pos2> = (0..6)
        .map(|i| {
            let ang = std::f32::consts::FRAC_PI_3 * i as f32 - std::f32::consts::FRAC_PI_2;
            c + Vec2::new(ang.cos(), ang.sin()) * s
        })
        .collect();
    let stroke = Stroke::new(1.3, p.muted.gamma_multiply(a));
    ui.painter()
        .add(egui::Shape::closed_line(hex.clone(), stroke));
    for k in [1, 3, 5] {
        ui.painter().line_segment([c, hex[k]], stroke);
    }
    ui.painter().text(
        c + Vec2::new(0., s + 14.),
        Align2::CENTER_CENTER,
        "Rendering preview",
        theme::sans(10.5),
        p.muted.gamma_multiply(0.75),
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(80));
}

fn paint_thumb(ui: &egui::Ui, rect: Rect, tex: &egui::TextureHandle) {
    let s = tex.size_vec2();
    let scale = (rect.width() / s.x).min(rect.height() / s.y);
    let r = Rect::from_center_size(rect.center(), s * scale);
    ui.painter().image(
        tex.id(),
        r,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
        egui::Color32::WHITE,
    );
}

fn star(ui: &egui::Ui, rect: Rect, on: bool, hover: bool) {
    let p = theme::of(ui);
    let c = rect.center();
    let pts: Vec<Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 5. - 0.5);
            let rad = if i % 2 == 0 {
                rect.width() / 2.
            } else {
                rect.width() / 4.6
            };
            c + Vec2::new(a.cos(), a.sin()) * rad
        })
        .collect();
    if on {
        // The star is concave: fill it as a fan of triangles around its centre.
        let mut mesh = egui::epaint::Mesh::default();
        mesh.colored_vertex(c, p.accent);
        for q in &pts {
            mesh.colored_vertex(*q, p.accent);
        }
        for i in 0..10u32 {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 10);
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter()
            .add(egui::Shape::closed_line(pts, Stroke::new(1.4, p.accent)));
    } else {
        ui.painter().add(egui::Shape::closed_line(
            pts,
            Stroke::new(1.4, if hover { p.text } else { p.muted }),
        ));
    }
}

/// A small pill over a thumbnail's top left corner.
fn badge(ui: &egui::Ui, at: Pos2, text: &str) {
    let p = theme::of(ui);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::medium(11.), p.accent_hi);
    let b = Rect::from_min_size(at, g.size() + Vec2::new(10., 4.));
    ui.painter().rect_filled(b, 8., p.accent_surface);
    ui.painter()
        .galley(b.min + Vec2::new(5., 2.), g, p.accent_hi);
}

fn short_folder(folder: &Path) -> String {
    format!("“{}”", folder_name(folder))
}

/// Menu entries that override the project detection for `folder`.
fn grouping_menu(ui: &mut egui::Ui, folder: &Path, current: Option<Grouping>, acts: &mut Vec<Act>) {
    ui.separator();
    if current != Some(Grouping::Project)
        && ui
            .button(format!("Treat {} as a project", short_folder(folder)))
            .on_hover_text("Show everything in this folder as one project card")
            .clicked()
    {
        acts.push(Act::SetGrouping(
            folder.to_path_buf(),
            Some(Grouping::Project),
        ));
        ui.close();
    }
    if current.is_some()
        && ui
            .button(format!("Group {} automatically", short_folder(folder)))
            .on_hover_text("Let Mica decide again whether this folder is a project")
            .clicked()
    {
        acts.push(Act::SetGrouping(folder.to_path_buf(), None));
        ui.close();
    }
}

/// The actions on a project (its card's menu, the bar above its files).
fn project_menu(
    ui: &mut egui::Ui,
    models: &[Model],
    pr: &mica_library::Project,
    acts: &mut Vec<Act>,
    with_open: bool,
) {
    if with_open && ui.button("Show files").clicked() {
        acts.push(Act::OpenFolder(Some(pr.folder.clone())));
        ui.close();
    }
    if let Some(f) = pr.main_3mf(models)
        && ui
            .add_enabled(pr.online, egui::Button::new("Open project"))
            .clicked()
    {
        acts.push(Act::OpenProject(pr.path(models, f).to_path_buf()));
        ui.close();
    }
    let parts = pr.plate_parts(models);
    if !parts.is_empty()
        && ui
            .add_enabled(pr.online, egui::Button::new("Add all parts to plate"))
            .clicked()
    {
        acts.push(Act::AddAllToPlate(
            parts
                .iter()
                .map(|f| pr.path(models, *f).to_path_buf())
                .collect(),
        ));
        ui.close();
    }
    if ui.button("Show in Finder").clicked() {
        acts.push(Act::Reveal(pr.folder.clone()));
        ui.close();
    }
    ui.separator();
    if ui
        .button("Split this project")
        .on_hover_text("Show this folder's files one by one")
        .clicked()
    {
        acts.push(Act::SetGrouping(pr.folder.clone(), Some(Grouping::Split)));
        ui.close();
    }
    if pr.chosen
        && ui
            .button("Group automatically")
            .on_hover_text("Let Mica decide again whether this folder is a project")
            .clicked()
    {
        acts.push(Act::SetGrouping(pr.folder.clone(), None));
        ui.close();
    }
}

/// Above an open project's files: the way back, its name and its actions.
fn project_bar(
    ui: &mut egui::Ui,
    models: &[Model],
    pr: &mica_library::Project,
    acts: &mut Vec<Act>,
) {
    let p = theme::of(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.;
        if widgets::link(ui, "Library")
            .on_hover_text("Back to the whole library")
            .clicked()
        {
            acts.push(Act::OpenFolder(None));
        }
        ui.label(
            egui::RichText::new("›")
                .font(theme::sans(14.))
                .color(p.muted),
        );
        ui.label(
            egui::RichText::new(&pr.name)
                .font(theme::semibold(15.))
                .color(p.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.;
            let more = widgets::icon_button(ui, widgets::Glyph::More, "More", true);
            egui::Popup::menu(&more).show(|ui| {
                if ui
                    .button("Split this project")
                    .on_hover_text("Show this folder's files one by one")
                    .clicked()
                {
                    acts.push(Act::SetGrouping(pr.folder.clone(), Some(Grouping::Split)));
                    ui.close();
                }
                if pr.chosen
                    && ui
                        .button("Group automatically")
                        .on_hover_text("Let Mica decide again whether this folder is a project")
                        .clicked()
                {
                    acts.push(Act::SetGrouping(pr.folder.clone(), None));
                    ui.close();
                }
            });
            if widgets::button(ui, "Show in Finder", true).clicked() {
                acts.push(Act::Reveal(pr.folder.clone()));
            }
            let parts = pr.plate_parts(models);
            if !parts.is_empty()
                && widgets::button(ui, "Add all parts to plate", pr.online)
                    .on_hover_text(format!(
                        "Add the {} STL part{} to the current plate",
                        parts.len(),
                        if parts.len() == 1 { "" } else { "s" }
                    ))
                    .clicked()
            {
                acts.push(Act::AddAllToPlate(
                    parts
                        .iter()
                        .map(|f| pr.path(models, *f).to_path_buf())
                        .collect(),
                ));
            }
            if let Some(f) = pr.main_3mf(models)
                && widgets::primary_button(ui, "Open project", pr.online)
                    .on_hover_text(format!(
                        "Open {} (replaces the current plates)",
                        pr.path(models, f)
                            .file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                    ))
                    .clicked()
            {
                acts.push(Act::OpenProject(pr.path(models, f).to_path_buf()));
            }
        });
    });
    let mut facts = vec![library::project_summary(pr)];
    if !pr.printers.is_empty() {
        facts.push(format!(
            "sliced for {}",
            pr.printers
                .iter()
                .map(|x| library::short_printer(x))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if pr.chosen {
        facts.push("grouped by you".into());
    }
    facts.push(pr.folder.strip_prefix(&pr.root).map_or_else(
        |_| pr.folder.display().to_string(),
        |r| format!("{} › {}", folder_name(&pr.root), r.display()),
    ));
    muted(ui, facts.join(" · "), 11.5);
}

impl App {
    pub(crate) fn library_view(&mut self, ui: &mut egui::Ui) {
        self.library.refresh();
        let p = theme::of(ui);
        let mut acts: Vec<Act> = vec![];
        self.library.sync_preview(ui.ctx());
        if self.library.snapshot().scanning()
            || self.library.loading()
            || self.library.snapshot().pending_thumbnails > 0
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let left =
                    Rect::from_min_max(rect.min, Pos2::new(rect.left() + LEFT_W, rect.bottom()));
                ui.painter().rect_filled(left, 0., p.panel);
                ui.painter().line_segment(
                    [left.right_top(), left.right_bottom()],
                    Stroke::new(1., p.border),
                );
                let show_detail = self.library.error.is_none()
                    && !self.library.loading()
                    && self.library.selected_model().is_some();
                let right =
                    Rect::from_min_max(Pos2::new(rect.right() - DETAIL_W, rect.top()), rect.max);
                let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink(14.)));
                self.library_sidebar(&mut left_ui, &mut acts);
                let main = Rect::from_min_max(
                    Pos2::new(left.right() + 1., rect.top()),
                    Pos2::new(
                        if show_detail {
                            right.left() - 1.
                        } else {
                            rect.right()
                        },
                        rect.bottom(),
                    ),
                );
                let mut main_ui = ui
                    .new_child(egui::UiBuilder::new().max_rect(main.shrink2(Vec2::new(18., 14.))));
                self.library_main(&mut main_ui, &mut acts);
                if show_detail {
                    ui.painter().rect_filled(right, 0., p.panel);
                    ui.painter().line_segment(
                        [right.left_top(), right.left_bottom()],
                        Stroke::new(1., p.border),
                    );
                    let mut detail_ui =
                        ui.new_child(egui::UiBuilder::new().max_rect(right.shrink(16.)));
                    self.library_detail(&mut detail_ui, &mut acts);
                }
            });
        self.library_prompt(ui.ctx(), &mut acts);
        let ctx = ui.ctx().clone();
        for a in acts {
            self.library_act(a, &ctx);
        }
    }

    fn library_sidebar(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = theme::of(ui);
        ui.spacing_mut().item_spacing.y = 6.;
        let headless = self.headless;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let v = &mut self.library;
                widgets::card_header(ui, "Library folders", |ui| {
                    if widgets::icon_button(
                        ui,
                        widgets::Glyph::Sync,
                        "Scan every folder again",
                        v.lib.is_some() && !v.snapshot().folders.is_empty(),
                    )
                    .clicked()
                    {
                        acts.push(Act::Rescan(None));
                    }
                });
                if let Some(e) = &v.error {
                    ui.label(
                        egui::RichText::new(e)
                            .font(theme::sans(12.))
                            .color(theme::DANGER),
                    );
                    return;
                }
                if v.loading() {
                    muted(ui, "Opening library…", 12.);
                    return;
                }
                if v.snapshot().folders.is_empty() {
                    muted(
                        ui,
                        "Add the folders where you keep models, projects and G-code, for \
                         example ~/3D Models or a NAS share such as /Volumes/Models. Mica only \
                         reads them: nothing in them is ever changed.",
                        12.,
                    );
                } else {
                    folders_list(ui, v, acts, true);
                }
                ui.add_space(2.);
                add_folder_row(ui, v, acts, headless);
                ui.add_space(8.);
                ui.separator();
                widgets::card_header(ui, "Filters", |ui| {
                    if v.filter.narrowed() && widgets::link(ui, "Clear").clicked() {
                        let text = std::mem::take(&mut v.filter.text);
                        v.filter = Default::default();
                        v.filter.text = text;
                    }
                });
                filters(ui, v);
                let _ = p;
            });
    }

    fn library_main(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = theme::of(ui);
        let v = &mut self.library;
        if let Some(e) = &v.error {
            ui.add_space(60.);
            library_error(ui, e, acts);
            return;
        }
        if v.loading() {
            ui.spinner();
            muted(ui, "Opening library…", 14.);
            return;
        }
        if let Some(warning) = v.lib.as_ref().and_then(|lib| lib.warning()) {
            ui.label(
                egui::RichText::new(warning)
                    .font(theme::sans(12.))
                    .color(theme::WARNING),
            );
        }
        if v.lib.as_ref().is_some_and(|lib| lib.read_only()) {
            muted(ui, mica_library::READ_ONLY_NOTE, 12.);
        }
        let snap = v.snapshot().clone();
        // Toolbar: search, count, grouping, sort, grid / list.
        let n = v.visible_len();
        let (shown_projects, shown_models) = v.shown_counts();
        let models = |n: usize| {
            if n == 1 {
                "1 model".to_string()
            } else {
                format!("{n} models")
            }
        };
        let annotation = if let Some(pr) = v.current_project() {
            format!("{} files", pr.files.len() + pr.extras.len())
        } else if shown_projects > 0 {
            format!(
                "{shown_projects} project{} · {}",
                if shown_projects == 1 { "" } else { "s" },
                models(shown_models)
            )
        } else if n == snap.models.len() {
            models(n)
        } else {
            format!("{n} of {}", snap.models.len())
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            let w = (ui.available_width() - 470.).max(200.);
            ui.allocate_ui_with_layout(Vec2::new(w, 34.), Layout::top_down(Align::Min), |ui| {
                widgets::search_field(
                    ui,
                    egui::Id::unique("lib-search"),
                    &mut v.filter.text,
                    "Search projects, names, folders, tags…",
                    &annotation,
                );
            });
            let items: Vec<(String, String)> = GroupBy::ALL
                .iter()
                .map(|g| (format!("{g:?}"), g.label().to_string()))
                .collect();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(140., 30.), Sense::hover());
            if let Some(pick) = widgets::dropdown_with(
                ui,
                egui::Id::unique("lib-group"),
                rect,
                &format!("{:?}", v.group_by),
                &format!("Group: {}", v.group_by.label()),
                None,
                &items,
                true,
                false,
            ) && let Some(g) = GroupBy::ALL.iter().find(|g| format!("{g:?}") == pick)
            {
                v.group_by = *g;
                v.open_project = None;
            }
            let items: Vec<(String, String)> = Sort::ALL
                .iter()
                .map(|s| (format!("{s:?}"), s.label().to_string()))
                .collect();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(160., 30.), Sense::hover());
            if let Some(pick) = widgets::dropdown_with(
                ui,
                egui::Id::unique("lib-sort"),
                rect,
                &format!("{:?}", v.sort),
                &format!("Sort: {}", v.sort.label()),
                None,
                &items,
                true,
                false,
            ) && let Some(s) = Sort::ALL.iter().find(|s| format!("{s:?}") == pick)
            {
                v.sort = *s;
                v.descending = matches!(
                    s,
                    Sort::Modified | Sort::FileSize | Sort::PrintTime | Sort::Dimensions
                );
            }
            if widgets::icon_button(
                ui,
                if v.descending {
                    widgets::Glyph::Down
                } else {
                    widgets::Glyph::Up
                },
                if v.descending {
                    "Largest / newest first"
                } else {
                    "Smallest / oldest / A first"
                },
                true,
            )
            .clicked()
            {
                v.descending = !v.descending;
            }
            if let Some(i) = pills(ui, &["Grid", "List"], usize::from(v.list)) {
                v.list = i == 1;
            }
        });
        // Scan progress over every folder.
        let (done, total) = snap.progress();
        if snap.scanning() {
            ui.add_space(2.);
            let (r, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), Sense::hover());
            let bar = Rect::from_min_size(
                Pos2::new(r.left(), r.center().y + 5.),
                Vec2::new(r.width(), 3.),
            );
            ui.painter().rect_filled(bar, 1.5, p.border_raised);
            let mut d = bar;
            d.set_width(if total > 0 {
                bar.width() * done as f32 / total as f32
            } else {
                0.
            });
            ui.painter().rect_filled(d, 1.5, p.accent);
            ui.painter().text(
                Pos2::new(r.left(), r.center().y - 4.),
                Align2::LEFT_CENTER,
                if total > 0 {
                    format!(
                        "Scanning in the background: {done} of {total} files. You can keep working."
                    )
                } else {
                    "Listing files…".to_string()
                },
                theme::sans(11.5),
                p.muted,
            );
        } else {
            ui.add_space(6.);
        }
        if snap.models.is_empty() {
            ui.add_space(60.);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(if snap.folders.is_empty() {
                        "Your model library is empty"
                    } else if snap.scanning() {
                        "Looking for models…"
                    } else {
                        "No models found in these folders"
                    })
                    .font(theme::semibold(18.))
                    .color(p.text),
                );
                ui.add_space(6.);
                muted(
                    ui,
                    "Mica finds STL, 3MF, OBJ, STEP and G-code files in your library folders, \
                     reads their size, profiles, colors and pictures, and keeps the list up to \
                     date as files change.",
                    12.5,
                );
            });
            return;
        }
        if let Some(pr) = v.current_project().cloned() {
            project_bar(ui, &snap.models, &pr, acts);
            ui.add_space(4.);
        }
        if v.entries().is_empty() {
            ui.add_space(40.);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Nothing matches")
                        .font(theme::semibold(16.))
                        .color(p.text),
                );
                muted(ui, "Try fewer words or clear the filters.", 12.);
            });
            return;
        }
        if v.list {
            self.library_list(ui, acts);
        } else {
            self.library_grid(ui, acts);
        }
    }

    fn library_grid(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let cols = (((ui.available_width() + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
        // Cards share the row's width (no ragged gap on the right).
        let card_w = ((ui.available_width() - GAP * (cols - 1) as f32) / cols as f32)
            .clamp(CARD_W, CARD_W * 1.4);
        // Rows: section headers and runs of cards.
        let entries = self.library.entries().to_vec();
        let mut rows: Vec<(f32, std::ops::Range<usize>, bool)> = vec![];
        let mut y = 0.;
        let mut i = 0;
        while i < entries.len() {
            if matches!(entries[i], Entry::Header { .. }) {
                rows.push((y, i..i + 1, true));
                y += HEADER_H;
                i += 1;
                continue;
            }
            let start = i;
            while i < entries.len()
                && i - start < cols
                && !matches!(entries[i], Entry::Header { .. })
            {
                i += 1;
            }
            rows.push((y, start..i, false));
            y += CARD_H + GAP;
        }
        let total = y;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt(("lib-grid", self.library.open_project.clone()))
            .show_viewport(ui, |ui, viewport| {
                let (area, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), total), Sense::hover());
                let first = rows.partition_point(|r| {
                    r.0 + if r.2 { HEADER_H } else { CARD_H + GAP } < viewport.top()
                });
                for (top, range, header) in rows[first..].iter().cloned() {
                    if top > viewport.bottom() {
                        break;
                    }
                    let origin = area.min + Vec2::new(0., top);
                    if header {
                        let r = Rect::from_min_size(origin, Vec2::new(area.width(), HEADER_H));
                        self.lib_section_header(ui, r, &entries[range.start], acts);
                        continue;
                    }
                    for (c, e) in range.clone().enumerate() {
                        let rect = Rect::from_min_size(
                            origin + Vec2::new(c as f32 * (card_w + GAP), 0.),
                            Vec2::new(card_w, CARD_H),
                        );
                        match entries[e] {
                            Entry::Project(pi) => self.lib_project_card(ui, rect, e, pi, acts),
                            Entry::Model(mi) => self.lib_model_card(ui, rect, e, mi, None, acts),
                            Entry::File(f) => {
                                self.lib_model_card(ui, rect, e, f.model, Some(f.location), acts)
                            }
                            Entry::Extra(x) => self.lib_extra_card(ui, rect, e, x, acts),
                            Entry::Header { .. } => {}
                        }
                    }
                }
            });
    }

    /// A section header: title, count and (for folders) a menu.
    fn lib_section_header(&mut self, ui: &mut egui::Ui, r: Rect, e: &Entry, acts: &mut Vec<Act>) {
        let p = theme::of(ui);
        let Entry::Header {
            title,
            count,
            folder,
        } = e
        else {
            return;
        };
        let base = Pos2::new(r.left() + 2., r.bottom() - 12.);
        let g = ui
            .painter()
            .layout_no_wrap(title.clone(), theme::semibold(13.), p.text);
        let tw = g.size().x;
        ui.painter()
            .galley(base - Vec2::new(0., g.size().y / 2.), g, p.text);
        ui.painter().text(
            base + Vec2::new(tw + 8., 0.),
            Align2::LEFT_CENTER,
            count.to_string(),
            theme::sans(12.),
            p.muted,
        );
        ui.painter().line_segment(
            [
                Pos2::new(r.left(), r.bottom() - 2.),
                Pos2::new(r.right() - GAP, r.bottom() - 2.),
            ],
            Stroke::new(1., p.border_soft),
        );
        let Some(folder) = folder else { return };
        let resp = ui
            .interact(r, egui::Id::unique(("lib-head", folder)), Sense::click())
            .on_hover_text(format!("{}\nRight-click for more", folder.display()));
        let current = self.library.grouping_of(folder);
        resp.context_menu(|ui| {
            if ui.button("Show in Finder").clicked() {
                acts.push(Act::Reveal(folder.clone()));
                ui.close();
            }
            grouping_menu(ui, folder, current, acts);
        });
    }

    /// One model (or, inside a project, one of its files).
    fn lib_model_card(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        entry: usize,
        mi: usize,
        location: Option<usize>,
        acts: &mut Vec<Act>,
    ) {
        let p = theme::of(ui);
        let ctx = ui.ctx().clone();
        let snap = self.library.snapshot().clone();
        let Some(m) = snap.models.get(mi) else { return };
        let loc = &m.locations[location.unwrap_or(0).min(m.locations.len() - 1)];
        let name = location.map_or_else(
            || m.name.clone(),
            |_| {
                loc.path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            },
        );
        let resp = ui.interact(
            rect,
            egui::Id::unique(("lib-card", entry)),
            Sense::click_and_drag(),
        );
        // Drag onto the plate (a file inside a project: from that copy).
        self.library_drag_source(&resp, m, location);
        let selected = self.library.selected.as_deref() == Some(m.id.as_str())
            || self.library.picked.contains(&m.id);
        let marked = location.is_some() && self.library.searching() && self.library.matched(mi);
        let fill = if resp.hovered() {
            p.hover
        } else if marked {
            p.accent_surface
        } else {
            p.panel
        };
        ui.painter().rect_filled(rect, 10., fill);
        ui.painter().rect_stroke(
            rect,
            10.,
            Stroke::new(
                if selected { 2. } else { 1. },
                if selected || marked {
                    p.accent
                } else {
                    p.border_soft
                },
            ),
            egui::StrokeKind::Inside,
        );
        let card_w = rect.width();
        let thumb = Rect::from_min_size(
            rect.min + Vec2::new(8., 8.),
            Vec2::new(card_w - 16., THUMB_H),
        );
        ui.painter().rect_filled(thumb, 7., p.search_bg);
        match self.library.texture(&ctx, m) {
            Some(t) => paint_thumb(ui, thumb.shrink(6.), &t),
            None => thumb_placeholder(ui, thumb, m),
        }
        if !m.online() {
            ui.painter()
                .rect_filled(thumb, 7., p.bg.gamma_multiply(0.55));
            ui.painter().text(
                thumb.center_bottom() - Vec2::new(0., 12.),
                Align2::CENTER_CENTER,
                "Offline",
                theme::medium(11.),
                theme::WARNING,
            );
        }
        // Copies: a badge (inside a project: how many other folders hold one).
        let dir = loc.path.parent();
        let others = mica_library::Projects::other_folders(&snap.models, mi, dir);
        let copies = if location.is_some() {
            (others > 0).then(|| {
                format!(
                    "Also in {others} other folder{}",
                    if others == 1 { "" } else { "s" }
                )
            })
        } else {
            (m.locations.len() > 1).then(|| format!("×{}", m.locations.len()))
        };
        if let Some(t) = copies {
            badge(ui, thumb.min + Vec2::new(6., 6.), &t);
        }
        // Favourite star.
        let sr = Rect::from_center_size(
            Pos2::new(thumb.right() - 14., thumb.top() + 14.),
            Vec2::splat(16.),
        );
        let star_r = ui.interact(
            sr.expand(4.),
            egui::Id::unique(("lib-star", entry)),
            Sense::click(),
        );
        if m.user.favourite || resp.hovered() || star_r.hovered() {
            star(ui, sr, m.user.favourite, star_r.hovered());
        }
        if star_r.clicked() {
            acts.push(Act::Favourite(m.id.clone()));
        }
        let text_left = rect.left() + 10.;
        let name_rect = Rect::from_min_size(
            Pos2::new(text_left, thumb.bottom() + 8.),
            Vec2::new(card_w - 20., 18.),
        );
        let mut name_ui = ui.new_child(egui::UiBuilder::new().max_rect(name_rect));
        name_ui.add(
            egui::Label::new(
                egui::RichText::new(&name)
                    .font(theme::medium(13.))
                    .color(p.text),
            )
            .truncate()
            .selectable(false),
        );
        ui.painter().text(
            Pos2::new(text_left, thumb.bottom() + 38.),
            Align2::LEFT_CENTER,
            library::summary(m),
            theme::sans(11.5),
            p.muted,
        );
        // Filament colours: dots in the thumbnail's lower left corner.
        let dots: Vec<egui::Color32> = m
            .info
            .colours
            .iter()
            .take(8)
            .filter_map(|c| library::parse_colour(c))
            .collect();
        if !dots.is_empty() {
            let bg = Rect::from_min_size(
                Pos2::new(thumb.left() + 6., thumb.bottom() - 20.),
                Vec2::new(8. + 12. * dots.len() as f32, 14.),
            );
            ui.painter()
                .rect_filled(bg, 7., p.dark_box.gamma_multiply(0.85));
            for (k, c) in dots.iter().enumerate() {
                ui.painter().circle_filled(
                    Pos2::new(bg.left() + 10. + 12. * k as f32, bg.center().y),
                    4.,
                    *c,
                );
            }
        }
        let mut tip = loc.path.display().to_string();
        if others > 0 && location.is_none() {
            tip += &format!(
                "\nAlso in {others} other folder{}",
                if others == 1 { "" } else { "s" }
            );
        }
        let resp = resp.on_hover_text(tip);
        let pick = ui.input(|i| i.modifiers.command || i.modifiers.shift);
        if resp.double_clicked() {
            acts.push(Act::Open(m.id.clone()));
        } else if resp.clicked() && !star_r.clicked() {
            acts.push(if pick {
                Act::Pick(m.id.clone())
            } else {
                Act::Select(Some(m.id.clone()))
            });
        }
        // Loose models: their folder can be made a project.
        let folder = (location.is_none() && dir.is_some_and(|d| d != loc.root))
            .then(|| dir.map(|d| group::grouping_folder(d, &loc.root).to_path_buf()))
            .flatten();
        let current = folder.as_deref().and_then(|f| self.library.grouping_of(f));
        resp.context_menu(|ui| {
            if ui.button("Open").clicked() {
                acts.push(Act::Open(m.id.clone()));
                ui.close();
            }
            if ui.button("Show in Finder").clicked() {
                acts.push(Act::Reveal(loc.path.clone()));
                ui.close();
            }
            if let Some(f) = &folder {
                grouping_menu(ui, f, current, acts);
            }
        });
    }

    /// One project: its cover, name, what it holds and the printers it was sliced for.
    fn lib_project_card(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        entry: usize,
        pi: usize,
        acts: &mut Vec<Act>,
    ) {
        let p = theme::of(ui);
        let ctx = ui.ctx().clone();
        let Some(pr) = self.library.projects().list.get(pi).cloned() else {
            return;
        };
        let snap = self.library.snapshot().clone();
        let resp = ui.interact(
            rect,
            egui::Id::unique(("lib-card", entry)),
            Sense::click_and_drag(),
        );
        // Drag onto the plate: all its parts (or its 3MF's first plate).
        self.library_drag_project(&resp, &pr);
        // A second sheet behind the card: this card holds several files.
        let back = rect
            .translate(Vec2::new(4., -4.))
            .shrink2(Vec2::new(6., 0.));
        ui.painter().rect_filled(back, 10., p.raised);
        ui.painter().rect_stroke(
            back,
            10.,
            Stroke::new(1., p.border_soft),
            egui::StrokeKind::Inside,
        );
        ui.painter()
            .rect_filled(rect, 10., if resp.hovered() { p.hover } else { p.panel });
        ui.painter().rect_stroke(
            rect,
            10.,
            Stroke::new(1., p.border_raised),
            egui::StrokeKind::Inside,
        );
        let thumb = Rect::from_min_size(
            rect.min + Vec2::new(8., 8.),
            Vec2::new(rect.width() - 16., THUMB_H),
        );
        ui.painter().rect_filled(thumb, 7., p.search_bg);
        match self.library.cover_texture(&ctx, &pr) {
            Some(t) => paint_thumb(ui, thumb.shrink(6.), &t),
            None => {
                ui.painter().text(
                    thumb.center(),
                    Align2::CENTER_CENTER,
                    "Project",
                    theme::semibold(16.),
                    p.muted.gamma_multiply(0.8),
                );
            }
        }
        if !pr.online {
            ui.painter()
                .rect_filled(thumb, 7., p.bg.gamma_multiply(0.55));
        }
        let files = pr.files.len() + pr.extras.len();
        badge(ui, thumb.min + Vec2::new(6., 6.), &format!("{files} files"));
        // Printers it was sliced or set up for.
        let mut x = thumb.left() + 6.;
        for printer in pr.printers.iter().take(3) {
            let t = library::short_printer(printer);
            let g = ui.painter().layout_no_wrap(t, theme::medium(10.5), p.text);
            let b = Rect::from_min_size(
                Pos2::new(x, thumb.bottom() - 22.),
                g.size() + Vec2::new(10., 4.),
            );
            if b.right() > thumb.right() - 6. {
                break;
            }
            ui.painter()
                .rect_filled(b, 8., p.dark_box.gamma_multiply(0.85));
            ui.painter().galley(b.min + Vec2::new(5., 2.), g, p.text);
            x = b.right() + 4.;
        }
        let text_left = rect.left() + 10.;
        let name_rect = Rect::from_min_size(
            Pos2::new(text_left, thumb.bottom() + 8.),
            Vec2::new(rect.width() - 20., 18.),
        );
        let mut name_ui = ui.new_child(egui::UiBuilder::new().max_rect(name_rect));
        name_ui.add(
            egui::Label::new(
                egui::RichText::new(&pr.name)
                    .font(theme::semibold(13.))
                    .color(p.text),
            )
            .truncate()
            .selectable(false),
        );
        // While searching: how many of its files match.
        let hits = pr
            .files
            .iter()
            .filter(|f| self.library.matched(f.model))
            .count();
        let (line, colour) = if self.library.searching() && hits < pr.files.len() {
            (
                format!("{hits} matching file{}", if hits == 1 { "" } else { "s" }),
                p.accent_hi,
            )
        } else {
            (library::project_summary(&pr), p.muted)
        };
        ui.painter().text(
            Pos2::new(text_left, thumb.bottom() + 38.),
            Align2::LEFT_CENTER,
            line,
            theme::sans(11.5),
            colour,
        );
        let mut tip = pr.folder.display().to_string();
        if !pr.printers.is_empty() {
            tip += &format!("\nSliced for {}", pr.printers.join(", "));
        }
        tip += "\nClick to see its files; right-click for more";
        let resp = resp.on_hover_text(tip);
        if resp.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            acts.push(Act::OpenFolder(Some(pr.folder.clone())));
        }
        resp.context_menu(|ui| project_menu(ui, &snap.models, &pr, acts, true));
    }

    /// A picture or document of the open project.
    fn lib_extra_card(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        entry: usize,
        xi: usize,
        acts: &mut Vec<Act>,
    ) {
        let p = theme::of(ui);
        let ctx = ui.ctx().clone();
        let snap = self.library.snapshot().clone();
        let Some(x) = snap.extras.get(xi) else { return };
        let resp = ui.interact(rect, egui::Id::unique(("lib-card", entry)), Sense::click());
        ui.painter()
            .rect_filled(rect, 10., if resp.hovered() { p.hover } else { p.panel });
        ui.painter().rect_stroke(
            rect,
            10.,
            Stroke::new(1., p.border_soft),
            egui::StrokeKind::Inside,
        );
        let thumb = Rect::from_min_size(
            rect.min + Vec2::new(8., 8.),
            Vec2::new(rect.width() - 16., THUMB_H),
        );
        ui.painter().rect_filled(thumb, 7., p.search_bg);
        match self.library.picture_texture(&ctx, x) {
            Some(t) => paint_thumb(ui, thumb.shrink(6.), &t),
            None => {
                let ext = x
                    .path
                    .extension()
                    .map_or_else(|| "TXT".into(), |e| e.to_string_lossy().to_uppercase());
                ui.painter().text(
                    thumb.center(),
                    Align2::CENTER_CENTER,
                    ext,
                    theme::semibold(16.),
                    p.muted.gamma_multiply(0.8),
                );
            }
        }
        let text_left = rect.left() + 10.;
        let name_rect = Rect::from_min_size(
            Pos2::new(text_left, thumb.bottom() + 8.),
            Vec2::new(rect.width() - 20., 18.),
        );
        let mut name_ui = ui.new_child(egui::UiBuilder::new().max_rect(name_rect));
        name_ui.add(
            egui::Label::new(
                egui::RichText::new(x.name())
                    .font(theme::medium(13.))
                    .color(p.text),
            )
            .truncate()
            .selectable(false),
        );
        ui.painter().text(
            Pos2::new(text_left, thumb.bottom() + 38.),
            Align2::LEFT_CENTER,
            format!("{} · {}", x.kind.label(), library::bytes(x.size)),
            theme::sans(11.5),
            p.muted,
        );
        let resp = resp.on_hover_text(format!(
            "{}\nDouble-click to show in Finder",
            x.path.display()
        ));
        if resp.double_clicked() {
            acts.push(Act::Reveal(x.path.clone()));
        }
        resp.context_menu(|ui| {
            if ui.button("Show in Finder").clicked() {
                acts.push(Act::Reveal(x.path.clone()));
                ui.close();
            }
        });
    }

    fn library_list(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = theme::of(ui);
        let ctx = ui.ctx().clone();
        let w = ui.available_width();
        // Name | Type | Size | Printer | Print time | Modified | Folder
        let cols = [0.30, 0.09, 0.15, 0.17, 0.09, 0.09, 0.11];
        let heads = [
            ("Name", Some(Sort::Name)),
            ("Type", Some(Sort::Kind)),
            ("Size", Some(Sort::Dimensions)),
            ("Printer", None),
            ("Print time", Some(Sort::PrintTime)),
            ("Modified", Some(Sort::Modified)),
            ("Folder", None),
        ];
        let x = |i: usize| cols[..i].iter().sum::<f32>() * w;
        let (head, _) = ui.allocate_exact_size(Vec2::new(w, 24.), Sense::hover());
        for (i, (h, sort)) in heads.iter().enumerate() {
            let r = Rect::from_min_size(
                Pos2::new(head.left() + x(i), head.top()),
                Vec2::new(cols[i] * w, 24.),
            );
            let resp = ui.interact(
                r,
                egui::Id::unique(("lib-head", i)),
                if sort.is_some() {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            let active = *sort == Some(self.library.sort);
            let label = if active {
                format!(
                    "{h} {}",
                    if self.library.descending {
                        "↓"
                    } else {
                        "↑"
                    }
                )
            } else {
                h.to_string()
            };
            ui.painter().text(
                Pos2::new(r.left() + 8., r.center().y),
                Align2::LEFT_CENTER,
                widgets::spaced_upper(&label),
                theme::semibold(10.5),
                if active { p.accent_hi } else { p.muted },
            );
            if let Some(s) = sort
                && resp.clicked()
            {
                if active {
                    self.library.descending = !self.library.descending;
                } else {
                    self.library.sort = *s;
                    self.library.descending = false;
                }
            }
        }
        ui.painter().line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1., p.border),
        );
        let entries = self.library.entries().to_vec();
        let snap = self.library.snapshot().clone();
        let now = library::now();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt(("lib-list", self.library.open_project.clone()))
            .show_rows(ui, ROW_H, entries.len(), |ui, range| {
                for i in range {
                    let e = &entries[i];
                    let (row, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
                    if let Entry::Header { .. } = e {
                        self.lib_section_header(ui, row, e, acts);
                        continue;
                    }
                    let resp =
                        ui.interact(row, egui::Id::unique(("lib-row", i)), Sense::click()) | resp;
                    let model = match e {
                        Entry::Model(m) => Some((*m, None)),
                        Entry::File(f) => Some((f.model, Some(f.location))),
                        _ => None,
                    };
                    let selected = model.is_some_and(|(m, _)| {
                        self.library.selected.as_deref() == Some(snap.models[m].id.as_str())
                    });
                    let marked = matches!(e, Entry::File(f) if self.library.searching()
                        && self.library.matched(f.model));
                    if selected {
                        ui.painter().rect_filled(row, 5., p.selected_row);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(row, 5., p.hover);
                    } else if marked {
                        ui.painter().rect_filled(row, 5., p.accent_surface);
                    } else if i % 2 == 1 {
                        ui.painter()
                            .rect_filled(row, 0., p.panel.gamma_multiply(0.5));
                    }
                    let icon =
                        Rect::from_min_size(row.min + Vec2::new(6., 3.), Vec2::splat(ROW_H - 6.));
                    let (cells, fav, online, tip): ([String; 7], bool, bool, String) = match e {
                        Entry::Project(pi) => {
                            let pr = self.library.projects().list[*pi].clone();
                            if let Some(t) = self.library.cover_texture(&ctx, &pr) {
                                paint_thumb(ui, icon, &t);
                            }
                            let folder = pr
                                .folder
                                .parent()
                                .and_then(|d| d.strip_prefix(&pr.root).ok())
                                .map(|d| d.display().to_string())
                                .unwrap_or_default();
                            (
                                [
                                    format!("{}  ·  {}", pr.name, library::project_summary(&pr)),
                                    "Project".into(),
                                    pr.largest.map(|l| format!("{l:.0} mm")).unwrap_or_default(),
                                    pr.printers
                                        .iter()
                                        .map(|x| library::short_printer(x))
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    pr.print_time_s.map(library::duration).unwrap_or_default(),
                                    library::when(pr.modified, now),
                                    folder,
                                ],
                                false,
                                pr.online,
                                pr.folder.display().to_string(),
                            )
                        }
                        Entry::Extra(xi) => {
                            let xx = snap.extras[*xi].clone();
                            if let Some(t) = self.library.picture_texture(&ctx, &xx) {
                                paint_thumb(ui, icon, &t);
                            }
                            (
                                [
                                    xx.name(),
                                    xx.kind.label().into(),
                                    library::bytes(xx.size),
                                    String::new(),
                                    String::new(),
                                    library::when(xx.modified, now),
                                    String::new(),
                                ],
                                false,
                                xx.online,
                                xx.path.display().to_string(),
                            )
                        }
                        _ => {
                            let (mi, location) = model.unwrap_or((0, None));
                            let m = snap.models[mi].clone();
                            if let Some(t) = self.library.texture(&ctx, &m) {
                                paint_thumb(ui, icon, &t);
                            }
                            let loc =
                                &m.locations[location.unwrap_or(0).min(m.locations.len() - 1)];
                            let name = loc
                                .path
                                .file_name()
                                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                            let dup = if m.locations.len() > 1 {
                                format!("  ×{}", m.locations.len())
                            } else {
                                String::new()
                            };
                            (
                                [
                                    format!("{name}{dup}"),
                                    m.kind.label().to_string(),
                                    m.info.size_mm.map(library::dims).unwrap_or_default(),
                                    m.info.printer.clone().unwrap_or_default(),
                                    m.info
                                        .print_time_s
                                        .map(library::duration)
                                        .unwrap_or_default(),
                                    library::when(m.modified(), now),
                                    loc.folder(),
                                ],
                                m.user.favourite,
                                m.online(),
                                loc.path.display().to_string(),
                            )
                        }
                    };
                    for (c, text) in cells.iter().enumerate() {
                        let mut left = row.left() + x(c) + if c == 0 { ROW_H + 6. } else { 8. };
                        if c == 0 && fav {
                            star(
                                ui,
                                Rect::from_center_size(
                                    Pos2::new(left + 6., row.center().y),
                                    Vec2::splat(12.),
                                ),
                                true,
                                false,
                            );
                            left += 18.;
                        }
                        let cell = Rect::from_min_max(
                            Pos2::new(left, row.top()),
                            Pos2::new(row.left() + x(c) + cols[c] * w - 6., row.bottom()),
                        );
                        let color = if c == 0 {
                            if online { p.text } else { p.muted }
                        } else {
                            p.label
                        };
                        let galley = ui.painter().layout(
                            text.clone(),
                            if c == 0 {
                                theme::medium(12.5)
                            } else {
                                theme::sans(12.)
                            },
                            color,
                            f32::INFINITY,
                        );
                        let clipped = ui.painter().with_clip_rect(cell);
                        clipped.galley(
                            Pos2::new(cell.left(), cell.center().y - galley.size().y / 2.),
                            galley,
                            color,
                        );
                    }
                    let resp = resp.on_hover_text(tip);
                    match e {
                        Entry::Project(pi) => {
                            if resp.clicked() {
                                let f = self.library.projects().list[*pi].folder.clone();
                                acts.push(Act::OpenFolder(Some(f)));
                            }
                        }
                        Entry::Extra(xi) => {
                            if resp.double_clicked() {
                                acts.push(Act::Reveal(snap.extras[*xi].path.clone()));
                            }
                        }
                        _ => {
                            if let Some((mi, _)) = model {
                                let id = snap.models[mi].id.clone();
                                if resp.double_clicked() {
                                    acts.push(Act::Open(id));
                                } else if resp.clicked() {
                                    acts.push(Act::Select(Some(id)));
                                }
                            }
                        }
                    }
                }
            });
    }

    fn library_detail(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let p = theme::of(ui);
        let Some(m) = self.library.selected_model().cloned() else {
            return;
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("lib-detail")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.;
                widgets::card_header(ui, "Model", |ui| {
                    if widgets::icon_button(ui, widgets::Glyph::Close, "Close the details", true)
                        .clicked()
                    {
                        acts.push(Act::Select(None));
                    }
                });
                // The 3D preview, its size, triangles, fits and drag handle (library_preview.rs).
                self.library_detail_preview(ui, &m);
                ui.horizontal(|ui| {
                    let (sr, sresp) = ui.allocate_exact_size(Vec2::splat(22.), Sense::click());
                    star(ui, sr.shrink(2.), m.user.favourite, sresp.hovered());
                    if sresp
                        .on_hover_text(if m.user.favourite {
                            "Remove from favorites"
                        } else {
                            "Add to favorites"
                        })
                        .clicked()
                    {
                        acts.push(Act::Favourite(m.id.clone()));
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&m.name)
                                .font(theme::semibold(15.))
                                .color(p.text),
                        )
                        .wrap(),
                    );
                });
                // Actions.
                let online = m.online();
                let path = m.primary().path.clone();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    let (label, tip) = match m.kind {
                        Kind::ThreeMf => (
                            "Open project",
                            "Open this project (replaces the current plates)",
                        ),
                        Kind::Stl => ("Add to plate", "Add this model to the current plate"),
                        Kind::Gcode | Kind::Bgcode => (
                            "Open",
                            "Show this G-code file in Preview (read-only) and send it to a printer",
                        ),
                        _ => ("Open", "Mica can't open this file type yet"),
                    };
                    let can = online && (m.kind.openable() || m.kind.is_gcode());
                    let r = widgets::primary_button(ui, label, can).on_hover_text(if online {
                        tip
                    } else {
                        "The folder holding this file is offline"
                    });
                    if r.clicked() {
                        acts.push(match m.kind {
                            Kind::ThreeMf => Act::OpenProject(path.clone()),
                            Kind::Gcode | Kind::Bgcode => Act::OpenGcode(path.clone()),
                            _ => Act::AddToPlate(path.clone()),
                        });
                    }
                    if widgets::button(ui, "Show in Finder", online)
                        .on_hover_text("Reveal the file in Finder")
                        .clicked()
                    {
                        acts.push(Act::Reveal(path.clone()));
                    }
                });
                if !m.info.readable
                    && let Some(e) = &m.info.error
                {
                    muted(ui, e, 11.5);
                }
                // Facts.
                widgets::group_title(ui, "Details");
                let i = &m.info;
                let mut facts: Vec<(&str, String)> = vec![("Type", m.kind.label().to_string())];
                if let Some(s) = i.size_mm {
                    facts.push(("Size", library::dims(s)));
                }
                if i.plates > 0 && m.kind == Kind::ThreeMf {
                    facts.push(("Plates", i.plates.to_string()));
                }
                if i.objects > 0 && m.kind == Kind::ThreeMf {
                    facts.push(("Objects", i.objects.to_string()));
                }
                if i.triangles > 0 {
                    facts.push(("Triangles", i.triangles.to_string()));
                }
                if let Some(x) = &i.printer {
                    facts.push(("Printer", x.clone()));
                }
                if let Some(x) = &i.print_profile {
                    facts.push(("Print profile", preset_names::display_name(x).to_string()));
                }
                if !i.filament_profiles.is_empty() {
                    let mut f: Vec<String> = i
                        .filament_profiles
                        .iter()
                        .map(|n| preset_names::display_name(n).to_string())
                        .collect();
                    f.dedup();
                    facts.push(("Filament", f.join(", ")));
                }
                if i.sliced {
                    facts.push(("Sliced", "Yes, ready to print".into()));
                }
                if let Some(t) = i.print_time_s {
                    facts.push(("Print time", library::duration(t)));
                }
                if let Some(g) = i.filament_g {
                    let m_ = i
                        .filament_m
                        .map(|m| format!(" · {m:.1} m"))
                        .unwrap_or_default();
                    facts.push(("Filament used", format!("{g:.1} g{m_}")));
                }
                facts.push(("File size", library::bytes(m.size())));
                facts.push(("Modified", library::date(m.modified())));
                egui::Grid::new("lib-facts")
                    .num_columns(2)
                    .spacing([10., 5.])
                    .show(ui, |ui| {
                        for (k, val) in &facts {
                            ui.label(
                                egui::RichText::new(*k)
                                    .font(theme::sans(12.))
                                    .color(p.muted),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(val)
                                        .font(theme::sans(12.))
                                        .color(p.text),
                                )
                                .wrap(),
                            );
                            ui.end_row();
                        }
                        if !i.colours.is_empty() {
                            ui.label(
                                egui::RichText::new("Colors")
                                    .font(theme::sans(12.))
                                    .color(p.muted),
                            );
                            swatches(ui, &i.colours, 13.);
                            ui.end_row();
                        }
                    });
                // Where it is.
                widgets::group_title(ui, "Location");
                let loc = m.primary();
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(loc.path.display().to_string())
                            .font(theme::mono(11.))
                            .color(if loc.online { p.label } else { p.muted }),
                    )
                    .wrap(),
                );
                if m.locations.len() > 1 {
                    ui.label(
                        egui::RichText::new(format!(
                            "Also in {} other place{}:",
                            m.locations.len() - 1,
                            if m.locations.len() > 2 { "s" } else { "" }
                        ))
                        .font(theme::medium(12.))
                        .color(p.text),
                    );
                    for l in &m.locations[1..] {
                        let text = format!(
                            "{} · {}",
                            folder_name(&l.root),
                            if l.folder().is_empty() {
                                "(top)".to_string()
                            } else {
                                l.folder()
                            }
                        );
                        if widgets::link(ui, &text)
                            .on_hover_text(format!("{}\nShow in Finder", l.path.display()))
                            .clicked()
                        {
                            acts.push(Act::Reveal(l.path.clone()));
                        }
                    }
                }
                // Tags.
                widgets::group_title(ui, "Tags");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                    for t in &m.user.tags {
                        if pill(ui, &format!("# {t}  ×"), true)
                            .on_hover_text("Remove this tag")
                            .clicked()
                        {
                            acts.push(Act::RemoveTag(m.id.clone(), t.clone()));
                        }
                    }
                });
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.library.tag_input)
                        .id(egui::Id::unique("lib-tag-input"))
                        .hint_text(
                            egui::RichText::new("Add a tag and press Enter")
                                .color(p.muted)
                                .font(theme::sans(12.)),
                        )
                        .font(theme::sans(12.))
                        .desired_width(ui.available_width()),
                );
                if r.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && !self.library.tag_input.trim().is_empty()
                {
                    let t = std::mem::take(&mut self.library.tag_input);
                    acts.push(Act::AddTag(m.id.clone(), t.trim().to_string()));
                    r.request_focus();
                }
            });
    }

    /// Double-clicking a model while the plate has objects: add to it, or start over.
    fn library_prompt(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(prompt) = self.library.open_prompt.clone() else {
            return;
        };
        let modified = self.is_modified();
        let mut open = true;
        egui::Window::new("Open model")
            .id(egui::Id::unique("lib-open-prompt"))
            .open(&mut open)
            .anchor(Align2::CENTER_TOP, Vec2::new(0., 140.))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(380.);
                let p = theme::of(ui);
                ui.label(
                    egui::RichText::new(format!("“{}”", prompt.name))
                        .font(theme::semibold(14.))
                        .color(p.text),
                );
                muted(ui, "The current plate already has objects. Add this model next to them, or start a new project with it?", 12.);
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, "Add to current plate", true).clicked() {
                        acts.push(Act::AddToPlate(prompt.path.clone()));
                    }
                    if widgets::button(ui, "New project", !modified)
                        .on_hover_text(if modified {
                            "Save or discard the current project first (File › New project)"
                        } else {
                            "Start over with only this model"
                        })
                        .clicked()
                    {
                        acts.push(Act::NewProjectWith(prompt.path.clone()));
                    }
                    if widgets::button(ui, "Cancel", true).clicked() {
                        acts.push(Act::ClosePrompt);
                    }
                });
            });
        if !open {
            acts.push(Act::ClosePrompt);
        }
    }

    /// Applies one library action (the monkey test drives the view through this too).
    pub(crate) fn library_act(&mut self, a: Act, ctx: &egui::Context) {
        let to_prepare = |app: &mut App| {
            app.library.shown = false;
            app.library.open_prompt = None;
            app.machine = false;
            app.preview = false;
        };
        match a {
            Act::Select(id) => {
                if id != self.library.selected {
                    self.library.tag_input.clear();
                }
                self.library.picked.clear();
                self.library.selected = id;
            }
            Act::Pick(id) => {
                let v = &mut self.library;
                if v.selected.as_deref() == Some(id.as_str()) {
                    // Leaving the primary: the next picked one takes over.
                    v.selected = v.picked.pop_first();
                } else if !v.picked.remove(&id) {
                    if let Some(s) = v.selected.take() {
                        v.picked.insert(s);
                    }
                    v.selected = Some(id);
                }
                v.tag_input.clear();
            }
            Act::Open(id) => {
                let Some(m) = self.library.model(&id).cloned() else {
                    return;
                };
                self.library.selected = Some(id);
                let path = m.primary().path.clone();
                if !m.online() {
                    self.notify_warning(format!("{} is on a folder that is offline", m.name));
                } else if m.kind.is_gcode() {
                    self.library_act(Act::OpenGcode(path), ctx);
                } else if !m.kind.openable() {
                    self.notify_warning(format!("Mica can't open {} files yet", m.kind.label()));
                } else if m.kind == Kind::ThreeMf {
                    self.library_act(Act::OpenProject(path), ctx);
                } else if self.plates.current().objects.is_empty() {
                    self.library_act(Act::AddToPlate(path), ctx);
                } else {
                    self.library.open_prompt = Some(OpenPrompt {
                        path,
                        name: m.name.clone(),
                    });
                }
            }
            Act::AddToPlate(path) | Act::OpenProject(path) => {
                to_prepare(self);
                if self.headless {
                    let result =
                        crate::app::load_files(vec![path], &self.config, self.bed.center());
                    self.finish_load(result);
                } else {
                    self.load(vec![path], ctx);
                }
            }
            Act::OpenGcode(path) => {
                self.library.shown = false;
                if self.headless {
                    if let Err(e) = self.open_gcode_now(&path) {
                        self.notify_error("The G-code file could not be opened", e);
                    }
                } else {
                    self.open_gcode(path, ctx);
                }
            }
            Act::NewProjectWith(path) => {
                if self.is_modified() {
                    return;
                }
                self.new_project_now();
                self.library_act(Act::AddToPlate(path), ctx);
            }
            Act::Reveal(path) => {
                if self.headless {
                    self.status = format!("Show in Finder: {}", path.display());
                } else if let Err(e) = library::reveal(&path) {
                    self.notify_error(
                        "Could not show the file",
                        format!("Could not show {}: {e}", path.display()),
                    );
                }
            }
            Act::Favourite(id) => self.library.toggle_favourite(&id),
            Act::AddTag(id, t) => self.library.add_tag(&id, &t),
            Act::RemoveTag(id, t) => self.library.remove_tag(&id, &t),
            Act::AddFolder(path) => {
                // Headless runs (the monkey test, screenshots) only ever index temporary
                // folders, never the user's own.
                let tmp = std::env::temp_dir();
                let tmp_real = std::fs::canonicalize(&tmp).unwrap_or_else(|_| tmp.clone());
                if self.headless && !path.starts_with(&tmp) && !path.starts_with(&tmp_real) {
                    self.library.notice = Some("Headless runs only index temporary folders".into());
                    return;
                }
                self.library.add_folder(&path);
                if self.library.notice.is_none() {
                    self.library.folder_input.clear();
                }
            }
            Act::PickFolder => {
                if !self.headless
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title("Add a library folder")
                        .pick_folder()
                {
                    self.library.add_folder(&dir);
                }
            }
            Act::RemoveFolder(path) => {
                if let Some(l) = &self.library.lib {
                    l.remove_folder(&path);
                }
                if self.library.filter.folder.as_ref() == Some(&path) {
                    self.library.filter.folder = None;
                }
            }
            Act::Rescan(f) => {
                if let Some(l) = &self.library.lib {
                    l.rescan(f.as_deref());
                }
            }
            Act::Retry => self.library.retry(),
            Act::FolderFilter(f) => self.library.filter.folder = f,
            Act::ClosePrompt => self.library.open_prompt = None,
            Act::OpenFolder(f) => self.library.open_project = f,
            Act::AddAllToPlate(paths) => {
                if paths.is_empty() {
                    return;
                }
                to_prepare(self);
                if self.headless {
                    let result = crate::app::load_files(paths, &self.config, self.bed.center());
                    self.finish_load(result);
                } else {
                    self.load(paths, ctx);
                }
            }
            Act::SetGrouping(folder, g) => {
                if g == Some(Grouping::Split) && self.library.open_project.as_ref() == Some(&folder)
                {
                    self.library.open_project = None;
                }
                self.library.set_grouping(&folder, g);
            }
        }
    }

    /// Settings › Files: the same folder list as the Library view.
    pub(crate) fn library_section(&mut self, ui: &mut egui::Ui) {
        widgets::group_title(ui, "Model library");
        let mut acts = vec![];
        self.library.refresh();
        muted(
            ui,
            "Folders Mica indexes for the Library tab. They are only read, never changed; the \
             index and thumbnails are kept in Mica's own data folder.",
            11.5,
        );
        if let Some(e) = &self.library.error {
            library_error(ui, e, &mut acts);
            for a in acts {
                self.library_act(a, ui.ctx());
            }
            return;
        }
        if self.library.loading() {
            muted(ui, "Opening library…", 12.);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if self.library.lib.is_none() {
            muted(ui, "The library is not available in this window.", 11.5);
            return;
        }
        let read_only = self.library.lib.as_ref().is_some_and(|l| l.read_only());
        if read_only {
            muted(ui, mica_library::READ_ONLY_NOTE, 12.);
        }
        folders_list(ui, &self.library, &mut acts, false);
        let headless = self.headless;
        if !read_only {
            add_folder_row(ui, &mut self.library, &mut acts, headless);
        }
        ui.add_space(10.);
        let ctx = ui.ctx().clone();
        for a in acts {
            self.library_act(a, &ctx);
        }
    }
}

/// Keeps a screenshot's temporary library folders alive until the image is written.
pub struct DemoGuard {
    _dirs: Vec<mica_library::testkit::TempDir>,
}

/// `--library*` screenshot flags taking a value.
pub const SHOT_VALUE_FLAGS: [&str; 9] = [
    "--library-group",
    "--library-project",
    "--library-search",
    "--library-select",
    "--library-filter",
    "--library-files",
    "--library-sort",
    "--library-drag",
    "--library-drop-part",
];

/// Headless screenshots: builds a synthetic library (from the repository's test models and
/// generated files) in a temporary folder, indexes it and shows the Library tab.
///
/// `--library` shows the grid; `--library-list` the list; `--library-search TEXT`;
/// `--library-select N` (one-based, in display order) opens the details; `--library-filter
/// multi|sliced|favourites|stl|3mf|gcode|small`; `--library-sort name|modified|size|time`;
/// `--library-group projects|folder|none|printer|type|tag`; `--library-project NAME` shows
/// the files of the project whose name contains NAME; `--library-files N` (default 36, plus a
/// few downloads with parts, a 3MF, G-code and pictures); `--library-offline` adds a second folder that then goes
/// offline; `--library-scanning` stops the scan half-way (progress showing); `--library-empty`
/// shows the first-run state; `--library-pending` leaves rendered thumbnails pending;
/// `--library-drag N[,M]` drags models over Prepare (with `--pointer`); `--library-drop-part N`
/// drops a model onto the plate's first object (the part-kind choice shows);
/// `--library-printers` sets up My printers (U1, CORE One, XL) for the "Fits" line.
pub fn screenshot_setup(app: &mut App, flags: &[String]) -> Result<DemoGuard, String> {
    use mica_library::testkit::{TempDir, synthetic_library, synthetic_range};
    let value = |name: &str| {
        flags
            .iter()
            .position(|f| f == name)
            .and_then(|i| flags.get(i + 1))
            .cloned()
    };
    let has = |name: &str| flags.iter().any(|f| f == name);
    let data = TempDir::new("shot-data");
    let lib_dir = TempDir::new("shot-lib");
    let library_root = lib_dir.path().join("Models");
    let n: usize = value("--library-files")
        .and_then(|v| v.parse().ok())
        .unwrap_or(36);
    app.library.open(
        data.path(),
        mica_library::Options {
            watch: false,
            threads: 4,
            // `--library-pending`: thumbnails not rendered yet (placeholders show).
            render: !has("--library-pending"),
            ..Default::default()
        },
    );
    app.library.shown = true;
    let t = std::time::Duration::from_secs(120);
    let Some(lib) = &app.library.lib else {
        return Err(app.library.error.clone().unwrap_or_default());
    };
    if has("--library-empty") {
        lib.wait_idle(t);
    } else {
        synthetic_library(&library_root, n).map_err(|e| e.to_string())?;
        mica_library::testkit::synthetic_downloads(&library_root, 0).map_err(|e| e.to_string())?;
        if has("--library-offline") {
            let nas = lib_dir.path().join("NAS Models");
            synthetic_range(&nas.join("Archive"), 1001..1013).map_err(|e| e.to_string())?;
            lib.add_folder(&nas)?;
            lib.wait_idle(t);
            // The share goes away: its models stay, marked offline.
            std::fs::rename(&nas, lib_dir.path().join("NAS Models-unmounted"))
                .map_err(|e| e.to_string())?;
            lib.rescan(None);
            lib.wait_idle(t);
        }
        lib.add_folder(&library_root)?;
        if has("--library-scanning") {
            lib.wait_for(t, |s| s.files >= n / 2);
            lib.set_paused(true);
        } else {
            lib.wait_idle(t);
        }
        // A library someone uses: a few favourites and tags.
        let snap = lib.snapshot();
        for (i, m) in snap.models.iter().enumerate() {
            match i % 9 {
                0 => lib.set_favourite(&m.id, true),
                3 => lib.set_tags(&m.id, &["Workshop".into()]),
                5 => lib.set_tags(&m.id, &["Gifts".into(), "Kids".into()]),
                _ => {}
            }
        }
        if !has("--library-scanning") {
            lib.wait_idle(t);
        }
    }
    let v = &mut app.library;
    v.list = has("--library-list");
    if let Some(s) = value("--library-search") {
        v.filter.text = s;
    }
    match value("--library-filter").as_deref() {
        None => {}
        Some("multi") => v.filter.colours = Colours::Multi,
        Some("sliced") => v.filter.sliced = Sliced::Yes,
        Some("favourites") => v.filter.favourites = true,
        Some("stl") => v.toggle_kind(Kind::Stl),
        Some("3mf") => v.toggle_kind(Kind::ThreeMf),
        Some("gcode") => v.toggle_kind(Kind::Gcode),
        Some("small") => v.filter.max_size_mm = Some(50.),
        Some(x) => return Err(format!("--library-filter: unknown filter {x}")),
    }
    match value("--library-sort").as_deref() {
        None | Some("name") => {}
        Some("modified") => (v.sort, v.descending) = (Sort::Modified, true),
        Some("size") => (v.sort, v.descending) = (Sort::Dimensions, true),
        Some("time") => (v.sort, v.descending) = (Sort::PrintTime, true),
        Some(x) => return Err(format!("--library-sort: unknown order {x}")),
    }
    if let Some(g) = value("--library-group") {
        v.group_by = *GroupBy::ALL
            .iter()
            .find(|x| x.label().eq_ignore_ascii_case(&g))
            .ok_or_else(|| format!("--library-group: unknown grouping {g}"))?;
    }
    v.refresh();
    if let Some(name) = value("--library-project") {
        let f = v
            .projects()
            .list
            .iter()
            .find(|p| p.name.to_lowercase().contains(&name.to_lowercase()))
            .map(|p| p.folder.clone())
            .ok_or_else(|| format!("--library-project: no project named {name}"))?;
        v.open_project = Some(f);
        v.refresh();
    }
    if let Some(i) = value("--library-select").and_then(|s| s.parse::<usize>().ok()) {
        let id = v
            .visible_at(i.saturating_sub(1))
            .map(|m| m.id.clone())
            .ok_or("--library-select: no such model")?;
        v.selected = Some(id);
    }
    // My printers for the "Fits" line (`--library-printers`): a U1, a CORE One and an XL.
    if has("--library-printers") {
        use crate::printhost::config::{Kind as HostKind, Printer};
        let all = app.profiles.all_printers();
        for (name, kind, preset) in [
            ("U1", HostKind::SnapmakerU1, "Snapmaker U1 (0.4 nozzle)"),
            (
                "CORE One",
                HostKind::PrusaLink,
                "Prusa CORE One HF0.4 nozzle",
            ),
            (
                "XL",
                HostKind::PrusaLink,
                "Original Prusa XL - 5T Input Shaper 0.4 nozzle",
            ),
        ] {
            let Some((_, vendor, preset)) = all.iter().find(|(_, _, n)| n == preset) else {
                continue;
            };
            let mut p = Printer::new(kind);
            p.name = name.into();
            p.address = "192.0.2.10".into();
            p.profile = Some(format!("{vendor}/{preset}"));
            app.printers.list.push(p);
        }
    }
    let v = &mut app.library;
    // A drag in progress (`--library-drag N[,M…]`, over Prepare; `--pointer` places it) or
    // models dropped onto the plate's first object (`--library-drop-part N`).
    let items = |spec: String, v: &LibraryView| -> Result<Vec<_>, String> {
        spec.split(',')
            .map(|n| {
                let i: usize = n.trim().parse().map_err(|_| "expected model numbers")?;
                let m = v.visible_at(i.saturating_sub(1)).ok_or("no such model")?;
                Ok(v.drag_items(m).remove(0))
            })
            .collect()
    };
    if let Some(spec) = value("--library-drag") {
        v.shot = Some(crate::library_drop::Shot::Hover(items(spec, v)?));
        v.shown = false;
    }
    if let Some(spec) = value("--library-drop-part") {
        v.shot = Some(crate::library_drop::Shot::DropOnObject(items(spec, v)?, 0));
        v.shown = false;
    }
    Ok(DemoGuard {
        _dirs: vec![lib_dir, data],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mica_library::store::{Changes, Store};
    use mica_library::testkit::{TempDir, synthetic_library};
    use std::time::Duration;

    fn frame(
        ctx: &egui::Context,
        app: &mut App,
        events: Vec<egui::Event>,
        preferences: bool,
    ) -> Vec<(String, Pos2)> {
        fn texts(shape: egui::Shape, out: &mut Vec<(String, Pos2)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.pos + t.galley.size() / 2.));
                }
                egui::Shape::Vec(shapes) => {
                    for s in shapes {
                        texts(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440., 900.))),
                events,
                ..Default::default()
            },
            |ui| {
                if preferences {
                    app.library_section(ui);
                } else {
                    app.library_view(ui);
                }
            },
        );
        let mut out = vec![];
        for s in output.shapes {
            texts(s.shape, &mut out);
        }
        output.textures_delta.clear();
        out
    }

    fn click_retry(ctx: &egui::Context, app: &mut App, at: Pos2) {
        frame(ctx, app, vec![egui::Event::PointerMoved(at)], false);
        for pressed in [true, false] {
            frame(
                ctx,
                app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                false,
            );
        }
    }

    #[test]
    fn in_use_library_is_read_only_and_taken_over() {
        let dir = TempDir::new("gui-in-use");
        let data = dir.path().join("library");
        let models = dir.path().join("models");
        std::fs::create_dir_all(&data).unwrap();
        synthetic_library(&models, 1).unwrap();
        let index = data.join("index.redb");
        // Another Mica holds the index; it has indexed one model (a file since removed).
        let (first, _, _) = Store::open(&index).unwrap();
        first
            .write(&Changes {
                folders: Some(vec![models.clone()]),
                files: vec![(
                    models.join("gone.stl"),
                    Some(mica_library::FileRecord {
                        root: models.clone(),
                        size: 3,
                        mtime_ns: 7,
                        hash: "ab".into(),
                        kind: mica_library::Kind::Stl,
                    }),
                )],
                models: vec![("ab".into(), Some(Default::default()))],
                ..Default::default()
            })
            .unwrap();
        let ctx = egui::Context::default();
        let mut app = App::create(
            &ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(
                    dir.path().join("profiles"),
                )),
                files: vec![],
                headless: true,
            },
        );
        app.library.open(
            &data,
            mica_library::Options {
                watch: false,
                takeover_check: Duration::from_millis(100),
                ..Default::default()
            },
        );
        let timeout = Duration::from_secs(10);
        assert!(app.library.lib.as_ref().unwrap().wait_idle(timeout));
        assert!(app.library.lib.as_ref().unwrap().read_only());
        let note = mica_library::READ_ONLY_NOTE;
        for preferences in [true, false] {
            // A second frame completes egui's initial layout pass.
            frame(&ctx, &mut app, vec![], preferences);
            let text = frame(&ctx, &mut app, vec![], preferences);
            assert!(text.iter().any(|(t, _)| t == note), "{text:?}");
            assert!(!text.iter().any(|(t, _)| t == "Retry"));
            assert!(!text.iter().any(|(t, _)| t.contains("library is empty")));
        }
        assert!(app.library.error.is_none());
        // Browsing works from what the other Mica committed.
        assert_eq!(app.library.snapshot().folders[0].path, models);
        assert_eq!(app.library.snapshot().models.len(), 1);
        // Changes are refused and nothing is written, moved or set aside.
        let before = std::fs::read(&index).unwrap();
        app.library.add_folder(dir.path());
        assert_eq!(app.library.notice.as_deref(), Some(note));
        app.library.toggle_favourite("ab");
        app.library.add_tag("ab", "Mine");
        app.library.refresh();
        assert!(!app.library.snapshot().models[0].user.favourite);
        assert!(app.library.snapshot().models[0].user.tags.is_empty());
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(std::fs::read(&index).unwrap(), before);
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 2); // index + thumbs
        // Once the other Mica quits, this one takes the index over by itself and scans.
        drop(first);
        let lib = app.library.lib.as_ref().unwrap();
        let start = std::time::Instant::now();
        while lib.read_only() && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!lib.read_only());
        assert!(lib.wait_idle(timeout));
        let text = frame(&ctx, &mut app, vec![], false);
        assert!(app.library.error.is_none());
        assert!(!text.iter().any(|(t, _)| t == note));
        let snap = app.library.snapshot();
        assert_eq!(snap.models.len(), 1);
        assert_ne!(snap.models[0].id, "ab");
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 2);
    }

    #[test]
    fn projects_open_split_and_group_by() {
        let dir = TempDir::new("gui-projects");
        let data = dir.path().join("library");
        let models = dir.path().join("models");
        mica_library::testkit::synthetic_downloads(&models, 0).unwrap();
        synthetic_library(&models.join("Mixed"), 8).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::create(
            &ctx,
            crate::app::Options {
                prefs_path: None,
                store: Some(mica_profiles::user::Store::open(
                    dir.path().join("profiles"),
                )),
                files: vec![],
                headless: true,
            },
        );
        app.library.open(
            &data,
            mica_library::Options {
                watch: false,
                ..Default::default()
            },
        );
        let lib = app.library.lib.as_ref().unwrap();
        lib.add_folder(&models).unwrap();
        assert!(lib.wait_idle(Duration::from_secs(60)));
        app.library.shown = true;
        let has = |text: &[(String, Pos2)], t: &str| text.iter().any(|(x, _)| x == t);
        frame(&ctx, &mut app, vec![], false);
        let text = frame(&ctx, &mut app, vec![], false);
        // One card for the dragon download, with what it holds; its parts are inside.
        assert!(has(&text, "Articulated Dragon"));
        assert!(has(&text, "3MF + 10 parts · 1 G-code"));
        assert!(has(&text, "Desk Hook"));
        assert!(!has(&text, "wing_left.stl"));
        assert!(text.iter().any(|(t, _)| t.starts_with("3 projects")));
        let at = text
            .iter()
            .find(|(t, _)| t == "Articulated Dragon")
            .unwrap()
            .1;
        click_retry(&ctx, &mut app, at);
        let folder = app.library.open_project.clone().expect("project opened");
        assert!(folder.ends_with("Articulated_Dragon-model_files"));
        frame(&ctx, &mut app, vec![], false);
        let text = frame(&ctx, &mut app, vec![], false);
        for t in [
            "Open project",
            "Add all parts to plate",
            "wing_left.stl",
            "Also in 1 other folder",
            "Parts",
        ] {
            assert!(has(&text, t), "{t} missing");
        }
        // Back, then search: the project shows with how many of its files match.
        app.library_act(Act::OpenFolder(None), &ctx);
        app.library.filter.text = "wing".into();
        frame(&ctx, &mut app, vec![], false);
        let text = frame(&ctx, &mut app, vec![], false);
        assert!(has(&text, "Articulated Dragon") && has(&text, "2 matching files"));
        // Split: the parts are single cards again (and the choice is kept by folder).
        app.library_act(
            Act::SetGrouping(folder.clone(), Some(Grouping::Split)),
            &ctx,
        );
        frame(&ctx, &mut app, vec![], false);
        let text = frame(&ctx, &mut app, vec![], false);
        assert!(!has(&text, "Articulated Dragon"));
        assert!(has(&text, "wing_left.stl"));
        assert_eq!(app.library.grouping_of(&folder), Some(Grouping::Split));
        // Group by folder: a header per folder.
        app.library.filter.text.clear();
        app.library.group_by = GroupBy::Folder;
        frame(&ctx, &mut app, vec![], false);
        let text = frame(&ctx, &mut app, vec![], false);
        assert!(
            text.iter()
                .any(|(t, _)| t.ends_with("Downloads › Articulated_Dragon-model_files › files"))
        );
        // Adding every part puts ten objects on the plate.
        app.library_act(Act::SetGrouping(folder.clone(), None), &ctx);
        app.library.refresh();
        let p = app.library.projects().list[0].clone();
        let models = app.library.snapshot().models.clone();
        let parts: Vec<PathBuf> = p
            .plate_parts(&models)
            .iter()
            .map(|f| p.path(&models, *f).to_path_buf())
            .collect();
        app.library_act(Act::AddAllToPlate(parts), &ctx);
        assert_eq!(app.plates.current().objects.len(), 10);
        assert!(!app.library.shown);
    }
}
