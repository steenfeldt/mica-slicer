// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoCut.cpp :: render_connectors_input_window, render_slider_double_input
// Changes: Only the cut card's connectors section follows upstream (type with its dependent style and shape, size and depth each with a tolerance under it, rotation, delete and clear); it is laid out as Mica chips and fields, the size and its tolerance are diameters as upstream's UI shows them, and "Add connectors" toggles a click-to-place mode instead of a separate connectors window. The volume, range and plane cards are original.

//! Tool cards for what sits inside an object: a selected modifier / negative / support
//! volume (position, rotation, size relative to the object), a selected height range
//! (From / To) and the cut tool's plane (height, tilt, what to keep). Same place and look
//! as the object's tool card in `objects.rs`; one typed value or one slider drag is one
//! undo entry.

use super::widgets::{self, Glyph, NumberSpec};
use crate::app::App;
use crate::cut::Keep;
use crate::scene::{Sub, Volume};
use crate::theme;
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};
use glam::Vec3;
use mica_core::cut_connectors as cc;

const CARD_W: f32 = 300.;
const FIELD_W: f32 = 56.;
const FIELD_H: f32 = 26.;
const LABEL_W: f32 = 74.;

/// A row of three compact X / Y / Z fields after a muted label; returns the axis and value
/// edited this frame.
fn triple(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    values: Vec3,
    decimals: usize,
    min: f64,
    speed: f64,
) -> Option<(usize, f32)> {
    let p = theme::of(ui);
    let mut edited = None;
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), FIELD_H + 2.),
        Sense::hover(),
    );
    ui.painter().text(
        Pos2::new(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::sans(12.),
        p.muted_alt,
    );
    for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
        let x = row.left() + LABEL_W + k as f32 * (FIELD_W + 6.);
        let rect = Rect::from_min_size(
            Pos2::new(x, row.center().y - FIELD_H / 2.),
            Vec2::new(FIELD_W, FIELD_H),
        );
        ui.painter().text(
            Pos2::new(rect.left() + 6., rect.center().y),
            Align2::LEFT_CENTER,
            *axis,
            theme::sans(10.5),
            p.muted,
        );
        let spec = NumberSpec {
            unit: "",
            decimals,
            min,
            max: 1e6,
            speed,
        };
        let fid = ui.make_persistent_id(("sub-field", id, k));
        if let Some(v) = widgets::number_with(ui, fid, rect, values[k] as f64, &spec, true, false) {
            edited = Some((k, v as f32));
        }
    }
    edited
}

/// One labelled field (From / To / Z / tilt).
#[allow(clippy::too_many_arguments)]
pub(super) fn single(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    value: f32,
    unit: &'static str,
    decimals: usize,
    min: f64,
    max: f64,
    speed: f64,
) -> Option<f32> {
    let p = theme::of(ui);
    let (row, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), FIELD_H + 2.),
        Sense::hover(),
    );
    ui.painter().text(
        Pos2::new(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::sans(12.),
        p.muted_alt,
    );
    let rect = Rect::from_min_size(
        Pos2::new(row.left() + LABEL_W, row.center().y - FIELD_H / 2.),
        Vec2::new(FIELD_W * 1.6, FIELD_H),
    );
    let spec = NumberSpec {
        unit,
        decimals,
        min,
        max,
        speed,
    };
    let fid = ui.make_persistent_id(("sub-single", id));
    widgets::number_with(ui, fid, rect, value as f64, &spec, true, false).map(|v| v as f32)
}

pub(super) fn card(
    ctx: &egui::Context,
    viewport: egui::Rect,
    title: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Area::new(egui::Id::unique("object-card"))
        .order(egui::Order::Middle)
        .fixed_pos(viewport.right_top() + egui::vec2(-14. - CARD_W, 14.))
        .show(ctx, |ui| {
            let p = theme::of(ui);
            theme::overlay_frame(ui)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(CARD_W - 24.);
                    ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                    ui.label(
                        egui::RichText::new(widgets::spaced_upper(title))
                            .font(theme::semibold(11.))
                            .color(p.muted),
                    );
                    ui.add_space(4.);
                    body(ui);
                });
        });
}

