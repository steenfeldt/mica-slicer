// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The About window (Mica › About Mica Slicer on the Mac, Help › About Mica Slicer
//! elsewhere): the app's icon, version, update check and links on the About tab, and on the
//! Credits tab the projects Mica is built on or inspired by (`crate::credits`), grouped and
//! searchable. A quiet "Open-source licenses" link (on both tabs) opens the Licenses view in
//! the same window: every Rust crate with its license and copyright, and the full text of
//! each license.

use super::preferences::{IconTab, icon_tab_row};
use super::widgets;
use crate::app::App;
use crate::credits::{self, Entry, Group};
use crate::theme;
use eframe::egui::{self, Align, Key, Layout, RichText, Sense, Vec2};

const WINDOW_W: f32 = 660.;
const MAX_HEIGHT: f32 = 720.;
pub const WEBSITE: &str = "https://micaslicer.com";
pub const SOURCE: &str = env!("CARGO_PKG_REPOSITORY");
/// The release status shown beside the version.
pub const ALPHA: &str = "Alpha (pre-release)";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AboutTab {
    #[default]
    About,
    Credits,
}

impl AboutTab {
    pub const ALL: [Self; 2] = [Self::About, Self::Credits];

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim() {
            "about" => Some(Self::About),
            "credits" => Some(Self::Credits),
            _ => None,
        }
    }
}

/// The open About window.
#[derive(Clone, Debug, Default)]
pub struct About {
    pub tab: AboutTab,
    pub search: String,
    /// The Licenses view (every Rust crate) is shown instead of the tab.
    pub licenses: bool,
    /// The search of the Licenses view.
    pub licenses_search: String,
    /// The license whose full text is shown (an SPDX id), over the tab or the Licenses view.
    pub license: Option<String>,
}

impl About {
    /// Back one level: the license text, then the Licenses view. False when at the top.
    fn back(&mut self) -> bool {
        if self.license.take().is_some() {
            return true;
        }
        std::mem::take(&mut self.licenses)
    }
}

/// What a click in the window asks for, carried out after drawing.
enum Click {
    Url(String),
    License(String),
    Tab(AboutTab),
    /// The Licenses view, with this search typed.
    Licenses(String),
    CheckForUpdates,
}

impl App {
    pub(crate) fn open_about(&mut self, tab: AboutTab) {
        self.dialogs.about = Some(About {
            tab,
            ..About::default()
        });
    }

    pub(crate) fn about_window(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.dialogs.about.take() else {
            return;
        };
        let mut open = true;
        let mut done = false;
        let escape = !widgets::field_editing(ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(Key::Escape));
        if escape && !state.back() {
            done = true;
        }
        let brick_layers = self.prefs.features.brick_layers_enabled();
        let updates_busy = self.updater.working();
        let mut clicks: Vec<Click> = vec![];
        let height = (ctx.content_rect().height() - 120.).clamp(360., MAX_HEIGHT);
        egui::Window::new("About Mica Slicer")
            .id(egui::Id::unique("mica-about"))
            .open(&mut open)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(-WINDOW_W / 2., 60.))
            .fixed_size([WINDOW_W, height])
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                let tabs = [
                    IconTab {
                        key: "about",
                        label: "About",
                        hint: "Version, updates and license",
                    },
                    IconTab {
                        key: "credits",
                        label: "Credits",
                        hint: "The projects and people Mica is built on",
                    },
                ];
                let at = AboutTab::ALL
                    .iter()
                    .position(|&t| t == state.tab)
                    .unwrap_or(0);
                if let Some(k) = icon_tab_row(ui, "about-tab", &tabs, at) {
                    state.tab = AboutTab::ALL[k];
                    state.license = None;
                    state.licenses = false;
                }
                ui.separator();
                let footer = 40.;
                let body_h = (ui.available_height() - footer).max(80.);
                if let Some(id) = state.license.clone() {
                    license_view(ui, &id, body_h, &mut state, &mut clicks);
                } else if state.licenses {
                    licenses_view(ui, body_h, &mut state, &mut clicks);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("mica-about-body", state.tab == AboutTab::Credits))
                        .max_height(body_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width() - 8.);
                            match state.tab {
                                AboutTab::About => about_tab(ui, updates_busy, &mut clicks),
                                AboutTab::Credits => {
                                    credits_tab(ui, &mut state, brick_layers, &mut clicks)
                                }
                            }
                        });
                }
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("© 2026 Rene Steenfeldt · GNU AGPL-3.0")
                            .font(theme::sans(12.))
                            .color(p.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "Done", true).clicked() {
                            done = true;
                        }
                    });
                });
            });
        for c in clicks {
            match c {
                Click::Url(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                Click::License(id) => state.license = Some(id),
                Click::Tab(t) => {
                    state.tab = t;
                    state.license = None;
                    state.licenses = false;
                }
                Click::Licenses(q) => {
                    state.licenses = true;
                    state.licenses_search = q;
                    state.license = None;
                }
                Click::CheckForUpdates => {
                    self.run_menu(crate::menu::Action::CheckForUpdates, ctx);
                }
            }
        }
        if open && !done {
            self.dialogs.about = Some(state);
        }
    }
}

