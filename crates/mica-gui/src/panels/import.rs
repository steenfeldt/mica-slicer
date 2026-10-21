// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Profiles from other slicers, on the user's terms: the startup notice ("Found 3 printer and
//! 8 filament profiles in OrcaSlicer"), the review window that imports a chosen selection, the
//! source badges in the preset pickers (grouped My profiles / Imported / System, presets for
//! other printers behind "Show all"), the filament picker's visible filaments (starred in an
//! inline list with search), and Settings › Profiles to import again, remove imported
//! profiles, merge duplicates and re-import changed ones. Logic in `crate::import`.

use super::preferences::item;
use super::profiles::{Origin, PresetKind, Profiles};
use super::widgets;
use crate::app::App;
use crate::import::{Candidate, Stale};
use crate::notifications::{Action, Level, Notification};
use crate::preset_names::{self, Label};
use crate::theme;
use eframe::egui::{self, Align, Layout, Sense, Vec2};
use mica_profiles::import::{Found, Source, Status};
use mica_profiles::prusa::Kind;
use mica_profiles::user::{UserProfile, kind_name};
use std::collections::BTreeMap;

/// An imported group in a picker folds when it holds more than this many presets.
const FOLD_OVER: usize = 8;

/// The import windows' state.
#[derive(Default)]
pub struct ImportUi {
    /// The review window, when open.
    pub review: Option<Review>,
    /// What the startup notice offered (marked as asked about once answered).
    pub offered: Vec<Found>,
    /// Settings › Profiles: changed-source and duplicate lists, worked out when the tab shows.
    pub manage: Option<Manage>,
}

pub struct Review {
    pub rows: Vec<Candidate>,
    /// Files that could not be read.
    pub errors: Vec<String>,
    /// Per slicer in the list: keep asking about new profiles.
    pub ask: BTreeMap<Source, bool>,
}

pub struct Manage {
    pub stale: Vec<Stale>,
    pub duplicates: Vec<Vec<(Kind, String)>>,
}

/// What a click in a grouped preset list asked for.
pub(crate) enum MenuPick {
    Choose(String),
    ShowAll(bool),
    /// The filament picker's "Choose visible filaments…".
    ChooseVisible,
    /// The filament picker's "Show all": every filament made for the printer is listed.
    AllFilaments,
}

/// The filament picker's visible-filament state, for its footer.
#[derive(Clone, Copy)]
pub(crate) struct Visibility {
    /// System filaments made for the printer that the picker leaves out.
    pub hidden: usize,
}

/// A temporary-data key (the search text) that opens T1's visible-filament list (screenshots).
pub(crate) const OPEN_VISIBLE_FILAMENTS: &str = "mica-open-visible-filaments";

/// What the visible-filament list asked for.
pub(crate) enum ChooseAct {
    Toggle(String, bool),
    ShowAll,
    Done,
}

