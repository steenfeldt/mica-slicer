// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmoBrimEars.cpp :: GLGizmoBrimEars::on_render_input_window
// Changes: An egui card with plain-language labels: ear size, the auto-placement angle and detection radius, Auto-place, Remove all, the loose-ear count and the "not painted" warning with its one-click fix.

//! The brim ears card and the App side of placing and removing ears.

use super::layer_edit::{note, number_row};
use super::widgets::{self, Glyph, NumberSpec};
use crate::app::App;
use crate::brim_ears as be;
use crate::theme;
use eframe::egui::{self, Vec2};
use glam::Vec2 as GVec2;

const CARD_W: f32 = 300.;

impl App {
    /// Places an ear on the selected object at `at` (world XY): one undo step.
    pub(crate) fn add_brim_ear(&mut self, at: GVec2) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let d = self.brim_ear.diameter_for(self.effective_config());
        let mut o = self.plates.current().objects[i].clone();
        if be::add(&mut o, at, d) {
            self.record("Add brim ear");
            self.plates.current_mut().objects[i] = o;
            self.sync_instances(i);
            self.changed();
        }
    }

    /// Removes ear `ear` of the selected object: one undo step.
    pub(crate) fn remove_brim_ear(&mut self, ear: usize) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() || ear >= self.plates.current().objects[i].brim_ears.len() {
            return;
        }
        self.record("Remove brim ear");
        self.plates.current_mut().objects[i].brim_ears.remove(ear);
        self.sync_instances(i);
        self.changed();
    }

    fn edit_ears(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut crate::scene::Object, &be::Settings, f32),
    ) {
        let Some(i) = self.selected else { return };
        if self.plate_locked() {
            return;
        }
        let d = self.brim_ear.diameter_for(self.effective_config());
        let mut o = self.plates.current().objects[i].clone();
        f(&mut o, &self.brim_ear, d);
        if o != self.plates.current().objects[i] {
            self.record(label);
            self.plates.current_mut().objects[i] = o;
            self.sync_instances(i);
            self.changed();
        }
    }

    /// "Auto-place" on the selected object: one undo step.
    pub(crate) fn auto_place_ears(&mut self) {
        self.edit_ears("Auto-place brim ears", |o, s, d| {
            be::auto_place(o, s, d);
        });
    }

    /// The brim ears card for the selected object.
    pub(crate) fn ears_window(&mut self, ctx: &egui::Context) {
        let Some(i) = self.selected else { return };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let config = self.effective_config();
        let orca = config.is_some_and(be::orca);
        let painted = be::painted(config, o);
        let name = std::path::Path::new(&o.name)
            .file_stem()
            .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned());
        let count = o.brim_ears.len();
        let loose = self.ears_loose.as_ref().map_or(0, |(_, l)| l.len());
        let mut settings = self.brim_ear.clone();
        let diameter = settings.diameter_for(config);
        let (mut auto, mut clear, mut use_painted, mut leave) = (false, false, false, false);
        let viewport = self.viewport_rect;
        egui::Area::new(egui::Id::unique("brim-ears-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W, 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(widgets::spaced_upper(&format!("{name} · Brim ears")))
                                    .font(theme::semibold(11.))
                                    .color(p.muted),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if widgets::icon_button_sized(ui, Glyph::Close, "Leave the tool (Esc)", true, 22.)
                                    .clicked()
                                {
                                    leave = true;
                                }
                            });
                        });
                        ui.add_space(4.);
                        if !orca {
                            note(
                                ui,
                                "This printer's profile has no brim ears (PrusaSlicer has none): ears are kept in the project but do not print.",
                                theme::WARNING,
                            );
                        } else if !painted {
                            note(
                                ui,
                                "Ears only print when the brim is set to “Ears where I place them”.",
                                theme::WARNING,
                            );
                            if widgets::styled_button(ui, "Use placed ears for this object", true, true, 26.)
                                .clicked()
                            {
                                use_painted = true;
                            }
                            ui.add_space(4.);
                        }
                        if let Some(v) = number_row(
                            ui,
                            "Ear size",
                            "ears-diameter",
                            f64::from(diameter),
                            &NumberSpec {
                                unit: "mm",
                                decimals: 1,
                                min: f64::from(be::DIAMETER_MIN),
                                max: f64::from(be::DIAMETER_MAX),
                                speed: 0.1,
                            },
                            "Diameter of new ears · Alt-scroll in the view changes it too",
                            Some((f64::from(be::DIAMETER_MIN), f64::from(be::DIAMETER_MAX))),
                        ) {
                            settings.set_diameter(v as f32);
                        }
                        ui.add_space(6.);
                        widgets::group_title(ui, "Place automatically");
                        if let Some(v) = number_row(
                            ui,
                            "Sharper than",
                            "ears-angle",
                            f64::from(settings.max_angle),
                            &NumberSpec {
                                unit: "°",
                                decimals: 0,
                                min: 0.,
                                max: 180.,
                                speed: 0.5,
                            },
                            "Corners whose inside angle is below this get an ear (Orca's “Max angle”)",
                            Some((0., 180.)),
                        ) {
                            settings.max_angle = v.clamp(0., 180.) as f32;
                        }
                        if let Some(v) = number_row(
                            ui,
                            "Ignore wiggles",
                            "ears-detection",
                            f64::from(settings.detection),
                            &NumberSpec {
                                unit: "mm",
                                decimals: 1,
                                min: 0.,
                                max: 20.,
                                speed: 0.05,
                            },
                            "The outline is simplified by this much before corners are found (Orca's “Detection radius”)",
                            Some((0., 10.)),
                        ) {
                            settings.detection = v.max(0.) as f32;
                        }
                        ui.horizontal(|ui| {
                            if widgets::styled_button(ui, "Auto-place", true, false, 26.)
                                .on_hover_text("An ear on every sharp corner of the first layer")
                                .clicked()
                            {
                                auto = true;
                            }
                            if widgets::styled_button(ui, "Remove all", count > 0, false, 26.).clicked() {
                                clear = true;
                            }
                        });
                        ui.add_space(4.);
                        let status = match (count, loose) {
                            (0, _) => "No ears yet · click the model to place one".to_string(),
                            (n, 0) => format!("{n} ear{}", if n == 1 { "" } else { "s" }),
                            (n, k) => format!(
                                "{n} ear{} · {k} not touching the model (orange) would print loose",
                                if n == 1 { "" } else { "s" }
                            ),
                        };
                        note(ui, &status, if loose > 0 { theme::WARNING } else { p.muted_alt });
                        note(
                            ui,
                            "Click the model to place an ear · click an ear to remove it",
                            p.muted,
                        );
                    });
            });
        if settings != self.brim_ear {
            self.brim_ear = settings;
        }
        if use_painted {
            self.record("Use placed brim ears");
            self.plates.current_mut().objects[i]
                .overrides
                .insert("brim_type".into(), "painted".into());
            self.sync_instances(i);
            self.changed();
        }
        if auto {
            self.auto_place_ears();
        }
        if clear {
            self.edit_ears("Remove brim ears", |o, _, _| o.brim_ears.clear());
        }
        if leave {
            self.viewport.object_tool = None;
            self.object_tool_changed(None);
        }
    }
}