/// The app's icon as a texture, decoded once.
pub(crate) fn icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("mica-about-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory(include_bytes!("../../assets/icon/mica-256.png"))
        .ok()?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let texture = ctx.load_texture(
        "mica-about-icon",
        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

fn about_tab(ui: &mut egui::Ui, updates_busy: bool, clicks: &mut Vec<Click>) {
    let p = theme::of(ui);
    ui.vertical_centered(|ui| {
        ui.add_space(14.);
        if let Some(t) = icon(ui.ctx()) {
            ui.add(egui::Image::new(&t).fit_to_exact_size(Vec2::splat(96.)));
        }
        ui.add_space(10.);
        ui.label(
            RichText::new("Mica Slicer")
                .font(theme::semibold(24.))
                .color(p.text),
        );
        // The version with a quiet Alpha badge beside it, centered together.
        let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
        let painter = ui.painter().clone();
        let width = |text: &str, font| {
            painter
                .layout_no_wrap(text.to_string(), font, p.text)
                .size()
                .x
        };
        let row = width(&version, theme::mono(12.5)) + 8. + width(ALPHA, theme::sans(10.5)) + 12.;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - row) / 2.).max(0.));
            ui.spacing_mut().item_spacing.x = 8.;
            ui.label(
                RichText::new(&version)
                    .font(theme::mono(12.5))
                    .color(p.muted),
            );
            widgets::status_badge(ui, ALPHA);
        });
        ui.add_space(6.);
        ui.label(
            RichText::new("A fast, native 3D-printing slicer, written in Rust.")
                .font(theme::sans(13.5))
                .color(p.label),
        );
        ui.add_space(8.);
        // "Alpha (pre-release) · Provided as is, without warranty · License", centered.
        let note = format!("{ALPHA} · Provided as is, without warranty · ");
        let row = width(&note, theme::sans(12.)) + width("License", theme::sans(12.));
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - row) / 2.).max(0.));
            ui.spacing_mut().item_spacing.x = 0.;
            ui.label(RichText::new(&note).font(theme::sans(12.)).color(p.muted));
            if widgets::link(ui, "License")
                .on_hover_text("GNU AGPL-3.0, sections 15 and 16: no warranty, limited liability")
                .clicked()
            {
                clicks.push(Click::License("AGPL-3.0".into()));
            }
        });
        ui.add_space(14.);
        let label = if updates_busy {
            "Checking for updates…"
        } else {
            "Check for updates"
        };
        if widgets::primary_button(ui, label, !updates_busy).clicked() {
            clicks.push(Click::CheckForUpdates);
        }
        ui.add_space(14.);
        // Website · License · Source code, centered.
        let links: [(&str, Click); 3] = [
            ("micaslicer.com", Click::Url(WEBSITE.into())),
            ("License: AGPL-3.0", Click::License("AGPL-3.0".into())),
            ("Source code", Click::Url(SOURCE.into())),
        ];
        let widths: f32 = links
            .iter()
            .map(|(t, _)| {
                ui.painter()
                    .layout_no_wrap(t.to_string(), theme::sans(12.), p.text)
                    .size()
                    .x
            })
            .sum::<f32>()
            + 2. * 28.;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - widths) / 2.).max(0.));
            ui.spacing_mut().item_spacing.x = 0.;
            for (k, (text, click)) in links.into_iter().enumerate() {
                if k > 0 {
                    ui.add_sized(
                        [28., 16.],
                        egui::Label::new(RichText::new("·").font(theme::sans(12.)).color(p.muted)),
                    );
                }
                let hint = match &click {
                    Click::Url(u) => u.clone(),
                    _ => "Show the full license text".into(),
                };
                if widgets::link(ui, text).on_hover_text(hint).clicked() {
                    clicks.push(click);
                }
            }
        });
        ui.add_space(18.);
        ui.set_max_width(500.);
        ui.label(
            RichText::new(
                "Mica Slicer is free software: you can share and change it under the terms of \
                 the GNU Affero General Public License, version 3. It comes with no warranty.",
            )
            .font(theme::sans(12.5))
            .color(p.muted),
        );
        ui.add_space(10.);
        ui.label(
            RichText::new(
                "Built on PrusaSlicer, OrcaSlicer and Slic3r, and on the work of many more \
                 people.",
            )
            .font(theme::sans(12.5))
            .color(p.muted),
        );
        if widgets::link(ui, "See the credits").clicked() {
            clicks.push(Click::Tab(AboutTab::Credits));
        }
        ui.add_space(22.);
        licenses_link(ui, "Open-source licenses", String::new(), clicks);
    });
}