/// `2026-10-20` for seconds since 1970 (UTC).
pub(crate) fn date(secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The badge tooltip of an imported profile: where it came from and when.
pub(crate) fn origin_text(p: &UserProfile) -> String {
    let Some(source) = p.source() else {
        return String::new();
    };
    let mut text = format!("Imported from {}", source.app_name());
    match &p.import {
        Some(i) => {
            if i.original_name != p.name {
                text.push_str(&format!("\nName there: {}", i.original_name));
            }
            text.push_str(&format!("\nFile: {}", i.path.display()));
            if i.date > 0 {
                text.push_str(&format!("\nImported on {}", date(i.date)));
            }
        }
        None => {
            if let Some(path) = p.source_path() {
                text.push_str(&format!("\nFile: {}", path.display()));
            }
        }
    }
    text
}

/// A small pill naming the slicer a profile came from ("Orca", "PrusaSlicer").
pub(crate) fn source_badge(ui: &mut egui::Ui, source: Source, tooltip: &str) -> egui::Response {
    let p = theme::of(ui);
    let galley =
        ui.painter()
            .layout_no_wrap(source.badge().to_string(), theme::sans(10.5), p.muted);
    let size = Vec2::new(galley.size().x + 10., 16.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        8.,
        egui::Stroke::new(1., p.border_raised),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(rect.left() + 5., rect.center().y - galley.size().y / 2.),
        galley,
        p.muted,
    );
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// The badge of a listed preset, if it was imported.
pub(crate) fn preset_badge(ui: &mut egui::Ui, profiles: &Profiles, kind: PresetKind, name: &str) {
    if let Origin::Imported(source) = profiles.origin(kind, name) {
        let tip = profiles
            .user_profile(kind, name)
            .map(origin_text)
            .unwrap_or_default();
        source_badge(ui, source, &tip);
    }
}

/// One preset of a picker: its shown name (`label`, see `crate::preset_names`), with the
/// suffix that tells it apart muted, dimmed when it is made for other printers; the full
/// name in the tooltip. Clicked?
pub(crate) fn preset_row(
    ui: &mut egui::Ui,
    profiles: &Profiles,
    kind: PresetKind,
    name: &str,
    label: &Label,
    selected: bool,
) -> bool {
    let other = profiles.is_incompatible(kind, name);
    let colour = if other {
        theme::of(ui).muted
    } else {
        theme::of(ui).text
    };
    let job = preset_names::text(ui, label, colour);
    let r = ui.selectable_label(selected, job);
    let tip = match (other, preset_names::tooltip(name, label)) {
        (true, Some(full)) => Some(format!("{full}\nMade for other printers")),
        (true, None) => Some("Made for other printers".to_string()),
        (false, full) => full,
    };
    match tip {
        Some(t) => r.on_hover_text(t).clicked(),
        None => r.clicked(),
    }
}

fn group_header(ui: &mut egui::Ui, text: &str) {
    let p = theme::of(ui);
    ui.add_space(2.);
    ui.label(
        egui::RichText::new(widgets::spaced_upper(text))
            .font(theme::sans(10.5))
            .color(p.muted),
    );
}

/// The order pickers group presets in: the user's own first, then imported, then system.
pub(crate) fn origin_rank(origin: Origin) -> usize {
    match origin {
        Origin::Mine => 0,
        Origin::Imported(_) => 1,
        Origin::System => 2,
    }
}

/// A print or filament picker's list: My profiles, Imported (folded when long) and System
/// groups, then, with "Show all", the presets made for other printers. `row` draws one
/// preset's row and says whether it was clicked. `visibility`: the filament picker's footer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn grouped_presets(
    ui: &mut egui::Ui,
    id: egui::Id,
    profiles: &Profiles,
    kind: PresetKind,
    names: &[String],
    current: &str,
    visibility: Option<Visibility>,
    mut row: impl FnMut(&mut egui::Ui, &str, &Label) -> bool,
) -> Option<MenuPick> {
    let mut groups: [(&str, Vec<(&String, Label)>); 4] = [
        ("My profiles", vec![]),
        ("Imported", vec![]),
        ("System", vec![]),
        ("For other printers", vec![]),
    ];
    // Every listed preset counts for telling shown names apart, folded groups too.
    let labels = preset_names::labels_ranked(
        names
            .iter()
            .map(|n| (n.as_str(), profiles.is_incompatible(kind, n))),
    );
    for (n, label) in names.iter().zip(labels) {
        let g = if profiles.is_incompatible(kind, n) {
            3
        } else {
            origin_rank(profiles.origin(kind, n))
        };
        groups[g].1.push((n, label));
    }
    let headed = groups.iter().filter(|(_, g)| !g.is_empty()).count() > 1;
    let mut pick = None;
    // The list scrolls to the selected preset in its first frames after opening (it wasn't
    // drawn the frame before), until the scroll area knows its content's height.
    let frame = ui.ctx().cumulative_frame_nr();
    let seen_id = id.with("list-frames");
    let opened = ui
        .data(|d| d.get_temp::<(u64, u64)>(seen_id))
        .filter(|(_, last)| last + 1 >= frame)
        .map_or(frame, |(opened, _)| opened);
    ui.data_mut(|d| d.insert_temp(seen_id, (opened, frame)));
    let opening = frame - opened < 3;
    egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .max_height(420.)
        .show(ui, |ui| {
            for (gi, (title, list)) in groups.iter().enumerate() {
                if list.is_empty() {
                    continue;
                }
                let mut open = true;
                if gi == 1 && list.len() > FOLD_OVER {
                    let fold_id = id.with("imported-open");
                    let holds_current = list.iter().any(|(n, _)| *n == current);
                    open = ui
                        .data(|d| d.get_temp::<bool>(fold_id))
                        .unwrap_or(holds_current);
                    let label =
                        format!("{} {title} ({})", if open { "▾" } else { "▸" }, list.len());
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(label)
                                    .font(theme::sans(11.))
                                    .color(theme::of(ui).muted),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        open = !open;
                        ui.data_mut(|d| d.insert_temp(fold_id, open));
                    }
                } else if headed {
                    group_header(ui, title);
                }
                if !open {
                    continue;
                }
                for (n, label) in list {
                    let r = ui.horizontal(|ui| {
                        let clicked = row(ui, n, label);
                        preset_badge(ui, profiles, kind, n);
                        clicked
                    });
                    if opening && *n == current {
                        r.response.scroll_to_me_animation(
                            Some(Align::Center),
                            egui::style::ScrollAnimation::none(),
                        );
                    }
                    let clicked = r.inner;
                    if clicked {
                        pick = Some(MenuPick::Choose(n.to_string()));
                    }
                }
            }
        });
    let others = profiles.hidden_count(kind);
    ui.separator();
    let mut all = profiles.show_incompatible;
    let label = if all || others == 0 {
        "Show profiles for other printers".to_string()
    } else {
        format!("Show profiles for other printers ({others})")
    };
    if ui
        .checkbox(&mut all, label)
        .on_hover_text(
            "Lists every profile, also those made for other printers (as PrusaSlicer's \
             \"show incompatible\" does). They may not print well on this printer.",
        )
        .changed()
    {
        pick = Some(MenuPick::ShowAll(all));
    }
    if let Some(v) = visibility {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.;
            if widgets::link(ui, "Choose visible filaments…")
                .on_hover_text("Star the filaments this printer's list shows")
                .clicked()
            {
                pick = Some(MenuPick::ChooseVisible);
            }
            if v.hidden > 0 {
                let p = theme::of(ui);
                ui.label(
                    egui::RichText::new(format!("· {} hidden ·", v.hidden))
                        .font(theme::sans(12.))
                        .color(p.muted),
                );
                if widgets::link(ui, "Show all")
                    .on_hover_text("List every filament made for this printer again")
                    .clicked()
                {
                    pick = Some(MenuPick::AllFilaments);
                }
            }
        });
    }
    pick
}