impl App {
    /// The card for a selected volume: its placement inside the object, size, kind, and
    /// Delete. Values are in the object's own millimetres (before the object's transform).
    pub(crate) fn volume_window(&mut self, ctx: &egui::Context, i: usize, id: u64) {
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let Some(v) = o.volume(id) else {
            self.sub = None;
            return;
        };
        let title = format!("{} · {}", v.name, v.kind_label());
        let (bmin, bmax) = v.local_bounds;
        let base = Vec3::from(bmax) - Vec3::from(bmin);
        let (x, y, z) = v.rotation.to_euler(glam::EulerRot::XYZ);
        let rotation = Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees());
        let position = v.position;
        let scale = v.scale;
        let kind = v.kind.clone();
        let overrides = v.overrides.len();
        let mut new_position = None;
        let mut new_rotation = None;
        let mut new_size = None;
        let mut delete = false;
        let mut settings = false;
        let mut back = false;
        let viewport = self.viewport_rect;
        card(ctx, viewport, &title, |ui| {
            let p = theme::of(ui);
            if let Some((k, val)) = triple(ui, "Position mm", "vpos", position, 1, -1e6, 0.5) {
                new_position = Some((k, val));
            }
            if let Some((k, val)) = triple(ui, "Rotation °", "vrot", rotation, 0, -1e6, 1.) {
                new_rotation = Some((k, val));
            }
            if let Some((k, val)) = triple(ui, "Size mm", "vsize", base * scale, 1, 0.1, 0.5) {
                new_size = Some((k, val));
            }
            ui.add_space(6.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                ui.label(
                    egui::RichText::new(match kind {
                        mica_core::threemf::PartKind::Modifier => {
                            "Changes settings inside its shape"
                        }
                        mica_core::threemf::PartKind::NegativeVolume => "Takes material away",
                        mica_core::threemf::PartKind::SupportEnforcer => {
                            "Forces supports inside its shape"
                        }
                        mica_core::threemf::PartKind::SupportBlocker => {
                            "Keeps supports out of its shape"
                        }
                        _ => "",
                    })
                    .font(theme::sans(12.))
                    .color(p.muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button_sized(ui, Glyph::Close, "Delete this volume", true, 24.)
                        .clicked()
                    {
                        delete = true;
                    }
                });
            });
            ui.add_space(4.);
            ui.horizontal(|ui| {
                let text = if matches!(kind, mica_core::threemf::PartKind::Modifier) {
                    format!(
                        "Modifier settings · {overrides} override{}",
                        if overrides == 1 { "" } else { "s" }
                    )
                } else {
                    "Settings…".to_string()
                };
                if widgets::link(ui, &text).clicked() {
                    settings = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::link(ui, "← Object").clicked() {
                        back = true;
                    }
                });
            });
        });
        if new_position.is_some() || new_rotation.is_some() || new_size.is_some() {
            self.begin_gesture("Edit volume");
            let o = &mut self.plates.current_mut().objects[i];
            if let Some(v) = o.volume_mut(id) {
                if let Some((k, val)) = new_position {
                    v.position[k] = val;
                }
                if let Some((k, val)) = new_rotation {
                    let mut r = rotation;
                    r[k] = val;
                    v.rotation = glam::Quat::from_euler(
                        glam::EulerRot::XYZ,
                        r.x.to_radians(),
                        r.y.to_radians(),
                        r.z.to_radians(),
                    )
                    .normalize();
                }
                if let Some((k, val)) = new_size
                    && base[k] > 0.
                {
                    v.scale[k] = (val / base[k]).max(0.001);
                }
            }
            self.sync_instances(i);
            self.changed();
        }
        if delete {
            self.delete_sub();
        }
        if settings {
            self.editor.object_mode = true;
        }
        if back {
            self.sub = None;
        }
    }

    /// The card for a selected height range: From / To above the object's bottom, Delete.
    pub(crate) fn range_window(&mut self, ctx: &egui::Context, i: usize, id: u64) {
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let Some(r) = o.range(id) else {
            self.sub = None;
            return;
        };
        let (min, max) = o.bounds();
        let height = max.z - min.z;
        let (lo, hi) = (r.min_z, r.max_z);
        let overrides = r.overrides.len();
        let title = format!("Height range · {}", r.label());
        let mut new_lo = None;
        let mut new_hi = None;
        let mut delete = false;
        let mut settings = false;
        let mut back = false;
        let viewport = self.viewport_rect;
        card(ctx, viewport, &title, |ui| {
            let p = theme::of(ui);
            new_lo = single(ui, "From", "rfrom", lo, "mm", 2, 0., height as f64, 0.1);
            new_hi = single(ui, "To", "rto", hi, "mm", 2, 0., height as f64, 0.1);
            ui.add_space(4.);
            ui.label(
                egui::RichText::new(format!(
                    "Heights above the object's bottom; the object is {height:.1} mm tall."
                ))
                .font(theme::sans(12.))
                .color(p.muted),
            );
            ui.add_space(4.);
            ui.horizontal(|ui| {
                if widgets::link(
                    ui,
                    &format!(
                        "Range settings · {overrides} override{}",
                        if overrides == 1 { "" } else { "s" }
                    ),
                )
                .clicked()
                {
                    settings = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button_sized(ui, Glyph::Close, "Delete this range", true, 24.)
                        .clicked()
                    {
                        delete = true;
                    }
                    if widgets::link(ui, "← Object").clicked() {
                        back = true;
                    }
                });
            });
        });
        if new_lo.is_some() || new_hi.is_some() {
            self.begin_gesture("Edit height range");
            let o = &mut self.plates.current_mut().objects[i];
            if let Some(r) = o.range_mut(id) {
                if let Some(v) = new_lo {
                    r.min_z = v.min(r.max_z - 0.01).max(0.);
                }
                if let Some(v) = new_hi {
                    r.max_z = v.max(r.min_z + 0.01).min(height);
                }
            }
            o.normalize_ranges();
            if o.range(id).is_none() {
                self.sub = None;
            }
            self.sync_instances(i);
            self.changed();
        }
        if delete {
            self.delete_sub();
        }
        if settings {
            self.editor.object_mode = true;
        }
        if back {
            self.sub = None;
        }
    }

    /// The cut tool's card: plane height and tilt, which halves to keep, place on the cut
    /// face, connectors (later) and the Cut button.
    pub(crate) fn cut_window(&mut self, ctx: &egui::Context) {
        self.validate_cut();
        if let Some(c) = &mut self.cut
            && let Some(o) = self.plates.current().objects.get(c.object)
        {
            c.check(o);
        }
        if let Some(c) = &mut self.cut {
            // Opening "Add connectors" says the halves should join: Cut without any asks.
            c.wants_connectors |= c.editing;
            if c.prompt == Some(crate::cut::CutPrompt::NoConnectors) && !c.connectors.is_empty() {
                c.prompt = None;
            }
        }
        let Some(cut) = self.cut.clone() else { return };
        let mut connector_ui = ConnectorUi::default();
        let name = self
            .plates
            .current()
            .objects
            .get(cut.object)
            .map(|o| {
                std::path::Path::new(&o.name)
                    .file_stem()
                    .map_or(o.name.clone(), |s| s.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let (zmin, zmax) = self
            .plates
            .current()
            .objects
            .get(cut.object)
            .map(|o| {
                let (a, b) = o.bounds();
                (a.z, b.z)
            })
            .unwrap_or((0., 100.));
        let mut new_z = None;
        let mut new_tx = None;
        let mut new_ty = None;
        let mut keep = None;
        let mut place = None;
        let mut flip = false;
        let mut reset = false;
        let mut apply = false;
        let mut cancel = false;
        let viewport = self.viewport_rect;
        card(ctx, viewport, &format!("{name} · Cut"), |ui| {
            let p = theme::of(ui);
            new_z = single(
                ui,
                "Height",
                "cutz",
                cut.z(),
                "mm",
                2,
                zmin as f64,
                zmax as f64,
                0.2,
            );
            new_tx = single(
                ui,
                "Tilt X",
                "cuttx",
                cut.tilt_x.to_degrees(),
                "°",
                0,
                -89.,
                89.,
                1.,
            );
            new_ty = single(
                ui,
                "Tilt Y",
                "cutty",
                cut.tilt_y.to_degrees(),
                "°",
                0,
                -89.,
                89.,
                1.,
            );
            ui.add_space(6.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                ui.label(
                    egui::RichText::new("Keep")
                        .font(theme::sans(12.))
                        .color(p.muted_alt),
                );
                for (k, label) in [
                    (Keep::Upper, "Upper"),
                    (Keep::Lower, "Lower"),
                    (Keep::Both, "Both"),
                ] {
                    // Connectors join both halves: both are kept (upstream).
                    let free = cut.connectors.is_empty();
                    let on = if free { cut.keep == k } else { k == Keep::Both };
                    if widgets::chip(ui, None, label, on, free).clicked() {
                        keep = Some(k);
                    }
                }
            });
            ui.add_space(4.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                let (r, _) = ui.allocate_exact_size(Vec2::new(34., 20.), Sense::hover());
                if let Some(v) = widgets::toggle_at(
                    ui,
                    ui.make_persistent_id("cut-place"),
                    r,
                    cut.place_on_cut,
                    true,
                ) {
                    place = Some(v);
                }
                ui.label(
                    egui::RichText::new("Place on cut face")
                        .font(theme::sans(12.))
                        .color(p.label),
                )
                .on_hover_text("Lay each kept half on its cut so it prints flat");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::styled_button(ui, "Flip", true, false, 24.)
                        .on_hover_text("Turn the plane over (swaps upper and lower)")
                        .clicked()
                    {
                        flip = true;
                    }
                    if widgets::styled_button(ui, "Reset", true, false, 24.)
                        .on_hover_text("Horizontal plane through the middle")
                        .clicked()
                    {
                        reset = true;
                    }
                });
            });
            ui.add_space(6.);
            connectors_section(ui, &cut, &mut connector_ui);
            if cut.prompt == Some(crate::cut::CutPrompt::NoConnectors) {
                ui.add_space(6.);
                match prompt_box(
                    ui,
                    "No connectors placed — the halves won't join.",
                    &["Place automatically", "Cut anyway"],
                ) {
                    Some(0) => connector_ui.auto = Some(None),
                    Some(_) => connector_ui.cut_anyway = true,
                    None => {}
                }
            }
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                let blocked = cut.conflict();
                let label = crate::cut::cut_label(&cut.connectors);
                let button = widgets::primary_button(ui, &label, blocked.is_none());
                if let Some(c) = blocked {
                    button.on_disabled_hover_text(format!(
                        "{}: move or resize it first",
                        c.message()
                    ));
                } else if button.clicked() {
                    apply = true;
                }
                if widgets::styled_button(ui, "Cancel", true, false, 28.).clicked() {
                    cancel = true;
                }
                // The keys, when they fit beside a long "Cut with …" (never cut off).
                for hint in ["Esc cancels · C toggles the tool", "Esc cancels"] {
                    let text = egui::RichText::new(hint)
                        .font(theme::sans(11.))
                        .color(p.muted);
                    let galley = egui::WidgetText::from(text.clone()).into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Body,
                    );
                    if galley.size().x <= ui.available_width() {
                        ui.label(text);
                        break;
                    }
                }
            });
            // Why Cut is off, under it.
            if let Some(c) = cut.conflict() {
                ui.add_space(2.);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} (shown in red): move or resize it first",
                            c.message()
                        ))
                        .font(theme::sans(11.))
                        .color(p.warn_text),
                    )
                    .wrap(),
                );
            }
        });
        if let Some(c) = &mut self.cut {
            if connector_ui.toggle_editing {
                c.editing = !c.editing;
                self.status = if c.editing {
                    "Connectors: click the cut face to place one, or Place automatically · Delete removes the selected one".into()
                } else {
                    format!("{} connector(s) placed", c.connectors.len())
                };
            }
            if let Some((a, s)) = connector_ui.set {
                c.set_connector(a, s);
                c.wants_connectors = true;
            }
            if connector_ui.delete {
                c.edit_connectors(crate::cut::ConnectorEdit::DeleteSelected);
            }
            if connector_ui.clear {
                c.edit_connectors(crate::cut::ConnectorEdit::Clear);
            }
            if let Some(z) = new_z {
                c.set_z(z);
            }
            if let Some(t) = new_tx {
                c.tilt_x = t.to_radians();
            }
            if let Some(t) = new_ty {
                c.tilt_y = t.to_radians();
            }
            if let Some(k) = keep {
                c.keep = k;
            }
            if let Some(v) = place {
                c.place_on_cut = v;
            }
            if flip {
                c.tilt_x += std::f32::consts::PI;
                if c.tilt_x > std::f32::consts::PI {
                    c.tilt_x -= 2. * std::f32::consts::PI;
                }
            }
            if reset && let Some(o) = self.plates.current().objects.get(c.object) {
                let fresh = crate::cut::CutState::for_object(c.object, o);
                c.center = fresh.center;
                c.tilt_x = 0.;
                c.tilt_y = 0.;
            }
        }
        if let Some(replace) = connector_ui.auto {
            self.auto_place_connectors(replace);
        }
        if connector_ui.dismiss
            && let Some(c) = &mut self.cut
        {
            c.prompt = None;
        }
        if connector_ui.cut_anyway {
            self.cut_anyway();
        } else if apply {
            self.apply_cut();
        }
        if cancel {
            self.cut = None;
        }
        let _ = Sub::Range(0);
        let _: Option<&Volume> = None;
    }
}