/// The small, quiet link to the Licenses view.
fn licenses_link(ui: &mut egui::Ui, text: &str, search: String, clicks: &mut Vec<Click>) {
    let p = theme::of(ui);
    let r = ui
        .add(
            egui::Label::new(RichText::new(text).font(theme::sans(11.5)).color(p.muted))
                .sense(Sense::click())
                .selectable(false),
        )
        .on_hover_text("Every Rust crate in Mica, with its license");
    if r.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        ui.painter().hline(
            r.rect.x_range(),
            r.rect.bottom() - 1.,
            egui::Stroke::new(1., p.muted),
        );
    }
    if r.clicked() {
        clicks.push(Click::Licenses(search));
    }
}

fn credits_tab(ui: &mut egui::Ui, state: &mut About, brick_layers: bool, clicks: &mut Vec<Click>) {
    let p = theme::of(ui);
    let all = credits::entries(brick_layers);
    let q = state.search.trim().to_lowercase();
    let shown: Vec<&Entry> = all.iter().filter(|e| e.matches(&q)).collect();
    let n = shown.len();
    let annotation = if q.is_empty() {
        String::new()
    } else {
        format!("{n} match{}", if n == 1 { "" } else { "es" })
    };
    ui.add_space(4.);
    ui.label(
        RichText::new(
            "Mica credits the originals, also when it only adopted an idea. Thank you to \
             everyone below.",
        )
        .font(theme::sans(12.5))
        .color(p.muted),
    );
    ui.add_space(6.);
    widgets::search_field(
        ui,
        egui::Id::unique("mica-credits-search"),
        &mut state.search,
        "Search credits: a name, a license or what it does",
        &annotation,
    );
    for g in Group::ALL {
        let entries: Vec<&&Entry> = shown.iter().filter(|e| e.group == g).collect();
        if entries.is_empty() {
            continue;
        }
        widgets::group_title(ui, g.title());
        ui.label(
            RichText::new(g.intro())
                .font(theme::sans(12.))
                .color(p.muted),
        );
        ui.add_space(4.);
        for e in entries {
            entry_row(ui, e, clicks);
        }
    }
    // The full crate list lives in the Licenses view; a search that finds crates there says so.
    let crate_hits = if q.is_empty() {
        0
    } else {
        credits::crates().iter().filter(|c| c.matches(&q)).count()
    };
    if n == 0 && !q.is_empty() {
        ui.add_space(16.);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(format!("Nothing matches “{}”", state.search.trim()))
                    .font(theme::sans(13.))
                    .color(p.muted),
            );
        });
    }
    ui.add_space(12.);
    ui.vertical_centered(|ui| {
        if crate_hits > 0 {
            licenses_link(
                ui,
                &format!(
                    "{crate_hits} Rust crate{} in Open-source licenses",
                    if crate_hits == 1 { "" } else { "s" }
                ),
                state.search.trim().to_string(),
                clicks,
            );
        } else {
            licenses_link(
                ui,
                "Open-source licenses: every Rust crate in Mica",
                String::new(),
                clicks,
            );
        }
    });
    ui.add_space(8.);
}