/// A star that turns a filament's place in the picker on or off. Clicked?
fn star(ui: &mut egui::Ui, on: bool) -> bool {
    let p = theme::of(ui);
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(18.), Sense::click());
    if on {
        let colour = if r.hovered() {
            p.accent_hi
        } else {
            p.accent_bright
        };
        widgets::star_filled(ui.painter(), rect.shrink(2.), colour);
    } else {
        let colour = if r.hovered() { p.text } else { p.muted };
        widgets::glyph(ui.painter(), rect.shrink(2.), widgets::Glyph::Star, colour);
    }
    r.on_hover_text(if on {
        "Listed in the picker. Click to hide it"
    } else {
        "Click to list it in the picker"
    })
    .clicked()
}

/// The filament picker's inline "Choose visible filaments" list: search first, the starred
/// filaments on top (in the order they had when the list opened, so rows don't jump), a star
/// per row, "Show all" and "Done". `filaments`: the system filaments made for the printer;
/// `visible`: whether one is listed now.
pub(crate) fn visible_chooser(
    ui: &mut egui::Ui,
    id: egui::Id,
    profiles: &Profiles,
    filaments: &[String],
    visible: impl Fn(&str) -> bool,
) -> Vec<ChooseAct> {
    let p = theme::of(ui);
    let mut acts = Vec::new();
    let order_id = id.with("choose-order");
    let (order, starred): (Vec<String>, usize) = ui
        .data(|d| d.get_temp::<(Vec<String>, usize)>(order_id))
        .filter(|(o, _)| o.len() == filaments.len())
        .unwrap_or_else(|| {
            let (mut on, off): (Vec<String>, Vec<String>) =
                filaments.iter().cloned().partition(|f| visible(f));
            let starred = on.len();
            on.extend(off);
            (on, starred)
        });
    ui.data_mut(|d| d.insert_temp(order_id, (order.clone(), starred)));
    ui.horizontal(|ui| {
        group_header(ui, "Visible filaments");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::link(ui, "Done").clicked() {
                acts.push(ChooseAct::Done);
            }
        });
    });
    let search_id = id.with("choose-search");
    let mut search: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let words: Vec<String> = search
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let matches = |n: &str| preset_names::matches(n, &words);
    let labels = preset_names::labels(order.iter().map(String::as_str));
    let shown = order.iter().filter(|n| visible(n)).count();
    if ui
        .data(|d| d.get_temp::<bool>(id.with("choose-focus")))
        .unwrap_or(true)
    {
        ui.memory_mut(|m| m.request_focus(search_id.with("search")));
        ui.data_mut(|d| d.insert_temp(id.with("choose-focus"), false));
    }
    widgets::search_field(
        ui,
        search_id,
        &mut search,
        "Search filaments",
        &format!("{shown} of {}", order.len()),
    );
    ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Starred filaments are listed in the picker. Your own and imported ones \
                     always are.",
                )
                .font(theme::sans(11.5))
                .color(p.muted),
            )
            .wrap(),
        );
    });
    ui.add_space(2.);
    egui::ScrollArea::vertical()
        .id_salt(id.with("choose-scroll"))
        .max_height(380.)
        .show(ui, |ui| {
            let mut any = false;
            let mut last_starred = None;
            for (i, n) in order.iter().enumerate().filter(|(_, n)| matches(n)) {
                let label = &labels[i];
                let on = visible(n);
                // The list as it opened: starred ones, then the rest under a rule.
                let was_starred = i < starred;
                if last_starred == Some(true) && !was_starred {
                    ui.separator();
                }
                last_starred = Some(was_starred);
                any = true;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    let mut toggle = star(ui, on);
                    if let Some(c) = profiles.filament_colour(n) {
                        super::swatch(ui, c, 12.);
                    } else {
                        ui.add_space(12.);
                    }
                    let colour = if on { p.text } else { p.muted };
                    let job = preset_names::text(ui, label, colour);
                    let r = ui.selectable_label(false, job);
                    let r = match preset_names::tooltip(n, label) {
                        Some(full) => r.on_hover_text(full),
                        None => r,
                    };
                    toggle |= r.clicked();
                    if toggle {
                        acts.push(ChooseAct::Toggle(n.clone(), !on));
                    }
                });
            }
            if !any {
                ui.label(
                    egui::RichText::new("No filament matches.")
                        .font(theme::sans(12.))
                        .color(p.muted),
                );
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        if widgets::link(ui, "Show all")
            .on_hover_text("List every filament made for this printer")
            .clicked()
        {
            acts.push(ChooseAct::ShowAll);
        }
    });
    acts
}

