// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Calibrate menu's cards: the test card (explanation, pictogram, ranges with defaults,
//! live estimate, *Create plate and slice*) and the calibration plate's card in the viewport
//! (what to look for, *Enter result*, the computed value, *Save to filament profile*). The
//! pictograms are painted; the model lives in `crate::calibration`.

use super::{
    profiles::PresetKind,
    widgets::{self, CONTROL_H, CONTROL_W, NumberSpec},
};
use crate::app::App;
use crate::calibration::{self, Card, Outcome, PlateJob, Test, trim_num};
use crate::plates::Plate;
use crate::preset_names;
use crate::scene::Object;
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use mica_core::config::Config;

const CARD_W: f32 = 340.;
const LABEL_W: f32 = 168.;

impl App {
    /// Opens the card of `test` with defaults from the selected filament (the last chosen
    /// tool is kept when switching tests).
    pub(crate) fn open_calibration(&mut self, test: Test) {
        let Some(config) = self.effective_config().cloned() else {
            self.status = "Select a printer and filament first".into();
            return;
        };
        let tool = self
            .dialogs
            .calibration
            .as_ref()
            .map_or(0, |c| c.tool)
            .min(self.tool_count().saturating_sub(1));
        self.dialogs.calibration = Some(Card::new(test, &config, tool));
    }