/// One credit: the name (a link), its pin and license, who made it, what Mica took.
fn entry_row(ui: &mut egui::Ui, e: &Entry, clicks: &mut Vec<Click>) {
    let p = theme::of(ui);
    egui::Frame::new()
        .fill(p.raised)
        .stroke(egui::Stroke::new(1., p.border_soft))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let name = ui
                    .add(
                        egui::Label::new(
                            RichText::new(&e.name)
                                .font(theme::semibold(13.5))
                                .color(p.text),
                        )
                        .sense(Sense::click())
                        .selectable(false),
                    )
                    .on_hover_text(&e.link);
                if name.hovered() {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }
                if name.clicked() {
                    clicks.push(Click::Url(e.link.clone()));
                }
                if let Some(pin) = e.pin() {
                    ui.label(RichText::new(pin).font(theme::mono(11.)).color(p.muted));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    license_chips(ui, &e.license, clicks);
                });
            });
            ui.label(RichText::new(&e.by).font(theme::sans(12.)).color(p.muted));
            ui.add_space(2.);
            ui.label(
                RichText::new(&e.what)
                    .font(theme::sans(12.5))
                    .color(p.label),
            );
        });
    ui.add_space(6.);
}

/// A small chip per license of an expression (right to left in a right-to-left layout):
/// a click shows the bundled text, or opens the license's page.
fn license_chips(ui: &mut egui::Ui, expr: &str, clicks: &mut Vec<Click>) {
    let p = theme::of(ui);
    let mut ids = credits::license_ids(expr);
    if ui.layout().prefer_right_to_left() {
        ids.reverse();
    }
    ui.spacing_mut().item_spacing.x = 4.;
    for id in ids {
        let bundled = credits::license_text(&id).is_some();
        let r = ui
            .add(
                egui::Button::new(
                    RichText::new(&id)
                        .font(theme::mono(10.5))
                        .color(p.accent_hi),
                )
                .fill(p.accent_surface)
                .stroke(egui::Stroke::NONE)
                .corner_radius(9.)
                .min_size(Vec2::new(0., 18.)),
            )
            .on_hover_text(if bundled {
                "Show the full license text"
            } else {
                "Open the license in your browser"
            });
        if r.clicked() {
            clicks.push(if bundled {
                Click::License(id)
            } else {
                Click::Url(credits::license_url(&id))
            });
        }
    }
}