/// What the connectors section of the cut card asked for this frame.
#[derive(Default)]
struct ConnectorUi {
    toggle_editing: bool,
    set: Option<(cc::Attributes, cc::Sizes)>,
    delete: bool,
    clear: bool,
    /// "Place automatically", with the answer to "replace the earlier ones?" if asked.
    auto: Option<Option<bool>>,
    /// Closes the open inline question without acting.
    dismiss: bool,
    cut_anyway: bool,
}

/// A small inline question in the card (warning colours): `text`, then a row of buttons,
/// the first one primary. Returns the index of the button clicked.
fn prompt_box(ui: &mut egui::Ui, text: &str, buttons: &[&str]) -> Option<usize> {
    let p = theme::of(ui);
    let mut clicked = None;
    egui::Frame::new()
        .fill(p.warn_surface)
        .stroke(egui::Stroke::new(1., p.warn_border))
        .corner_radius(6.)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .font(theme::sans(12.))
                        .color(p.warn_text),
                )
                .wrap(),
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                for (i, b) in buttons.iter().enumerate() {
                    if widgets::styled_button(ui, b, true, i == 0, 24.).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
    clicked
}

/// The cut card's connectors: "Add connectors" mode, then type with its dependent style and
/// shape, size and depth each with their tolerance under them, rotation, the selection's
/// Delete and Clear (upstream's `render_connectors_input_window`). Edits apply to the
/// selected connector and to the next one placed.
fn connectors_section(ui: &mut egui::Ui, cut: &crate::cut::CutState, out: &mut ConnectorUi) {
    use cc::{ConnectorShape, ConnectorStyle, ConnectorType};
    let p = theme::of(ui);
    let (mut a, mut s) = cut
        .selected_connector
        .and_then(|i| cut.connectors.get(i))
        .map_or(cut.connector, |c| (c.attributes, c.sizes));
    let before = (a, s);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.;
        ui.label(
            egui::RichText::new("Connectors")
                .font(theme::sans(12.))
                .color(p.label),
        );
        let n = cut.connectors.len();
        if n > 0 {
            ui.label(
                egui::RichText::new(format!("{n} placed"))
                    .font(theme::sans(11.))
                    .color(p.muted),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if cut.editing {
                "Done"
            } else if n > 0 {
                "Edit connectors"
            } else {
                "Add connectors"
            };
            if widgets::styled_button(ui, label, true, cut.editing, 24.)
                .on_hover_text("Click the cut plane in the view to place plugs, dowels or snaps")
                .clicked()
            {
                out.toggle_editing = true;
            }
        });
    });
    if !cut.editing && cut.connectors.is_empty() {
        return;
    }
    ui.add_space(4.);
    let chips = |ui: &mut egui::Ui, label: &str, f: &mut dyn FnMut(&mut egui::Ui)| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6., 4.);
            ui.label(
                egui::RichText::new(label)
                    .font(theme::sans(12.))
                    .color(p.muted_alt),
            );
            f(ui);
        });
    };
    chips(ui, "Type", &mut |ui| {
        for t in ConnectorType::ALL {
            let tip = match t {
                ConnectorType::Plug => "A peg on the lower half that fits a hole in the upper half",
                ConnectorType::Dowel => "A separate pin; each half gets a hole as deep as Depth",
                ConnectorType::Snap => "A split peg that clicks into a hole in the upper half",
            };
            if widgets::chip(ui, None, t.label(), a.kind == t, true)
                .on_hover_text(tip)
                .clicked()
            {
                a.kind = t;
            }
        }
    });
    // Dependent settings sit indented under the type they belong to.
    ui.indent("cut-connector-type", |ui| {
        if a.kind == ConnectorType::Plug {
            chips(ui, "Style", &mut |ui| {
                for st in ConnectorStyle::ALL {
                    if widgets::chip(ui, None, st.label(), a.style == st, true).clicked() {
                        a.style = st;
                    }
                }
            });
        }
        if a.kind != ConnectorType::Snap {
            chips(ui, "Shape", &mut |ui| {
                for sh in ConnectorShape::ALL {
                    if widgets::chip(ui, None, sh.label(), a.effective().shape == sh, true)
                        .clicked()
                    {
                        a.shape = sh;
                    }
                }
            });
        } else {
            ui.label(
                egui::RichText::new("Snaps are round, with a bulge that clicks into place")
                    .font(theme::sans(11.))
                    .color(p.muted),
            );
        }
    });
    ui.add_space(2.);
    if let Some(v) = single(
        ui,
        "Size",
        "cut-conn-size",
        2. * s.radius,
        "mm",
        2,
        0.4,
        100.,
        0.1,
    ) {
        s.radius = (v / 2.).max(0.2);
    }
    ui.indent("cut-conn-size-tol", |ui| {
        if let Some(v) = single(
            ui,
            "Tolerance",
            "cut-conn-size-tol",
            2. * s.radius_tolerance,
            "mm",
            2,
            0.,
            s.radius.max(0.2) as f64,
            0.01,
        ) {
            s.radius_tolerance = v / 2.;
        }
    });
    // A dowel goes this deep into each half (its type's tip says so; "Depth (each half)" did
    // not fit the label column).
    let depth_label = "Depth";
    // Upstream: a snap is at least as deep as it is wide.
    let depth_min = if a.kind == ConnectorType::Snap {
        2. * s.radius
    } else {
        0.2
    };
    if let Some(v) = single(
        ui,
        depth_label,
        "cut-conn-depth",
        s.height,
        "mm",
        2,
        depth_min as f64,
        200.,
        0.1,
    ) {
        s.height = v.max(depth_min);
    }
    ui.indent("cut-conn-depth-tol", |ui| {
        if let Some(v) = single(
            ui,
            "Tolerance",
            "cut-conn-depth-tol",
            s.height_tolerance,
            "mm",
            2,
            0.,
            (s.height * 0.5).max(0.1) as f64,
            0.01,
        ) {
            s.height_tolerance = v;
        }
    });
    if let Some(v) = single(
        ui,
        "Rotation",
        "cut-conn-angle",
        s.angle.to_degrees(),
        "°",
        0,
        0.,
        180.,
        1.,
    ) {
        s.angle = v.to_radians();
    }
    if (a, s) != before {
        out.set = Some((a, s));
    }
    ui.add_space(2.);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.;
        if widgets::styled_button(ui, "Place automatically", true, false, 24.)
            .on_hover_text(format!(
                "Spread {} of this size over the cut face, {:.1} mm clear of its edges",
                crate::cut::kind_noun(a.kind, 2),
                cc::AUTO_MARGIN
            ))
            .clicked()
        {
            out.auto = Some(None);
        }
        // The selection's buttons on the right, apart from "Place automatically".
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            if widgets::styled_button(ui, "Clear", !cut.connectors.is_empty(), false, 24.)
                .on_hover_text("Remove all connectors")
                .clicked()
            {
                out.clear = true;
            }
            let selected = cut.selected_connector.is_some();
            if widgets::styled_button(ui, "Delete", selected, false, 24.)
                .on_hover_text("Remove the selected connector (Delete)")
                .clicked()
            {
                out.delete = true;
            }
        });
    });
    if cut.prompt == Some(crate::cut::CutPrompt::ReplaceAuto) {
        let n = cut.connectors.iter().filter(|c| c.auto).count();
        let text = format!(
            "Replace the {n} {} placed automatically, or add more beside them?",
            if n == 1 { "connector" } else { "connectors" }
        );
        ui.add_space(4.);
        match prompt_box(ui, &text, &["Replace", "Add more", "Cancel"]) {
            Some(0) => out.auto = Some(Some(true)),
            Some(1) => out.auto = Some(Some(false)),
            Some(_) => out.dismiss = true,
            None => {}
        }
    }
    if let Some((note, warn)) = &cut.note {
        ui.add(
            egui::Label::new(
                egui::RichText::new(note)
                    .font(theme::sans(11.))
                    .color(if *warn { p.warn_text } else { p.muted }),
            )
            .wrap(),
        );
    }
}