    /// "Create plate and slice": the generated test objects on a new plate named after the
    /// test and the filament, locked, with the engine's overrides kept on the plate's job (the
    /// user's settings stay untouched), then sliced. Returns the plate index.
    pub(crate) fn create_calibration_plate(
        &mut self,
        ctx: &egui::Context,
    ) -> Result<usize, String> {
        let card = self
            .dialogs
            .calibration
            .clone()
            .ok_or("No calibration card is open")?;
        let base = self.config.clone()?;
        let mut params = card.params.clone();
        params.tool = card.tool;
        let generated = mica_core::calib::generate(&params, &base)?;
        let filament = self
            .profiles
            .filaments
            .get(card.tool)
            .cloned()
            .ok_or("No filament selected for that tool")?;
        let head = preset_names::display_name(&filament).to_string();
        let name = format!("{} · {head}", card.test.short());
        let profile_name = if self
            .profiles
            .is_user_preset(PresetKind::Filament, &filament)
        {
            filament.clone()
        } else {
            format!("{head} tuned")
        };
        self.record("Create calibration plate");
        let objects: Vec<Object> = generated
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| {
                Object::new(mica_core::input::InputObject::plain(
                    o.name.clone(),
                    i,
                    0,
                    o.mesh.clone(),
                    mica_core::print_job::ToolAssignments([Some(card.tool); 5]),
                    base.clone(),
                ))
            })
            .collect();
        let mut plate = Plate::new(name.clone());
        plate.objects = objects;
        // The layout is the test's; moving its objects would change nothing.
        plate.locked = true;
        plate.calibration = Some(PlateJob {
            test: card.test,
            params,
            tool: card.tool,
            filament,
            samples: generated.samples.clone(),
            overrides: generated.overrides.clone(),
            input: String::new(),
            profile_name,
            first_layer_too: true,
            saved: None,
        });
        let index = if self.plates.len() == 1 && self.plates.is_blank() {
            self.plates.plates[0] = plate;
            0
        } else {
            self.plates.plates.push(plate);
            self.plates.len() - 1
        };
        self.plates.current = index;
        self.select(None, false);
        self.dialogs.calibration = None;
        self.preview = false;
        self.changed();
        self.fit_current_plate(false);
        self.slice_plate(index, false, ctx);
        self.status = format!("Calibration plate “{name}” created; slicing…");
        Ok(index)
    }

    /// The configuration the current calibration plate is generated on: the plate's own (the
    /// test prints on its tool with that tool's entries).
    fn calibration_plate_config(&self, plate: usize) -> Result<Config, String> {
        if self.plates.plate(plate).calibration.is_none() {
            return Err("Not a calibration plate".into());
        }
        self.plate_config(plate)
    }

    /// The reading of the current calibration plate's typed result.
    pub(crate) fn calibration_outcome(&self) -> Option<Result<Outcome, String>> {
        let plate = self.plates.current;
        let job = self.plates.plate(plate).calibration.as_ref()?;
        let config = match self.calibration_plate_config(plate) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        Some(calibration::evaluate(job, &config))
    }

    /// "Save to filament profile": the result's setting into a user filament profile
    /// (inherits the system preset, differences only), which is then selected for the tool.
    /// The system preset is never written.
    pub(crate) fn save_calibration_result(&mut self) -> Result<(), String> {
        let plate = self.plates.current;
        let job = self
            .plates
            .plate(plate)
            .calibration
            .clone()
            .ok_or("Not a calibration plate")?;
        let outcome = self
            .calibration_outcome()
            .ok_or("Not a calibration plate")??;
        let name = job.profile_name.trim().to_string();
        if name.is_empty() {
            return Err("Give the profile a name".into());
        }
        let changes: std::collections::BTreeMap<String, String> =
            outcome.changes.iter().cloned().collect();
        let saved = self.profiles.save_user_profile(
            PresetKind::Filament,
            &name,
            &job.filament,
            &changes,
        )?;
        if let Some(f) = self.profiles.filaments.get_mut(job.tool) {
            *f = saved.name.clone();
        }
        // Unsaved sidebar edits of the same keys would hide the saved value.
        if let Some(o) = self.overlays.filaments.get_mut(job.tool) {
            for k in changes.keys() {
                o.remove(k);
            }
        }
        self.profiles.refresh();
        self.settings_changed();
        let text = format!(
            "{} saved to “{}”, now selected for tool {}",
            outcome.headline,
            saved.name,
            job.tool + 1
        );
        if let Some(j) = &mut self.plates.plates[plate].calibration {
            j.saved = Some(text.clone());
            j.profile_name = saved.name;
        }
        self.status = text;
        Ok(())
    }

    /// The test card: a small non-modal window.
    pub(crate) fn calibration_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut card) = self.dialogs.calibration.clone() else {
            return;
        };
        let Some(config) = self.effective_config().cloned() else {
            self.dialogs.calibration = None;
            return;
        };
        let tools = self.tool_count();
        let filaments: Vec<String> = self
            .profiles
            .filaments
            .iter()
            .map(|f| preset_names::display_name(f).to_string())
            .collect();
        let busy = self.busy();
        let mut open = true;
        let mut create = false;
        let mut cancel = false;
        let mut switch_test: Option<Test> = None;
        let viewport = self.viewport_rect;
        egui::Window::new(card.test.title())
            .id(egui::Id::unique("calibration-card"))
            .open(&mut open)
            .default_pos(viewport.left_top() + egui::vec2(60., 70.))
            .default_width(420.)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                ui.set_width(420.);
                ui.spacing_mut().item_spacing = Vec2::new(8., 6.);
                ui.add_space(2.);
                ui.label(
                    egui::RichText::new(card.test.explanation())
                        .font(theme::sans(12.5))
                        .color(p.label),
                );
                ui.add_space(4.);
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 96.), Sense::hover());
                pictogram(ui, rect, card.test, None);
                ui.add_space(6.);
                // Filament / tool on multi-tool printers.
                if tools > 1 {
                    let mut new_tool = None;
                    row(ui, "Filament to calibrate", |ui, control| {
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(control));
                        let shown = format!(
                            "Tool {} · {}",
                            card.tool + 1,
                            filaments.get(card.tool).cloned().unwrap_or_default()
                        );
                        egui::ComboBox::from_id_salt("calibration-tool")
                            .width(control.width())
                            .selected_text(shown)
                            .show_ui(&mut child, |ui| {
                                for t in 0..tools {
                                    let label = format!(
                                        "Tool {} · {}",
                                        t + 1,
                                        filaments.get(t).cloned().unwrap_or_default()
                                    );
                                    if ui.selectable_label(t == card.tool, label).clicked() {
                                        new_tool = Some(t);
                                    }
                                }
                            });
                    });
                    if let Some(t) = new_tool {
                        card.set_tool(t, &config);
                    }
                    ui.label(
                        egui::RichText::new(
                            "The test prints on this tool: it alone is picked up and heated, the other tools stay parked and cold.",
                        )
                        .font(theme::sans(11.))
                        .color(p.muted),
                    );
                }
                let test = card.test;
                let (unit, per_mm) = test.range_unit();
                let decimals = test.decimals();
                if test.is_flow() {
                    row(ui, "Pass", |ui, control| {
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(control)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        child.spacing_mut().item_spacing.x = 6.;
                        for (t, label) in [(Test::FlowPass1, "1 · coarse"), (Test::FlowPass2, "2 · fine")]
                        {
                            if widgets::chip(&mut child, None, label, test == t, true).clicked()
                                && test != t
                            {
                                switch_test = Some(t);
                            }
                        }
                    });
                    ui.label(
                        egui::RichText::new(if test == Test::FlowPass1 {
                            "Patches at −20, −15, −10, −5, 0, +5, +10, +15 and +20 % of the current flow."
                        } else {
                            "Patches at −9 … 0 % of the current flow, in 1 % steps."
                        })
                        .font(theme::sans(11.))
                        .color(p.muted),
                    );
                } else {
                    let (min, max, drag) = match test {
                        Test::Temperature => (150., 350., 1.),
                        Test::Retraction => (0., 10., 0.02),
                        Test::MaxVolumetric => (0.5, 200., 0.1),
                        _ => (0., 1., 0.0005),
                    };
                    let step_spec = match test {
                        Test::Temperature => (5., 5., 1.),
                        Test::Retraction => (0.02, 2., 0.01),
                        Test::MaxVolumetric => (0.1, 20., 0.05),
                        _ => (0.0005, 0.05, 0.0002),
                    };
                    let mut params = card.params.clone();
                    let field = |ui: &mut egui::Ui, id: &str, value: f64, lo: f64, hi: f64, speed: f64| {
                        let mut out = None;
                        row(ui, &field_label(test, id), |ui, control| {
                            let spec = NumberSpec {
                                unit,
                                decimals,
                                min: lo,
                                max: hi,
                                speed,
                            };
                            let fid = ui.make_persistent_id(("calibration", id));
                            out = widgets::number_with(ui, fid, control, value, &spec, true, false);
                        });
                        out
                    };
                    if let Some(v) = field(ui, "start", params.start, min, max, drag) {
                        params.start = snap(test, v);
                    }
                    if let Some(v) = field(ui, "end", params.end, min, max, drag) {
                        params.end = snap(test, v);
                    }
                    if test != Test::Temperature
                        && let Some(v) = field(ui, "step", params.step, step_spec.0, step_spec.1, step_spec.2)
                    {
                        params.step = v;
                    }
                    if per_mm {
                        ui.label(
                            egui::RichText::new(format!(
                                "The step is per millimeter of height: {} {unit} at the bottom, +{} {unit} every mm.",
                                trim_num(params.start, decimals),
                                trim_num(params.step, decimals)
                            ))
                            .font(theme::sans(11.))
                            .color(p.muted),
                        );
                    } else if test == Test::Temperature {
                        ui.label(
                            egui::RichText::new(
                                "Hottest block at the bottom, 5 °C cooler every 10 mm; within the filament's limits.",
                            )
                            .font(theme::sans(11.))
                            .color(p.muted),
                        );
                    }
                    if test.is_pa() {
                        let suggested = params.speed.is_none();
                        row(ui, "Fast speed", |ui, control| {
                            let switch = Rect::from_min_size(
                                Pos2::new(control.left(), control.center().y - 10.),
                                Vec2::new(34., 20.),
                            );
                            let id = ui.make_persistent_id("calibration-speed-auto");
                            if let Some(on) = widgets::toggle_at(ui, id, switch, suggested, true) {
                                params.speed = if on {
                                    None
                                } else {
                                    Some(
                                        config
                                            .float_at("external_perimeter_speed", 0)
                                            .unwrap_or(100.)
                                            .max(100.)
                                            .round(),
                                    )
                                };
                            }
                            if let Some(v) = params.speed {
                                let rect = Rect::from_min_size(
                                    Pos2::new(control.left() + 42., control.top()),
                                    Vec2::new(control.width() - 42., control.height()),
                                );
                                let spec = NumberSpec {
                                    unit: "mm/s",
                                    decimals: 0,
                                    min: 10.,
                                    max: 1000.,
                                    speed: 1.,
                                };
                                let fid = ui.make_persistent_id("calibration-speed");
                                if let Some(n) = widgets::number_with(ui, fid, rect, v, &spec, true, false) {
                                    params.speed = Some(n);
                                }
                            } else {
                                ui.painter().text(
                                    Pos2::new(control.left() + 42., control.center().y),
                                    Align2::LEFT_CENTER,
                                    "suggested",
                                    theme::sans(12.),
                                    theme::of(ui).muted,
                                );
                            }
                        });
                    }
                    card.params = params;
                }
                // The estimate follows the parameters once a drag is over.
                if !ui.input(|i| i.pointer.any_down()) {
                    card.refresh(&config);
                }
                ui.add_space(4.);
                match &card.estimate {
                    Ok(e) => {
                        ui.label(
                            egui::RichText::new(e.text(card.test))
                                .font(theme::mono(12.))
                                .color(p.accent_hi),
                        );
                    }
                    Err(e) if e.is_empty() => {}
                    Err(e) => {
                        ui.label(
                            egui::RichText::new(format!("! {e}"))
                                .font(theme::sans(12.))
                                .color(theme::WARNING),
                        );
                    }
                }
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.;
                    let ok = card.estimate.is_ok() && !busy;
                    if widgets::primary_button(ui, "Create plate and slice", ok)
                        .on_hover_text(
                            "A new plate named after the test; the engine's overrides apply to that plate only",
                        )
                        .clicked()
                    {
                        create = true;
                    }
                    if widgets::button(ui, "Cancel", true).clicked() {
                        cancel = true;
                    }
                    ui.label(
                        egui::RichText::new("Your profiles are not changed until you save a result.")
                            .font(theme::sans(11.))
                            .color(p.muted),
                    );
                });
                ui.add_space(2.);
            });
        if let Some(t) = switch_test {
            let tool = card.tool;
            card = Card::new(t, &config, tool);
        }
        self.dialogs.calibration = Some(card);
        if !open || cancel {
            self.dialogs.calibration = None;
        }
        if create && let Err(e) = self.create_calibration_plate(ctx) {
            self.notify_error("The calibration plate was not created", e);
        }
    }

    /// The calibration plate's card in the viewport: the state of the print, what to look
    /// for, *Enter result* and *Save to filament profile*.
    pub(crate) fn calibration_window(&mut self, ctx: &egui::Context) {
        let plate = self.plates.current;
        let Some(job) = self.plates.plate(plate).calibration.clone() else {
            return;
        };
        let outcome = self.calibration_outcome();
        let sliced = self.finished().map(|f| f.summary.clone());
        let id = self.plates.plate(plate).id;
        let slicing = self.worker.as_ref().is_some_and(|(i, _)| *i == id);
        let busy = self.busy();
        let head = preset_names::display_name(&job.filament);
        let title = format!("{} · {head}", job.test.short());
        let mut new_input: Option<String> = None;
        let mut new_name: Option<String> = None;
        let mut first_layer: Option<bool> = None;
        let mut save = false;
        let mut remove = false;
        let mut slice = false;
        let viewport = self.viewport_rect;
        // In Preview the layer slider and its label card sit at the right edge; leave them room.
        let shift = if self.preview { 130. } else { 0. };
        egui::Area::new(egui::Id::unique("calibration-plate-card"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W - shift, 14.))
            .show(ctx, |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 24.);
                        ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                        ui.label(
                            egui::RichText::new(widgets::spaced_upper(&title))
                                .font(theme::semibold(11.))
                                .color(p.muted),
                        );
                        // 1. Print it.
                        let state = if job.saved.is_some() {
                            "Printed and read; the result is in your profile".to_string()
                        } else if slicing {
                            "Slicing…".to_string()
                        } else if let Some(s) = &sliced {
                            format!(
                                "{} · {:.1} g · {} samples — Export or Print…, then come back",
                                super::topbar::short_duration(s.time_s),
                                s.grams,
                                job.samples.len()
                            )
                        } else {
                            "Not sliced yet".to_string()
                        };
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(state)
                                    .font(theme::sans(12.))
                                    .color(if sliced.is_some() { p.text } else { p.muted }),
                            );
                            if sliced.is_none() && !slicing && !busy && job.saved.is_none() {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if widgets::styled_button(ui, "Slice", true, true, 24.).clicked() {
                                        slice = true;
                                    }
                                });
                            }
                        });
                        ui.add_space(4.);
                        let pick = match &outcome {
                            Some(Ok(o)) => Some(o.pick),
                            _ => None,
                        };
                        let (rect, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 90.),
                            Sense::hover(),
                        );
                        pictogram(ui, rect, job.test, pick);
                        ui.label(
                            egui::RichText::new(job.test.look_for())
                                .font(theme::sans(11.5))
                                .color(p.label),
                        );
                        ui.add_space(6.);
                        // 2. Enter the result.
                        widgets::group_title(ui, "Enter result");
                        let (label, unit, _) = job.test.result_prompt();
                        let mut input = job.input.clone();
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(label)
                                    .font(theme::sans(12.))
                                    .color(p.label),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if !unit.is_empty() {
                                    ui.label(
                                        egui::RichText::new(unit)
                                            .font(theme::mono(12.))
                                            .color(p.muted),
                                    );
                                }
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut input)
                                        .id(egui::Id::unique("calibration-result"))
                                        .font(theme::mono(13.))
                                        .desired_width(72.)
                                        .hint_text("…"),
                                );
                                if r.changed() {
                                    new_input = Some(input.clone());
                                }
                            });
                        });
                        if job.test == Test::Temperature {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                                if let Some(v) = widgets::toggle_at(
                                    ui,
                                    ui.make_persistent_id("calibration-first-layer"),
                                    r,
                                    job.first_layer_too,
                                    true,
                                ) {
                                    first_layer = Some(v);
                                }
                                ui.label(
                                    egui::RichText::new("First layer too, keeping its offset")
                                        .font(theme::sans(12.))
                                        .color(p.label),
                                );
                            });
                        }
                        match &outcome {
                            Some(Ok(o)) => {
                                ui.label(
                                    egui::RichText::new(&o.headline)
                                        .font(theme::semibold(14.))
                                        .color(p.accent_hi),
                                );
                                ui.label(
                                    egui::RichText::new(&o.detail)
                                        .font(theme::mono(11.))
                                        .color(p.muted_alt),
                                );
                            }
                            Some(Err(e)) if job.input.trim().is_empty() => {
                                ui.label(
                                    egui::RichText::new(e.as_str())
                                        .font(theme::sans(11.5))
                                        .color(p.muted),
                                );
                            }
                            Some(Err(e)) => {
                                ui.label(
                                    egui::RichText::new(format!("! {e}"))
                                        .font(theme::sans(11.5))
                                        .color(theme::WARNING),
                                );
                            }
                            None => {}
                        }
                        ui.add_space(6.);
                        // 3. Save it.
                        if let Some(text) = &job.saved {
                            egui::Frame::new()
                                .fill(p.accent_surface)
                                .stroke(Stroke::new(1., p.accent))
                                .corner_radius(6.)
                                .inner_margin(egui::Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        egui::RichText::new(format!("✓ {text}"))
                                            .font(theme::sans(12.))
                                            .color(p.text),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "The system preset is unchanged; your profile holds only the differences.",
                                        )
                                        .font(theme::sans(11.))
                                        .color(p.label),
                                    );
                                });
                            ui.add_space(4.);
                            ui.horizontal(|ui| {
                                if widgets::link(ui, "Remove this plate").clicked() {
                                    remove = true;
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(
                                        egui::RichText::new(next_step(job.test))
                                            .font(theme::sans(11.))
                                            .color(p.muted),
                                    );
                                });
                            });
                        } else {
                            widgets::group_title(ui, "Save to filament profile");
                            let mut name = job.profile_name.clone();
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Profile")
                                        .font(theme::sans(12.))
                                        .color(p.label),
                                );
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut name)
                                        .id(egui::Id::unique("calibration-profile-name"))
                                        .font(theme::sans(12.5))
                                        .desired_width(ui.available_width() - 4.),
                                );
                                if r.changed() {
                                    new_name = Some(name.clone());
                                }
                            });
                            if let Some(Ok(o)) = &outcome {
                                ui.label(
                                    egui::RichText::new(&o.explanation)
                                        .font(theme::sans(11.))
                                        .color(p.muted),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "A user profile inheriting “{}”; only the calibrated setting is stored. The system preset is never changed.",
                                        preset_names::display_name(&job.filament)
                                    ))
                                    .font(theme::sans(11.))
                                    .color(p.muted),
                                );
                            }
                            ui.add_space(4.);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.;
                                let ok = matches!(outcome, Some(Ok(_))) && !busy;
                                if widgets::primary_button(ui, "Save to filament profile", ok).clicked() {
                                    save = true;
                                }
                                if widgets::link(ui, "Remove plate").clicked() {
                                    remove = true;
                                }
                            });
                        }
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new(format!(
                                "{} settings overridden for this plate only · filament {} · tool {}",
                                job.overrides.len(),
                                head,
                                job.tool + 1
                            ))
                            .font(theme::sans(10.5))
                            .color(p.muted),
                        );
                    });
            });
        if let Some(j) = &mut self.plates.plates[plate].calibration {
            if let Some(i) = new_input {
                j.input = i;
            }
            if let Some(n) = new_name {
                j.profile_name = n;
            }
            if let Some(f) = first_layer {
                j.first_layer_too = f;
            }
        }
        if slice {
            self.slice_current(ctx);
        }
        if save && let Err(e) = self.save_calibration_result() {
            self.notify_error("The calibration result was not saved", e);
        }
        if remove {
            self.record("Remove plate");
            self.plates.remove(plate);
            self.selected = None;
            self.changed();
        }
    }
}