/// Forgets the visible-filament list's opening order, search and focus: it opens fresh.
pub(crate) fn reset_chooser(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| {
        d.remove::<(Vec<String>, usize)>(id.with("choose-order"));
        d.remove::<String>(id.with("choose-search"));
        d.remove::<bool>(id.with("choose-focus"));
        d.remove::<bool>(id.with("choosing"));
    });
}

impl Profiles {
    /// My printers as the printer picker lists them: the user's own, imported, then system
    /// ones, each in the order they were added.
    pub fn my_printers_in_picker_order(&self, prefs: &crate::state::Prefs) -> Vec<(usize, String)> {
        let mut mine = self.my_printers(prefs);
        mine.sort_by_key(|(_, n)| origin_rank(self.origin(PresetKind::Printer, n)));
        mine
    }

    /// How many presets of a kind "Show all" would add (none while it is on).
    pub fn hidden_count(&self, kind: PresetKind) -> usize {
        if self.show_incompatible {
            return 0;
        }
        let listed = match kind {
            PresetKind::Print => self.processes.len(),
            PresetKind::Filament => self.materials.len(),
            PresetKind::Printer => return 0,
        };
        self.all_presets(kind).len().saturating_sub(listed)
    }

    /// How many listed presets of a kind are not made for the selected printer.
    #[cfg(test)]
    pub fn incompatible_count(&self, kind: PresetKind) -> usize {
        self.incompatible
            .iter()
            .filter(|(k, _)| *k == kind.prusa())
            .count()
    }
}

impl App {
    /// At start: one notice naming the new presets found in other slicers. Nothing is imported
    /// until the user reviews them.
    pub(crate) fn offer_imports(&mut self) {
        let found = self.profiles.new_imports(&self.prefs);
        if found.is_empty() {
            return;
        }
        let phrase = crate::import::found_phrase(&found);
        let title = if phrase.chars().count() <= 44 {
            phrase.clone()
        } else {
            "Found profiles in other slicers".to_string()
        };
        let mut n = Notification::new(
            Level::Info,
            title,
            format!("{phrase}. Mica copies nothing until you choose what to import."),
        )
        .action("Review and import…", Action::ReviewImports)
        .action("Not now", Action::DismissImports);
        n.life = None;
        self.import_ui.offered = found;
        self.notify(n);
    }

    /// The offered presets count as asked about (the notice won't name them again).
    pub(crate) fn answer_import_offer(&mut self, id: u64) {
        let offered = std::mem::take(&mut self.import_ui.offered);
        self.prefs.import.mark_seen(&offered);
        self.prefs_dirty = true;
        self.notifications.dismiss(id);
    }