/// The Licenses view: every Rust crate the app and the command line use (generated from
/// Cargo's dependency data), searchable, with its version, license and copyright holders.
fn licenses_view(ui: &mut egui::Ui, height: f32, state: &mut About, clicks: &mut Vec<Click>) {
    let p = theme::of(ui);
    let top = ui.cursor().top();
    let crates = credits::crates();
    let q = state.licenses_search.trim().to_lowercase();
    let hits: Vec<&credits::Crate> = crates.iter().filter(|c| c.matches(&q)).collect();
    ui.horizontal(|ui| {
        if widgets::button(ui, "‹ Back", true).clicked() {
            state.licenses = false;
        }
        ui.label(
            RichText::new("Open-source licenses")
                .font(theme::semibold(14.))
                .color(p.text),
        );
    });
    ui.add_space(4.);
    ui.label(
        RichText::new(format!(
            "The {} Rust crates Mica's app and command line use on any platform, generated \
             from Cargo's dependency data. Click a license to read it.",
            crates.len()
        ))
        .font(theme::sans(12.))
        .color(p.muted),
    );
    ui.add_space(6.);
    let annotation = if q.is_empty() {
        String::new()
    } else {
        format!("{} of {}", hits.len(), crates.len())
    };
    widgets::search_field(
        ui,
        egui::Id::unique("mica-licenses-search"),
        &mut state.licenses_search,
        "Search crates: a name, a license or a copyright holder",
        &annotation,
    );
    ui.add_space(4.);
    let used = ui.cursor().top() - top;
    egui::ScrollArea::vertical()
        .id_salt("mica-about-licenses")
        .max_height((height - used).max(80.))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 8.);
            for c in &hits {
                ui.horizontal(|ui| {
                    let name = ui
                        .add(
                            egui::Label::new(
                                RichText::new(c.name)
                                    .font(theme::medium(12.5))
                                    .color(p.text),
                            )
                            .sense(Sense::click())
                            .selectable(false),
                        )
                        .on_hover_text(c.repository);
                    if name.hovered() && !c.repository.is_empty() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if name.clicked() && !c.repository.is_empty() {
                        clicks.push(Click::Url(c.repository.into()));
                    }
                    ui.label(
                        RichText::new(c.version)
                            .font(theme::mono(11.))
                            .color(p.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        license_chips(ui, c.license, clicks);
                    });
                });
                ui.label(
                    RichText::new(c.holders())
                        .font(theme::sans(11.))
                        .color(p.muted),
                );
                ui.add_space(3.);
            }
            if hits.is_empty() {
                ui.add_space(16.);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "No crate matches “{}”",
                            state.licenses_search.trim()
                        ))
                        .font(theme::sans(13.))
                        .color(p.muted),
                    );
                });
            }
        });
}