/// `Start`, `End`, `Step` with the test's wording.
fn field_label(test: Test, id: &str) -> String {
    match (test, id) {
        (Test::Temperature, "start") => "Bottom block (hottest)".into(),
        (Test::Temperature, "end") => "Top block (coolest)".into(),
        (Test::PaTower | Test::Retraction | Test::MaxVolumetric, "start") => "At the bottom".into(),
        (Test::PaTower | Test::Retraction | Test::MaxVolumetric, "end") => "At the top".into(),
        (Test::PaTower | Test::Retraction | Test::MaxVolumetric, "step") => "Step per mm".into(),
        (Test::PaLine, "start") => "Front row".into(),
        (Test::PaLine, "end") => "Back row".into(),
        (Test::PaPattern, "start") => "Left group".into(),
        (Test::PaPattern, "end") => "Right group".into(),
        (_, "start") => "Start".into(),
        (_, "end") => "End".into(),
        _ => "Step".into(),
    }
}

/// Temperatures stay on the 5 °C grid the engine requires.
fn snap(test: Test, v: f64) -> f64 {
    if test == Test::Temperature {
        (v / 5.).round() * 5.
    } else {
        v
    }
}

/// `Next: flow rate pass 1` after a save.
fn next_step(test: Test) -> &'static str {
    match test {
        Test::Temperature => "Next: Calibrate › Flow rate · pass 1",
        Test::FlowPass1 => "Next: Calibrate › Flow rate · pass 2",
        Test::FlowPass2 => "Next: Calibrate › Pressure advance",
        Test::PaLine | Test::PaPattern | Test::PaTower => "Next: Calibrate › Retraction test",
        Test::Retraction => "Next: Calibrate › Max volumetric speed",
        Test::MaxVolumetric => "Done: print something",
    }
}