    /// Opens the review window with everything found in the other slicers.
    pub(crate) fn open_import_review(&mut self) {
        let (rows, errors) = self.profiles.import_candidates(&self.prefs);
        let mut ask = BTreeMap::new();
        for r in &rows {
            ask.insert(r.found.source, self.prefs.import.asks(r.found.source));
        }
        self.import_ui.review = Some(Review { rows, errors, ask });
    }

    pub(crate) fn import_review_window(&mut self, ctx: &egui::Context) {
        let Some(mut review) = self.import_ui.review.take() else {
            return;
        };
        let mut open = true;
        let mut run = false;
        let mut cancel = false;
        egui::Window::new("Import profiles")
            .id(egui::Id::unique("import-review"))
            .open(&mut open)
            .default_size([640., 600.])
            .default_pos(ctx.content_rect().center() - egui::vec2(310., 260.))
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.label(
                    egui::RichText::new(
                        "Checked profiles are copied into Mica. Profiles for your printers are \
                         checked. The other slicers' files are never changed.",
                    )
                    .font(theme::sans(12.))
                    .color(p.muted),
                );
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Select:").font(theme::sans(12.)));
                    if widgets::link(ui, "For my printers").clicked() {
                        for r in &mut review.rows {
                            r.selected = r.default_pick();
                        }
                    }
                    if widgets::link(ui, "All").clicked() {
                        for r in &mut review.rows {
                            r.selected = r.importable();
                        }
                    }
                    if widgets::link(ui, "None").clicked() {
                        for r in &mut review.rows {
                            r.selected = false;
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 320.).clamp(200., 560.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if review.rows.is_empty() {
                            ui.label("No profiles were found in OrcaSlicer, Snapmaker Orca or PrusaSlicer.");
                        }
                        for source in Source::ALL {
                            if !review.rows.iter().any(|r| r.found.source == source) {
                                continue;
                            }
                            ui.add_space(6.);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(source.app_name())
                                        .font(theme::medium(14.))
                                        .color(p.text),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let ask = review.ask.entry(source).or_insert(true);
                                    let mut dont = !*ask;
                                    if ui
                                        .checkbox(
                                            &mut dont,
                                            format!("Don't ask again for {}", source.app_name()),
                                        )
                                        .on_hover_text(
                                            "Mica won't mention new profiles from this slicer at \
                                             start. Settings › Profiles imports at any time.",
                                        )
                                        .changed()
                                    {
                                        *ask = !dont;
                                    }
                                });
                            });
                            for kind in [Kind::Printer, Kind::Filament, Kind::Print] {
                                let idx: Vec<usize> = review
                                    .rows
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, r)| r.found.source == source && r.found.kind == kind)
                                    .map(|(i, _)| i)
                                    .collect();
                                if idx.is_empty() {
                                    continue;
                                }
                                let checked = idx.iter().filter(|&&i| review.rows[i].selected).count();
                                egui::CollapsingHeader::new(format!(
                                    "{} profiles ({checked} of {} checked)",
                                    capitalized(kind_name(kind)),
                                    idx.len()
                                ))
                                .id_salt(("import-kind", source.key(), kind_name(kind)))
                                .default_open(idx.len() <= 12)
                                .show(ui, |ui| {
                                    let labels = preset_names::labels(
                                        idx.iter().map(|&i| review.rows[i].found.name.as_str()),
                                    );
                                    for (i, label) in idx.into_iter().zip(&labels) {
                                        review_row(ui, &mut review.rows[i], label);
                                    }
                                });
                            }
                        }
                        if !review.errors.is_empty() {
                            ui.add_space(6.);
                            egui::CollapsingHeader::new(format!(
                                "{} files could not be read",
                                review.errors.len()
                            ))
                            .id_salt("import-errors")
                            .show(ui, |ui| {
                                for e in &review.errors {
                                    ui.label(egui::RichText::new(e).font(theme::sans(11.)).color(p.muted));
                                }
                            });
                        }
                    });
                ui.separator();
                let n = review.rows.iter().filter(|r| r.selected && r.importable()).count();
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = match n {
                            0 => "Import".to_string(),
                            1 => "Import 1 profile".to_string(),
                            n => format!("Import {n} profiles"),
                        };
                        if widgets::primary_button(ui, &label, n > 0).clicked() {
                            run = true;
                        }
                        if widgets::button(ui, "Cancel", true).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        // The slicers' "ask" choices and the list count as answered whatever the outcome.
        if run || cancel || !open {
            for (source, ask) in &review.ask {
                self.prefs.import.set_asks(*source, *ask);
            }
            let found: Vec<Found> = review.rows.iter().map(|r| r.found.clone()).collect();
            self.prefs.import.mark_seen(&found);
            self.prefs_dirty = true;
        }
        if run {
            let chosen: Vec<&Candidate> = review
                .rows
                .iter()
                .filter(|r| r.selected && r.importable())
                .collect();
            let done = self.profiles.import(&chosen, &mut self.prefs);
            self.after_profile_cleanup();
            let n = done.names.len();
            if n > 0 {
                self.notify_info(format!(
                    "Imported {n} profile{}. They are marked with the slicer they came from.",
                    if n == 1 { "" } else { "s" }
                ));
            }
            if !done.errors.is_empty() {
                self.notify_error("Some profiles were not imported", done.errors.join("\n"));
            }
        } else if open && !cancel {
            self.import_ui.review = Some(review);
        }
    }

    /// After importing or removing profiles: the selection, the lists and the slice follow.
    pub(crate) fn after_profile_cleanup(&mut self) {
        self.import_ui.manage = None;
        self.profiles.store_selection(&mut self.prefs);
        self.settings_changed();
    }

    /// Settings › Profiles.
    pub(crate) fn profiles_section(&mut self, ui: &mut egui::Ui) {
        let p = theme::of(ui);
        widgets::group_title(ui, "Other slicers");
        let mut review = false;
        item(
            ui,
            "Import profiles from other slicers",
            "Mica looks for your own printer, filament and print profiles in OrcaSlicer, \
             Snapmaker Orca and PrusaSlicer and lets you choose which to copy. Their files are \
             never changed.",
            |ui| {
                review = widgets::button(ui, "Import profiles…", true).clicked();
            },
        );
        let mut sources: Vec<Source> = self.profiles.import_dirs.iter().map(|d| d.source).collect();
        sources.sort();
        sources.dedup();
        if !sources.is_empty() {
            let mut toggles = Vec::new();
            item(
                ui,
                "Ask about new profiles",
                "At start, Mica mentions profiles that appeared in these slicers since it last \
                 asked.",
                |ui| {
                    for &source in &sources {
                        let mut on = self.prefs.import.asks(source);
                        if ui.checkbox(&mut on, source.app_name()).changed() {
                            toggles.push((source, on));
                        }
                    }
                },
            );
            for (source, on) in toggles {
                self.prefs.import.set_asks(source, on);
                self.prefs_dirty = true;
            }
        }
        if review {
            self.open_import_review();
        }

        widgets::group_title(ui, "Imported profiles");
        let imported: Vec<UserProfile> = self.profiles.imported().into_iter().cloned().collect();
        if imported.is_empty() {
            ui.label(
                egui::RichText::new("None yet.")
                    .font(theme::sans(12.))
                    .color(p.muted),
            );
            ui.add_space(10.);
            return;
        }
        let manage = self.import_ui.manage.get_or_insert_with(|| Manage {
            stale: self.profiles.stale_imports(),
            duplicates: self.profiles.duplicate_groups(),
        });
        let stale = manage.stale.clone();
        let duplicates = manage.duplicates.clone();
        let mut remove: Option<Vec<(Kind, String)>> = None;
        let mut merge: Option<Vec<(Kind, String)>> = None;
        let mut reimport: Option<(Vec<(Kind, String)>, bool)> = None;
        for source in Source::ALL {
            let mine: Vec<&UserProfile> = imported
                .iter()
                .filter(|u| u.source() == Some(source))
                .collect();
            if mine.is_empty() {
                continue;
            }
            let count = |k: Kind| mine.iter().filter(|u| u.kind == k).count();
            let text = format!(
                "{}. Removing moves Mica's copies to its trash folder; {} keeps its own.",
                mica_profiles::import::count_phrase(
                    count(Kind::Printer),
                    count(Kind::Filament),
                    count(Kind::Print)
                ),
                source.app_name()
            );
            item(ui, &format!("From {}", source.app_name()), &text, |ui| {
                if widgets::button(ui, "Remove all", true).clicked() {
                    remove = Some(mine.iter().map(|u| (u.kind, u.name.clone())).collect());
                }
            });
        }
        let unused = self.profiles.unused_imported(&self.prefs);
        item(
            ui,
            "Unused",
            &match unused.len() {
                0 => "Every imported profile is in use (selected, in My printers or a default for \
                      new projects)."
                    .to_string(),
                1 => "1 imported profile isn't selected, in My printers or a default for new \
                      projects."
                    .to_string(),
                n => format!(
                    "{n} imported profiles aren't selected, in My printers or a default for new \
                     projects."
                ),
            },
            |ui| {
                if widgets::button(ui, "Remove unused", !unused.is_empty()).clicked() {
                    remove = Some(unused.clone());
                }
            },
        );
        for group in &duplicates {
            let names: Vec<String> = preset_names::labels(group.iter().map(|(_, n)| n.as_str()))
                .iter()
                .map(|l| format!("“{}”", l.text()))
                .collect();
            item(
                ui,
                &format!("Duplicate {} profiles", kind_name(group[0].0)),
                &format!(
                    "{} have identical settings. Merging keeps one and points everything that \
                     used the others at it.",
                    names.join(" and ")
                ),
                |ui| {
                    if widgets::button(ui, "Merge", true).clicked() {
                        merge = Some(group.clone());
                    }
                },
            );
        }
        for s in &stale {
            let app = s.source.app_name();
            let text = if !s.exists {
                format!("No longer in {app}. Mica's copy still works.")
            } else if s.conflicts.is_empty() {
                format!(
                    "Changed in {app} since it was imported. Re-importing keeps your changes \
                     made in Mica on top."
                )
            } else {
                format!(
                    "Changed in {app} and in Mica: {}. Keep your values for these or take \
                     {app}'s; your other changes stay.",
                    conflict_names(&s.conflicts)
                )
            };
            item(ui, preset_names::display_name(&s.name), &text, |ui| {
                if s.conflicts.is_empty() {
                    if widgets::button(ui, "Re-import", s.exists).clicked() {
                        reimport = Some((vec![(s.kind, s.name.clone())], false));
                    }
                } else {
                    ui.horizontal(|ui| {
                        if widgets::button(ui, "Keep mine", true)
                            .on_hover_text(
                                "Re-import with Mica's values for the settings both changed",
                            )
                            .clicked()
                        {
                            reimport = Some((vec![(s.kind, s.name.clone())], false));
                        }
                        if widgets::button(ui, "Take theirs", true)
                            .on_hover_text(format!(
                                "Re-import with {app}'s values for the settings both changed"
                            ))
                            .clicked()
                        {
                            reimport = Some((vec![(s.kind, s.name.clone())], true));
                        }
                    });
                }
            });
        }
        ui.add_space(4.);
        egui::CollapsingHeader::new(format!("All imported profiles ({})", imported.len()))
            .id_salt("imported-list")
            .show(ui, |ui| {
                let labels = preset_names::labels(imported.iter().map(|u| u.name.as_str()));
                for (u, label) in imported.iter().zip(&labels) {
                    ui.horizontal(|ui| {
                        if widgets::icon_button(
                            ui,
                            widgets::Glyph::Close,
                            "Remove (Mica's copy only)",
                            true,
                        )
                        .clicked()
                        {
                            remove = Some(vec![(u.kind, u.name.clone())]);
                        }
                        ui.label(
                            egui::RichText::new(kind_name(u.kind))
                                .font(theme::sans(11.))
                                .color(p.muted),
                        );
                        let r = ui.label(egui::RichText::new(label.text()).font(theme::sans(12.)));
                        if let Some(full) = preset_names::tooltip(&u.name, label) {
                            r.on_hover_text(full);
                        }
                        if let Some(source) = u.source() {
                            source_badge(ui, source, &origin_text(u));
                        }
                        if self.profiles.in_use(u, &self.prefs) {
                            ui.label(
                                egui::RichText::new("in use")
                                    .font(theme::sans(11.))
                                    .color(p.accent_bright),
                            );
                        }
                    });
                }
            });
        ui.add_space(10.);
        let done = if let Some(which) = remove {
            Some((
                "Removed",
                self.profiles.remove_profiles(&which, &mut self.prefs),
            ))
        } else if let Some(group) = merge {
            Some((
                "Merged away",
                self.profiles.merge_duplicates(&group, &mut self.prefs),
            ))
        } else {
            reimport.map(|(which, take_theirs)| {
                (
                    "Re-imported",
                    self.profiles.reimport(&which, take_theirs, &mut self.prefs),
                )
            })
        };
        if let Some((verb, done)) = done {
            self.after_profile_cleanup();
            let n = done.names.len();
            if n > 0 {
                self.notify_info(format!(
                    "{verb} {n} profile{}.",
                    if n == 1 { "" } else { "s" }
                ));
            }
            if !done.errors.is_empty() {
                self.notify_error("Not every profile could be changed", done.errors.join("\n"));
            }
        }
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// One preset of the review list: checkbox, name, what Mica knows about it.
fn review_row(ui: &mut egui::Ui, r: &mut Candidate, label: &Label) {
    let p = theme::of(ui);
    ui.horizontal_wrapped(|ui| {
        let enabled = r.importable();
        let mut on = r.selected && enabled;
        let tip = format!(
            "{}{}{}",
            preset_names::tooltip(&r.found.name, label)
                .map(|full| format!("{full}\n"))
                .unwrap_or_default(),
            r.found.path.display(),
            if r.found.inherits.is_empty() {
                String::new()
            } else {
                format!("\nBuilds on: {}", r.found.inherits)
            }
        );
        if ui
            .add_enabled(enabled, egui::Checkbox::new(&mut on, label.text()))
            .on_hover_text(&tip)
            .on_disabled_hover_text(&tip)
            .changed()
        {
            r.selected = on;
        }
        let (note, colour) = if let Some(problem) = &r.problem {
            (
                "Can't import".to_string(),
                Some((theme::WARNING, problem.clone())),
            )
        } else {
            match &r.status {
                Status::New if r.duplicate_of.is_some() => (
                    format!("Same as {}", r.duplicate_of.as_deref().unwrap_or_default()),
                    Some((
                        p.muted,
                        "Identical settings: importing it too would add a duplicate.".to_string(),
                    )),
                ),
                Status::Imported(n) if *n == r.found.name => ("Already in Mica".to_string(), None),
                Status::Imported(n) => (
                    format!("Already in Mica as “{}”", preset_names::display_name(n)),
                    None,
                ),
                Status::Changed(_) if !r.conflicts.is_empty() => (
                    format!("Changed in {} and in Mica", r.found.source.app_name()),
                    Some((theme::WARNING, conflict_text(r.found.source, &r.conflicts))),
                ),
                Status::Changed(_) => (
                    "Changed since it was imported".to_string(),
                    Some((
                        p.accent_bright,
                        "Checking it re-imports the current file. Your changes made in Mica are \
                         kept on top."
                            .to_string(),
                    )),
                ),
                Status::New if r.for_my_printers => (String::new(), None),
                Status::New => (
                    "Not for your printers".to_string(),
                    Some((
                        p.muted,
                        "It doesn't suit a printer in My printers; import it anyway if you \
                         want it."
                            .to_string(),
                    )),
                ),
            }
        };
        if !note.is_empty() {
            let (c, hover) = colour.unwrap_or((p.muted, String::new()));
            let l = ui.label(egui::RichText::new(note).font(theme::sans(11.)).color(c));
            if !hover.is_empty() {
                l.on_hover_text(hover);
            }
        }
        if !r.conflicts.is_empty() && matches!(r.status, Status::Changed(_)) {
            let theirs = format!("Take {}'s", r.found.source.badge());
            if let Some(take) = keep_or_take(ui, r.take_theirs, &theirs) {
                r.take_theirs = take;
            }
        }
    });
}

/// What a re-import does with the settings changed on both sides, for a tooltip.
fn conflict_text(source: Source, keys: &[String]) -> String {
    format!(
        "Both {} and Mica changed {}.\nKeep mine re-imports it with Mica's values for these; \
         Take theirs uses {}'s. Your other changes made in Mica are kept either way.",
        source.app_name(),
        conflict_names(keys),
        source.app_name()
    )
}

/// "Nozzle temperature, Bed temperature and 2 more": the settings changed on both sides.
fn conflict_names(keys: &[String]) -> String {
    let mut names: Vec<String> = keys
        .iter()
        .take(3)
        .map(|k| crate::settings::full_label(k))
        .collect();
    if keys.len() > 3 {
        names.push(format!("{} more", keys.len() - 3));
    }
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "Keep mine | Take theirs" as two small badges; the new choice when one was clicked.
fn keep_or_take(ui: &mut egui::Ui, take_theirs: bool, theirs: &str) -> Option<bool> {
    let mut out = None;
    if widgets::badge(ui, "Keep mine", !take_theirs).clicked() {
        out = Some(false);
    }
    if widgets::badge(ui, theirs, take_theirs).clicked() {
        out = Some(true);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::date;

    #[test]
    fn dates_are_civil() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_792_454_400), "2026-10-20");
    }
}