/// The full text of one license, with a way back.
fn license_view(
    ui: &mut egui::Ui,
    id: &str,
    height: f32,
    state: &mut About,
    clicks: &mut Vec<Click>,
) {
    let p = theme::of(ui);
    ui.horizontal(|ui| {
        if widgets::button(ui, "‹ Back", true).clicked() {
            state.license = None;
        }
        ui.label(RichText::new(id).font(theme::semibold(14.)).color(p.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::link(ui, "Open in browser").clicked() {
                clicks.push(Click::Url(credits::license_url(id)));
            }
        });
    });
    ui.add_space(6.);
    let text = credits::license_text(id).unwrap_or("This license's text is not bundled.");
    egui::ScrollArea::vertical()
        .id_salt(("mica-about-license", id.to_string()))
        .max_height(height - 40.)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 8.);
            ui.add(
                egui::Label::new(RichText::new(text).font(theme::mono(11.5)).color(p.label)).wrap(),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Options};
    use crate::menu::Action;
    use eframe::egui::Pos2;
    use mica_profiles::user::Store;

    fn app(ctx: &egui::Context, name: &str) -> App {
        let dir = std::env::temp_dir().join(format!("mica-about-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        App::create(
            ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir.join("store"))),
                files: vec![],
                headless: true,
            },
        )
    }

    /// One whole-window frame; the texts drawn.
    fn frame(ctx: &egui::Context, app: &mut App) -> Vec<String> {
        fn texts(shape: egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.show(ui));
        output.textures_delta.clear();
        let mut out = vec![];
        for s in output.shapes {
            texts(s.shape, &mut out);
        }
        out
    }

    #[test]
    fn about_shows_version_and_links() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, "tab");
        app.run_menu(Action::About, &ctx);
        assert_eq!(app.dialogs.about.as_ref().unwrap().tab, AboutTab::About);
        frame(&ctx, &mut app);
        let t = frame(&ctx, &mut app);
        let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
        for want in [
            "About Mica Slicer",
            version.as_str(),
            "Check for updates",
            "micaslicer.com",
            "License: AGPL-3.0",
            "Source code",
            "Credits",
        ] {
            assert!(t.iter().any(|s| s == want), "{want} missing");
        }
        assert!(SOURCE.starts_with("https://"));
    }

    #[test]
    fn credits_hide_brick_layers_until_unlocked() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, "credits");
        app.open_about(AboutTab::Credits);
        let search = |app: &mut App, q: &str| {
            app.dialogs.about.as_mut().unwrap().search = q.into();
            frame(&ctx, app);
            frame(&ctx, app)
        };
        let t = search(&mut app, "");
        assert!(t.iter().any(|s| s == "PrusaSlicer"));
        for (q, want) in [
            ("cnc kitchen", "BumpMesh"),
            ("steve schoger", "Hero Patterns"),
            ("ayufan", "camera-streamer"),
            ("gfx-rs", "wgpu"),
        ] {
            let t = search(&mut app, q);
            assert!(t.iter().any(|s| s == want), "{want} missing for {q}");
        }
        // Credits lists only the key libraries, never the whole crate list.
        let t = search(&mut app, "steve schoger");
        assert!(
            t.iter()
                .any(|s| s == "Open-source licenses: every Rust crate in Mica")
        );
        // A crate that is not a credit points to the Licenses view instead of being listed.
        let t = search(&mut app, "smallvec");
        assert!(t.iter().any(|s| s == "Nothing matches “smallvec”"));
        assert!(
            t.iter()
                .any(|s| s == "1 Rust crate in Open-source licenses")
        );
        let t = search(&mut app, "brick");
        assert!(t.iter().any(|s| s == "Nothing matches “brick”"));
        assert!(
            !t.iter()
                .any(|s| s == "Brick Layers" || s.contains("GeekDetour"))
        );
        app.prefs.features.brick_layers = true;
        let t = search(&mut app, "brick");
        assert!(t.iter().any(|s| s == "Brick Layers"));
        // A license opens in place; Escape goes back, then closes the window.
        app.dialogs.about.as_mut().unwrap().license = Some("MIT".into());
        let t = frame(&ctx, &mut app);
        assert!(t.iter().any(|s| s.contains("Permission is hereby granted")));
        assert!(AboutTab::from_key("credits") == Some(AboutTab::Credits));
    }

    #[test]
    fn licenses_view_lists_every_crate() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, "licenses");
        app.open_about(AboutTab::About);
        frame(&ctx, &mut app);
        let t = frame(&ctx, &mut app);
        assert!(
            t.iter().any(|s| s == "Open-source licenses"),
            "the About link"
        );
        // The Licenses view replaces the tab, with a way back.
        app.dialogs.about.as_mut().unwrap().licenses = true;
        frame(&ctx, &mut app);
        let t = frame(&ctx, &mut app);
        let n = credits::crates().len();
        for want in ["‹ Back", "Open-source licenses", "About", "Credits"] {
            assert!(t.iter().any(|s| s == want), "{want} missing");
        }
        assert!(
            t.iter()
                .any(|s| s.contains(&format!("The {n} Rust crates")))
        );
        assert!(!t.iter().any(|s| s == "Check for updates"));
        // Searching finds any crate, with its version, license and copyright.
        app.dialogs.about.as_mut().unwrap().licenses_search = "bytemuck".into();
        frame(&ctx, &mut app);
        let t = frame(&ctx, &mut app);
        assert!(t.iter().any(|s| s == "bytemuck"));
        assert!(t.iter().any(|s| s == "Zlib") && t.iter().any(|s| s == "MIT"));
        assert!(t.iter().any(|s| s.contains("Lokathor")));
        assert!(t.iter().any(|s| s.contains(&format!("2 of {n}"))), "{t:?}");
        // A license text opens over the view; Back steps out one level at a time.
        let about = app.dialogs.about.as_mut().unwrap();
        about.license = Some("Zlib".into());
        assert!(about.back() && about.licenses && about.license.is_none());
        assert!(about.back() && !about.licenses);
        assert!(!about.back());
        let t = frame(&ctx, &mut app);
        assert!(t.iter().any(|s| s == "Check for updates"));
    }
}