/// A labelled control row: the label at the left, the control column at the right.
fn row(ui: &mut egui::Ui, label: &str, control: impl FnOnce(&mut egui::Ui, Rect)) {
    let p = theme::of(ui);
    let (r, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), CONTROL_H + 6.),
        Sense::hover(),
    );
    ui.painter().text(
        Pos2::new(r.left(), r.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::sans(13.),
        p.label,
    );
    let width = CONTROL_W.max(r.width() - LABEL_W);
    let rect = Rect::from_min_size(
        Pos2::new(r.right() - width, r.center().y - CONTROL_H / 2.),
        Vec2::new(width, CONTROL_H),
    );
    control(ui, rect);
}

/// A pictogram of the test with what to look for; `pick` highlights the chosen sample along
/// the test's axis (0 = first sample, 1 = last), the middle when there is no reading yet.
pub fn pictogram(ui: &mut egui::Ui, rect: Rect, test: Test, pick: Option<f32>) {
    let p = theme::of(ui);
    let painter = ui.painter_at(rect);
    let line = p.muted_alt;
    let faint = p.muted.gamma_multiply(0.7);
    let accent = p.accent_hi;
    let rect = rect.shrink2(Vec2::new(8., 6.));
    let n = match test {
        Test::FlowPass1 => 9,
        Test::FlowPass2 => 10,
        Test::PaLine | Test::PaPattern => 5,
        Test::PaTower | Test::Retraction | Test::MaxVolumetric | Test::Temperature => 5,
    };
    let picked = pick.map_or(n / 2, |f| ((f * n as f32).floor() as usize).min(n - 1));
    let caption = |at: Pos2, text: &str, anchor: Align2| {
        painter.text(at, anchor, text, theme::sans(10.), p.muted);
    };
    match test {
        Test::FlowPass1 | Test::FlowPass2 => {
            // Viewed from above: front row at the bottom, three per row, left to right.
            let rows = n.div_ceil(3);
            let (pw, ph) = (34., 20.);
            let x0 = rect.left() + 18.;
            for i in 0..n {
                let col = i % 3;
                let row = i / 3;
                let x = x0 + col as f32 * (pw + 10.);
                let y = rect.bottom() - 4. - ph - row as f32 * (ph + 6.);
                let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(pw, ph));
                let best = i == picked;
                let colour = if best { accent } else { line };
                painter.rect_stroke(
                    r,
                    2.,
                    Stroke::new(if best { 1.6 } else { 1. }, colour),
                    StrokeKind::Inside,
                );
                // The top surface: under-extruded gaps, good, over-extruded ridges.
                let regime = (i * 3) / n;
                for k in 0..4 {
                    let y = r.top() + 4. + k as f32 * 4.;
                    match regime {
                        0 => {
                            for seg in 0..3 {
                                let a = r.left() + 4. + seg as f32 * 9.;
                                painter.line_segment(
                                    [Pos2::new(a, y), Pos2::new(a + 5., y)],
                                    Stroke::new(1., faint),
                                );
                            }
                        }
                        2 => {
                            painter.line_segment(
                                [Pos2::new(r.left() + 3., y), Pos2::new(r.right() - 3., y)],
                                Stroke::new(2.2, faint),
                            );
                        }
                        _ => {
                            painter.line_segment(
                                [Pos2::new(r.left() + 3., y), Pos2::new(r.right() - 3., y)],
                                Stroke::new(1., if best { accent } else { line }),
                            );
                        }
                    }
                }
                painter.text(
                    Pos2::new(r.right() - 2., r.bottom() - 1.),
                    Align2::RIGHT_BOTTOM,
                    (i + 1).to_string(),
                    theme::mono(8.5),
                    if best { accent } else { p.muted },
                );
            }
            let _ = rows;
            let legend_x = x0 + 3. * (pw + 10.) + 6.;
            caption(
                Pos2::new(legend_x, rect.top() + 6.),
                "← gaps: too little",
                Align2::LEFT_TOP,
            );
            caption(
                Pos2::new(legend_x, rect.center().y),
                "smooth, lines touching: best",
                Align2::LEFT_CENTER,
            );
            caption(
                Pos2::new(legend_x, rect.bottom() - 6.),
                "ridges: too much →",
                Align2::LEFT_BOTTOM,
            );
            caption(
                Pos2::new(x0 - 4., rect.bottom() - 4.),
                "front",
                Align2::RIGHT_BOTTOM,
            );
        }
        Test::PaLine => {
            // Rows front (bottom) to back (top); slow – fast – slow.
            let x0 = rect.left() + 22.;
            let x1 = rect.right() - 90.;
            let w = x1 - x0;
            for i in 0..n {
                let y = rect.bottom() - 8. - i as f32 * (rect.height() - 14.) / (n - 1) as f32;
                let best = i == picked;
                let colour = if best { accent } else { line };
                let (a, b) = (x0 + w * 0.25, x0 + w * 0.75);
                let gap = if i > picked { 3. } else { 0. };
                painter.line_segment(
                    [Pos2::new(x0, y), Pos2::new(a - gap, y)],
                    Stroke::new(1.2, colour),
                );
                painter.line_segment(
                    [Pos2::new(a + gap, y), Pos2::new(b - gap, y)],
                    Stroke::new(1.2, colour),
                );
                painter.line_segment(
                    [Pos2::new(b + gap, y), Pos2::new(x1, y)],
                    Stroke::new(1.2, colour),
                );
                if i < picked {
                    // Too little advance: blobs where the speed changes.
                    let r = 1.6 + (picked - i) as f32 * 0.9;
                    painter.circle_filled(Pos2::new(a, y), r, faint);
                    painter.circle_filled(Pos2::new(b, y), r, faint);
                }
                painter.text(
                    Pos2::new(x0 - 6., y),
                    Align2::RIGHT_CENTER,
                    (i + 1).to_string(),
                    theme::mono(8.5),
                    if best { accent } else { p.muted },
                );
                if best {
                    caption(Pos2::new(x1 + 6., y), "even: best", Align2::LEFT_CENTER);
                }
            }
            caption(
                Pos2::new(x1 + 6., rect.bottom() - 6.),
                "blobs: too little",
                Align2::LEFT_BOTTOM,
            );
            caption(
                Pos2::new(x1 + 6., rect.top() + 6.),
                "gaps: too much",
                Align2::LEFT_TOP,
            );
            caption(
                Pos2::new(x0 + w * 0.5, rect.bottom() + 2.),
                "slow · fast · slow",
                Align2::CENTER_BOTTOM,
            );
        }
        Test::PaPattern => {
            // Groups left to right, three nested corners each.
            let x0 = rect.left() + 6.;
            let group_w = (rect.width() - 96.) / n as f32;
            let h = rect.height() - 14.;
            for i in 0..n {
                let gx = x0 + i as f32 * group_w;
                let best = i == picked;
                let colour = if best { accent } else { line };
                for k in 0..3 {
                    let inset = k as f32 * 5.;
                    let top = Pos2::new(gx + inset, rect.top() + 4. + inset);
                    let tip = Pos2::new(gx + group_w - 10. - inset, rect.top() + 4. + h / 2.);
                    let bottom = Pos2::new(gx + inset, rect.top() + 4. + h - inset);
                    if i > picked {
                        // Too much: the tip opens.
                        let t = 0.82;
                        painter.line_segment([top, top.lerp(tip, t)], Stroke::new(1.2, colour));
                        painter
                            .line_segment([bottom, bottom.lerp(tip, t)], Stroke::new(1.2, colour));
                    } else {
                        painter.add(egui::Shape::line(
                            vec![top, tip, bottom],
                            Stroke::new(1.2, colour),
                        ));
                        if i < picked {
                            painter.circle_filled(tip, 1.8 + (picked - i) as f32 * 0.8, faint);
                        }
                    }
                }
                painter.text(
                    Pos2::new(gx + 2., rect.bottom()),
                    Align2::LEFT_BOTTOM,
                    (i + 1).to_string(),
                    theme::mono(8.5),
                    if best { accent } else { p.muted },
                );
            }
            let lx = x0 + n as f32 * group_w + 4.;
            caption(
                Pos2::new(lx, rect.top() + 6.),
                "← round, fat tips: too little",
                Align2::LEFT_TOP,
            );
            caption(
                Pos2::new(lx, rect.center().y),
                "crisp corners: best",
                Align2::LEFT_CENTER,
            );
            caption(
                Pos2::new(lx, rect.bottom() - 6.),
                "open tips: too much →",
                Align2::LEFT_BOTTOM,
            );
        }
        Test::PaTower | Test::Retraction | Test::MaxVolumetric | Test::Temperature => {
            // Side view: a tower with bands, the value rising with height.
            let tw = if test == Test::Retraction { 90. } else { 70. };
            let x0 = rect.left() + 30.;
            let (y0, y1) = (rect.bottom() - 4., rect.top() + 4.);
            let band = (y0 - y1) / n as f32;
            // Ground.
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 4., y0),
                    Pos2::new(x0 + tw + 20., y0),
                ],
                Stroke::new(1., faint),
            );
            for i in 0..n {
                let top = y0 - (i + 1) as f32 * band;
                let bottom = y0 - i as f32 * band;
                let best = i == picked;
                let colour = if best { accent } else { line };
                let r = Rect::from_min_max(Pos2::new(x0, top), Pos2::new(x0 + tw, bottom));
                match test {
                    Test::Retraction => {
                        // Two posts; strings between them until the clean band.
                        for px in [x0, x0 + tw - 10.] {
                            painter.rect_stroke(
                                Rect::from_min_size(Pos2::new(px, top), Vec2::new(10., band)),
                                0.,
                                Stroke::new(1., colour),
                                StrokeKind::Inside,
                            );
                        }
                        if i < picked {
                            for k in 0..2 {
                                let y = top + band * (0.3 + 0.4 * k as f32);
                                let mut pts = Vec::new();
                                for s in 0..=8 {
                                    let t = s as f32 / 8.;
                                    pts.push(Pos2::new(
                                        x0 + 10. + (tw - 20.) * t,
                                        y + ((s % 2) as f32 - 0.5) * 1.5,
                                    ));
                                }
                                painter.add(egui::Shape::line(pts, Stroke::new(0.8, faint)));
                            }
                        }
                    }
                    Test::Temperature => {
                        // Blocks with a bridge: sag and strings when hot (low blocks).
                        for px in [x0, x0 + tw - 14.] {
                            painter.rect_stroke(
                                Rect::from_min_size(Pos2::new(px, top), Vec2::new(14., band)),
                                0.,
                                Stroke::new(1., colour),
                                StrokeKind::Inside,
                            );
                        }
                        let sag = if i < picked {
                            (picked - i) as f32 * 2.5
                        } else {
                            0.
                        };
                        let a = Pos2::new(x0 + 14., top + 3.);
                        let b = Pos2::new(x0 + tw - 14., top + 3.);
                        let mut pts = Vec::new();
                        for s in 0..=10 {
                            let t = s as f32 / 10.;
                            let y = a.y + sag * (4. * t * (1. - t));
                            pts.push(Pos2::new(a.x + (b.x - a.x) * t, y));
                        }
                        painter.add(egui::Shape::line(pts, Stroke::new(1.2, colour)));
                        if i + 1 < picked {
                            painter.line_segment(
                                [
                                    Pos2::new(x0 + 14., bottom - 4.),
                                    Pos2::new(x0 + tw - 14., bottom - 7.),
                                ],
                                Stroke::new(0.7, faint),
                            );
                        }
                        painter.text(
                            Pos2::new(x0 - 6., (top + bottom) / 2.),
                            Align2::RIGHT_CENTER,
                            format!("{}°", 230 - i * 5),
                            theme::mono(8.5),
                            if best { accent } else { p.muted },
                        );
                    }
                    Test::MaxVolumetric => {
                        // A single wall: smooth, then rough above the limit.
                        let rough = i > picked;
                        if rough {
                            for side in [x0, x0 + tw] {
                                let mut pts = Vec::new();
                                for s in 0..=6 {
                                    let t = s as f32 / 6.;
                                    pts.push(Pos2::new(
                                        side + ((s % 2) as f32 - 0.5) * 3.,
                                        bottom - band * t,
                                    ));
                                }
                                painter.add(egui::Shape::line(pts, Stroke::new(1., faint)));
                            }
                        } else {
                            painter.rect_stroke(r, 0., Stroke::new(1., colour), StrokeKind::Inside);
                        }
                    }
                    _ => {
                        // PA tower: blobby corners low, thin corners high.
                        painter.rect_stroke(
                            r,
                            0.,
                            Stroke::new(if i > picked { 0.6 } else { 1. }, colour),
                            StrokeKind::Inside,
                        );
                        if i < picked {
                            let rad = 1.5 + (picked - i) as f32 * 0.8;
                            painter.circle_filled(Pos2::new(x0, (top + bottom) / 2.), rad, faint);
                            painter.circle_filled(
                                Pos2::new(x0 + tw, (top + bottom) / 2.),
                                rad,
                                faint,
                            );
                        }
                    }
                }
                if best {
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x0 - 2., top),
                            Pos2::new(x0 + tw + 2., bottom),
                        ),
                        0.,
                        accent.gamma_multiply(0.12),
                    );
                }
            }
            // Height arrow and legend.
            let ax = x0 + tw + 12.;
            painter.line_segment(
                [Pos2::new(ax, y0), Pos2::new(ax, y1)],
                Stroke::new(1., faint),
            );
            painter.line_segment(
                [Pos2::new(ax - 3., y1 + 5.), Pos2::new(ax, y1)],
                Stroke::new(1., faint),
            );
            painter.line_segment(
                [Pos2::new(ax + 3., y1 + 5.), Pos2::new(ax, y1)],
                Stroke::new(1., faint),
            );
            let best_y = y0 - (picked as f32 + 0.5) * band;
            painter.line_segment(
                [Pos2::new(ax - 3., best_y), Pos2::new(ax + 3., best_y)],
                Stroke::new(1.4, accent),
            );
            let (low, high, best) = match test {
                Test::Retraction => ("strings: too short", "clean above here", "read this height"),
                Test::Temperature => (
                    "sag, strings: too hot",
                    "dull, weak: too cold",
                    "best block",
                ),
                Test::MaxVolumetric => {
                    ("smooth: fine", "rough, thin: too fast", "read this height")
                }
                _ => (
                    "blobby corners: too little",
                    "thin corners: too much",
                    "sharp corners: read this height",
                ),
            };
            caption(Pos2::new(ax + 8., y0 - 6.), low, Align2::LEFT_BOTTOM);
            caption(Pos2::new(ax + 8., y1 + 6.), high, Align2::LEFT_TOP);
            painter.text(
                Pos2::new(ax + 8., best_y),
                Align2::LEFT_CENTER,
                best,
                theme::sans(10.),
                accent,
            );
            if test != Test::Temperature {
                caption(Pos2::new(x0 - 4., y0 - 2.), "0", Align2::RIGHT_BOTTOM);
                caption(Pos2::new(x0 - 4., y1 + 2.), "mm", Align2::RIGHT_TOP);
            }
        }
    }
    let _ = Color32::TRANSPARENT;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Options;
    use mica_profiles::user::Store;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

    /// A CORE One app with an empty plate and a throwaway profile store.
    fn app() -> (egui::Context, App, std::path::PathBuf) {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!(
            "mica-calib-test-{}-{}",
            std::process::id(),
            NEXT_STORE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::create(
            &ctx,
            Options {
                prefs_path: None,
                store: Some(Store::open(dir.clone())),
                files: vec![],
                headless: true,
            },
        );
        let (vi, _, name) = app
            .profiles
            .all_printers()
            .into_iter()
            .find(|(_, _, n)| n == "Prusa CORE One HF0.4 nozzle")
            .expect("the Prusa bundle has the CORE One");
        app.profiles.select_printer(vi, &name);
        if let Some(f) = app
            .profiles
            .materials
            .iter()
            .find(|m| m.starts_with("Prusament PLA"))
            .cloned()
        {
            app.profiles.filaments[0] = f;
        }
        app.profiles.refresh();
        app.settings_changed();
        (ctx, app, dir)
    }

    /// Create → result → save: the plate is named after the test and the filament, locked,
    /// carries the engine's overrides on its job only (the global settings are untouched),
    /// the typed height becomes a pressure advance, and saving writes a user filament profile
    /// (inheriting the system preset, the one changed key) that is selected for the tool. The
    /// system preset itself is unchanged. Undo removes the plate again.
    #[test]
    fn create_read_and_save_a_pa_tower_on_the_core_one() {
        let (ctx, mut app, dir) = app();
        let filament = app.profiles.filaments[0].clone();
        assert!(!app.profiles.is_user_preset(PresetKind::Filament, &filament));
        let global_before = app.effective_config().unwrap().clone();
        app.open_calibration(Test::PaTower);
        let card = app.dialogs.calibration.as_ref().expect("card open");
        assert_eq!(card.test, Test::PaTower);
        let estimate = card.estimate.as_ref().expect("estimate");
        assert_eq!(estimate.samples, 51);
        let plate = app.create_calibration_plate(&ctx).unwrap();
        app.cancel_slice();
        assert!(app.dialogs.calibration.is_none());
        let p = app.plates.plate(plate);
        assert_eq!(p.name, "PA tower · Prusament PLA");
        assert!(p.locked);
        assert_eq!(p.objects.len(), 1);
        assert_eq!(p.objects[0].name, "calibration tower");
        let job = p.calibration.clone().expect("a calibration plate");
        assert_eq!(job.samples.len(), 51);
        assert!(
            job.overrides
                .iter()
                .any(|(k, v)| k == "layer_height" && v == "0.2")
        );
        assert!(
            p.overrides.is_empty(),
            "the plate's own overrides stay empty"
        );
        assert_eq!(job.profile_name, "Prusament PLA tuned");
        // The user's settings did not change.
        assert_eq!(
            app.effective_config().unwrap().raw("layer_height"),
            global_before.raw("layer_height")
        );
        assert!(app.overlays.print.is_empty());
        // No reading yet: the card asks for one.
        assert!(matches!(app.calibration_outcome(), Some(Err(_))));
        app.plates.plates[plate].calibration.as_mut().unwrap().input = "21.7".into();
        let outcome = app.calibration_outcome().unwrap().unwrap();
        assert_eq!(outcome.headline, "Pressure advance 0.042");
        assert_eq!(outcome.changes.len(), 1);
        assert_eq!(outcome.changes[0].0, "start_filament_gcode");
        // Save: a user profile with the one key, selected for tool 1.
        app.save_calibration_result().unwrap();
        let saved = app.profiles.filaments[0].clone();
        assert_eq!(saved, "Prusament PLA tuned");
        assert!(app.profiles.is_user_preset(PresetKind::Filament, &saved));
        let profile = app
            .profiles
            .store
            .load(mica_profiles::prusa::Kind::Filament, &saved)
            .unwrap();
        assert_eq!(profile.inherits, filament);
        assert_eq!(profile.settings.len(), 1);
        let gcode =
            mica_core::config::unescape_string(&profile.settings["start_filament_gcode"]).unwrap();
        assert_eq!(calibration::m572_value(&gcode, 0.4), Some(0.042));
        assert_eq!(calibration::m572_value(&gcode, 0.6), Some(0.022));
        // The effective configuration prints with it; the system preset still says 0.036.
        let now = app.effective_config().unwrap().clone();
        assert_eq!(calibration::current_pressure_advance(&now, 0), Some(0.042));
        app.profiles.filaments[0] = filament.clone();
        app.profiles.refresh();
        app.settings_changed();
        assert_eq!(
            calibration::current_pressure_advance(app.effective_config().unwrap(), 0),
            Some(0.036)
        );
        let job = app.plates.plate(plate).calibration.clone().unwrap();
        assert!(
            job.saved
                .as_deref()
                .unwrap()
                .contains("Pressure advance 0.042")
        );
        // Saving again into the same profile keeps it one profile.
        app.profiles.filaments[0] = saved.clone();
        app.profiles.refresh();
        app.settings_changed();
        app.plates.plates[plate].calibration.as_mut().unwrap().input = "10".into();
        app.save_calibration_result().unwrap();
        assert_eq!(app.profiles.filaments[0], saved);
        assert_eq!(
            calibration::current_pressure_advance(app.effective_config().unwrap(), 0),
            Some(0.02)
        );
        // Undo takes the plate away.
        while app.undo.can_undo() {
            app.undo();
        }
        assert!(app.plates.plates.iter().all(|p| p.calibration.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every test's card opens with a valid estimate on the CORE One, and a flow pass or a
    /// temperature reading saves the matching filament key.
    #[test]
    fn every_test_opens_and_flow_and_temperature_save_their_keys() {
        let (ctx, mut app, dir) = app();
        for test in Test::ALL {
            app.open_calibration(test);
            let card = app.dialogs.calibration.as_ref().unwrap();
            assert!(card.estimate.is_ok(), "{test:?}: {:?}", card.estimate);
        }
        app.open_calibration(Test::FlowPass1);
        let plate = app.create_calibration_plate(&ctx).unwrap();
        app.cancel_slice();
        assert_eq!(app.plates.plate(plate).objects.len(), 9);
        let old = app
            .effective_config()
            .unwrap()
            .float("extrusion_multiplier")
            .unwrap();
        app.plates.plates[plate].calibration.as_mut().unwrap().input = "4".into();
        app.save_calibration_result().unwrap();
        let now = app
            .effective_config()
            .unwrap()
            .float("extrusion_multiplier")
            .unwrap();
        assert!((now - old * 0.95).abs() < 1e-6, "{now} vs {old}");
        // Temperature, into the profile the flow pass just made.
        app.open_calibration(Test::Temperature);
        let plate = app.create_calibration_plate(&ctx).unwrap();
        app.cancel_slice();
        let job = app.plates.plate(plate).calibration.as_ref().unwrap();
        assert_eq!(job.profile_name, app.profiles.filaments[0]);
        assert!(job.samples.len() >= 4);
        let t = job.samples[1];
        app.plates.plates[plate].calibration.as_mut().unwrap().input = format!("{t}");
        app.save_calibration_result().unwrap();
        let c = app.effective_config().unwrap();
        assert_eq!(c.float("temperature"), Some(t));
        assert!((c.float("extrusion_multiplier").unwrap() - old * 0.95).abs() < 1e-6);
        let profile = app
            .profiles
            .store
            .load(
                mica_profiles::prusa::Kind::Filament,
                &app.profiles.filaments[0],
            )
            .unwrap();
        assert!(profile.settings.contains_key("extrusion_multiplier"));
        assert!(profile.settings.contains_key("temperature"));
        assert!(profile.settings.contains_key("first_layer_temperature"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
