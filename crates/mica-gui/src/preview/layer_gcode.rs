// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Per-layer custom G-code on the layer slider: the "+" button beside the upper handle (adds a
//! colour change; right-click for pause, custom G-code, template, tool changes), the "×" that
//! replaces it on a layer that already has an item, the markers on the track with their
//! tooltips, and the small pause / custom G-code editors. The slider never edits the plate: it
//! reports [`Edit`]s, which the editor applies as undo steps.

use super::color_print::{layer_for_z, next_color, next_tool, to_hex};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::custom_gcode::{Info, Item, Kind, Mode};

/// What the slider knows about the plate's custom G-code and the printer.
pub struct LayerGCode<'a> {
    pub info: &'a Info,
    /// The printer's tool count.
    pub tools: usize,
    /// Distinct tools the plate's objects print with.
    pub object_tools: usize,
    /// The lowest tool the objects print with (zero based).
    pub base_tool: usize,
    /// Klipper / Orca flavour (the U1): colour changes are tool changes, as in Orca.
    pub klipper: bool,
    /// The printer has a `template_custom_gcode`.
    pub has_template: bool,
    pub tool_colors: &'a [[f32; 3]],
}

/// A change the slider asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Insert or replace the item at its height (the plate's mode becomes `mode`).
    Put {
        item: Item,
        mode: Mode,
        label: &'static str,
    },
    /// Remove the item at this height.
    Remove { print_z: f64 },
}

/// Popup state, kept in the preview's view so headless screenshots can open it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerUi {
    /// The "+" menu is open.
    pub menu: bool,
    pub dialog: Option<Dialog>,
    /// A `+` (true) or `−` (false) key press for the upper handle's layer, applied by the
    /// slider on its next frame (see `keymap`).
    pub key: Option<bool>,
}

/// The pause message or custom G-code editor.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub kind: Kind,
    pub print_z: f64,
    pub text: String,
}

impl LayerGCode<'_> {
    pub fn mode(&self) -> Mode {
        Mode::of_print(self.tools, self.object_tools)
    }
    fn tool_hex(&self, tool: usize) -> String {
        to_hex(
            self.tool_colors
                .get(tool)
                .copied()
                .unwrap_or_else(|| super::legend::tool_color(tool)),
        )
    }
    /// The tool printing at `z` on a plate whose objects share one tool (after tool changes).
    pub fn tool_at(&self, z: f64) -> usize {
        self.info
            .items
            .iter()
            .rev()
            .find(|i| i.kind == Kind::ToolChange && i.print_z < z - 1e-6)
            .map_or(self.base_tool, |i| i.extruder.saturating_sub(1))
            .min(self.tools.saturating_sub(1))
    }
    /// The colour printing just below `z` on a single-tool printer.
    fn color_at(&self, z: f64) -> String {
        self.info
            .items
            .iter()
            .rev()
            .find(|i| {
                matches!(i.kind, Kind::ColorChange | Kind::ToolChange)
                    && i.print_z < z - 1e-6
                    && !i.color.is_empty()
            })
            .map_or_else(|| self.tool_hex(0), |i| i.color.clone())
    }
    /// A colour change (M600) at `z` for `tool` (zero based), in a new colour.
    pub fn color_change(&self, z: f64, tool: usize) -> Item {
        Item {
            color: next_color(self.info, &self.color_at(z)),
            ..Item::new(z, Kind::ColorChange, tool + 1)
        }
    }
    /// Switch the plate to `tool` (zero based) from `z` on.
    pub fn tool_change(&self, z: f64, tool: usize) -> Item {
        Item {
            color: self.tool_hex(tool),
            ..Item::new(z, Kind::ToolChange, tool + 1)
        }
    }
    /// What the "+" adds: a colour change on a single-tool printer; on a tool changer the next
    /// tool when every object prints with one tool (Orca's "Change filament"), else a colour
    /// change of the objects' first tool. Orca has no M600 colour change for several
    /// tools, so a Klipper plate with several object tools gets a pause instead.
    pub fn default_item(&self, z: f64) -> (Item, &'static str) {
        if self.tools <= 1 {
            return (self.color_change(z, 0), "Add color change");
        }
        match self.mode() {
            Mode::MultiAsSingle => (
                self.tool_change(z, next_tool(self.tool_at(z), self.tools)),
                "Add tool change",
            ),
            _ if self.klipper => (
                Item {
                    extra: "Change filament".into(),
                    ..Item::new(z, Kind::PausePrint, self.base_tool + 1)
                },
                "Add pause",
            ),
            _ => (self.color_change(z, self.base_tool), "Add color change"),
        }
    }
}

fn marker_color(item: &Item, g: &LayerGCode<'_>) -> Color32 {
    match item.kind {
        Kind::ColorChange | Kind::ToolChange => mica_core::color::parse_hex_f32(&item.color)
            .map(theme::to_color32)
            .unwrap_or_else(|| {
                theme::to_color32(super::legend::tool_color(item.extruder.saturating_sub(1)))
            }),
        Kind::PausePrint => theme::WARNING,
        Kind::Template | Kind::Custom => {
            let _ = g;
            Color32::from_rgb(0xc8, 0xd0, 0xdc)
        }
    }
}

/// Draws the markers, the "+"/"×" button, its menu and the editors. `y_of` maps a layer to
/// its pixel row, `zs` are the layers' print_z. May move `high` (clicking a marker selects its
/// layer); pushes edits.
#[allow(clippy::too_many_arguments)]
pub fn ui(
    ui: &mut egui::Ui,
    bounds: Rect,
    track_x: f32,
    zs: &[f64],
    y_of: &dyn Fn(usize) -> f32,
    high: &mut usize,
    state: &mut LayerUi,
    g: &LayerGCode<'_>,
    edits: &mut Vec<Edit>,
) -> bool {
    let p = theme::of(ui);
    let painter = ui.painter_at(bounds);
    let mut changed = false;
    // Markers: a swatch right of the track per item, hover for what it does, click to go there.
    for (i, item) in g.info.items.iter().enumerate() {
        let layer = layer_for_z(zs, item.print_z);
        let y = y_of(layer);
        let r = Rect::from_center_size(Pos2::new(track_x + 11., y), Vec2::new(11., 8.));
        let c = marker_color(item, g);
        match item.kind {
            Kind::PausePrint => {
                // Two bars, a pause sign.
                for dx in [-2.5, 2.5] {
                    painter.rect_filled(
                        Rect::from_center_size(Pos2::new(r.center().x + dx, y), Vec2::new(2., 8.)),
                        0.5,
                        c,
                    );
                }
            }
            Kind::Template | Kind::Custom => {
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "G",
                    theme::semibold(11.),
                    c,
                );
            }
            _ => {
                painter.rect_filled(r, 2., c);
                painter.rect_stroke(
                    r,
                    2.,
                    Stroke::new(1., p.border_soft),
                    egui::StrokeKind::Outside,
                );
            }
        }
        painter.line_segment(
            [Pos2::new(track_x + 3., y), Pos2::new(track_x + 5., y)],
            Stroke::new(1.5, c),
        );
        let response = ui
            .interact(
                r.expand(3.),
                ui.scope_id().with(("layer gcode marker", i)),
                Sense::click(),
            )
            .on_hover_text(format!(
                "{}\nat {:.2} mm, layer {}\nClick to go to this layer",
                item.describe(),
                item.print_z,
                layer + 1
            ));
        if response.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        if response.clicked() && *high != layer {
            *high = layer;
            changed = true;
        }
    }
    let Some(&z) = zs.get(*high) else {
        return changed;
    };
    let y = y_of(*high);
    let existing = g
        .info
        .items
        .iter()
        .position(|i| layer_for_z(zs, i.print_z) == *high);
    // The "+" (or "×" on a layer that has an item) left of the upper handle.
    let center = Pos2::new(track_x - 30., y);
    let button = Rect::from_center_size(center, Vec2::splat(20.));
    let response = ui
        .interact(
            button,
            ui.scope_id().with("layer gcode add"),
            Sense::click(),
        )
        .on_hover_text(match existing {
            Some(i) => format!(
                "{} at this layer\nClick to remove it · right-click for more",
                g.info.items[i].describe()
            ),
            None => format!(
                "{} at this layer\nRight-click for a pause, custom G-code and more",
                g.default_item(z).1
            ),
        });
    let hot = response.hovered() || state.menu;
    painter.circle_filled(center, 10., if hot { p.accent_bright } else { p.accent });
    let sign = if existing.is_some() { "×" } else { "+" };
    painter.text(
        center + Vec2::new(0., -0.5),
        Align2::CENTER_CENTER,
        sign,
        theme::semibold(17.),
        Color32::WHITE,
    );
    if response.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    // The `+` key adds like a click on "+"; `−` removes the layer's item.
    let key = state.key.take();
    if key == Some(false)
        && let Some(i) = existing
    {
        edits.push(Edit::Remove {
            print_z: g.info.items[i].print_z,
        });
    }
    if response.clicked() || (key == Some(true) && existing.is_none()) {
        state.menu = false;
        match existing {
            Some(i) => edits.push(Edit::Remove {
                print_z: g.info.items[i].print_z,
            }),
            None => {
                let (item, label) = g.default_item(z);
                edits.push(Edit::Put {
                    item,
                    mode: g.mode(),
                    label,
                });
            }
        }
    }
    if response.secondary_clicked() {
        state.menu = !state.menu;
    }
    if state.menu {
        menu(ui, button, z, existing, state, g, edits);
    }
    dialog(ui, bounds, state, g, edits);
    changed
}

fn menu_id() -> egui::Id {
    egui::Id::unique("mica layer gcode menu")
}

fn menu(
    ui: &mut egui::Ui,
    button: Rect,
    z: f64,
    existing: Option<usize>,
    state: &mut LayerUi,
    g: &LayerGCode<'_>,
    edits: &mut Vec<Edit>,
) {
    let mode = g.mode();
    let area = egui::Area::new(menu_id())
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(button.left_top() + Vec2::new(-6., 0.))
        .show(ui.ctx(), |ui| {
            theme::overlay_frame(ui).show(ui, |ui| {
                ui.set_min_width(210.);
                let p = theme::of(ui);
                ui.label(
                    egui::RichText::new(format!("At {z:.2} mm"))
                        .font(theme::semibold(12.))
                        .color(p.muted),
                );
                ui.add_space(4.);
                let put = |edits: &mut Vec<Edit>, item: Item, label: &'static str| {
                    edits.push(Edit::Put { item, mode, label });
                };
                let mut close = false;
                if let Some(i) = existing {
                    let item = &g.info.items[i];
                    if matches!(item.kind, Kind::PausePrint | Kind::Custom)
                        && ui.button("Edit…").clicked()
                    {
                        state.dialog = Some(Dialog {
                            kind: item.kind,
                            print_z: item.print_z,
                            text: item.extra.clone(),
                        });
                        close = true;
                    }
                    if ui
                        .button(format!("Remove {}", kind_word(item.kind)))
                        .clicked()
                    {
                        edits.push(Edit::Remove {
                            print_z: item.print_z,
                        });
                        close = true;
                    }
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Replace with:")
                            .font(theme::sans(11.))
                            .color(p.muted),
                    );
                }
                if !g.klipper && ui.button("Add color change").clicked() {
                    let tool = if g.tools > 1 && mode == Mode::MultiAsSingle {
                        g.tool_at(z)
                    } else if g.tools > 1 {
                        g.base_tool
                    } else {
                        0
                    };
                    put(edits, g.color_change(z, tool), "Add color change");
                    close = true;
                }
                if ui.button("Add pause…").clicked() {
                    state.dialog = Some(Dialog {
                        kind: Kind::PausePrint,
                        print_z: z,
                        text: String::new(),
                    });
                    close = true;
                }
                if ui.button("Add custom G-code…").clicked() {
                    state.dialog = Some(Dialog {
                        kind: Kind::Custom,
                        print_z: z,
                        text: String::new(),
                    });
                    close = true;
                }
                if g.has_template && ui.button("Add template G-code").clicked() {
                    put(
                        edits,
                        Item::new(z, Kind::Template, g.base_tool + 1),
                        "Add template G-code",
                    );
                    close = true;
                }
                if g.tools > 1 {
                    ui.separator();
                    let enabled = mode == Mode::MultiAsSingle;
                    for t in 0..g.tools {
                        let c = g
                            .tool_colors
                            .get(t)
                            .copied()
                            .unwrap_or_else(|| super::legend::tool_color(t));
                        let r = ui
                            .add_enabled_ui(enabled, |ui| {
                                ui.horizontal(|ui| {
                                    let (sw, _) =
                                        ui.allocate_exact_size(Vec2::splat(12.), Sense::hover());
                                    ui.painter().rect_filled(sw, 3., theme::to_color32(c));
                                    ui.button(format!("Change to tool {}", t + 1))
                                })
                                .inner
                            })
                            .inner;
                        let r = if enabled {
                            r
                        } else {
                            r.on_disabled_hover_text("Tool changes need every object on one tool")
                        };
                        if r.clicked() {
                            put(edits, g.tool_change(z, t), "Add tool change");
                            close = true;
                        }
                    }
                }
                if close {
                    state.menu = false;
                }
            });
        });
    if area.response.clicked_elsewhere() && !ui.rect_contains_pointer(button) {
        state.menu = false;
    }
}

fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::ColorChange => "color change",
        Kind::ToolChange => "tool change",
        Kind::PausePrint => "pause",
        Kind::Template => "template G-code",
        Kind::Custom => "custom G-code",
    }
}

fn dialog(
    ui: &mut egui::Ui,
    bounds: Rect,
    state: &mut LayerUi,
    g: &LayerGCode<'_>,
    edits: &mut Vec<Edit>,
) {
    let Some(d) = &mut state.dialog else {
        return;
    };
    let mut result: Option<bool> = None;
    let custom = d.kind == Kind::Custom;
    let title = if custom { "Custom G-code" } else { "Pause" };
    egui::Window::new(title)
        .id(egui::Id::unique("mica layer gcode dialog"))
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .default_pos(bounds.center() - Vec2::new(200., 120.))
        .show(ui.ctx(), |ui| {
            let p = theme::of(ui);
            ui.set_width(380.);
            ui.label(
                egui::RichText::new(if custom {
                    format!(
                        "G-code to run at the start of the layer at {:.2} mm",
                        d.print_z
                    )
                } else {
                    format!(
                        "The printer pauses at the start of the layer at {:.2} mm. \
                         The message shows on its screen.",
                        d.print_z
                    )
                })
                .font(theme::sans(12.))
                .color(p.label),
            );
            ui.add_space(6.);
            let r = if custom {
                ui.add(
                    egui::TextEdit::multiline(&mut d.text)
                        .font(theme::mono(12.))
                        .code_editor()
                        .desired_rows(8)
                        .desired_width(f32::INFINITY)
                        .hint_text("M117 Hello\nG4 S5"),
                )
            } else {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut d.text)
                        .desired_width(f32::INFINITY)
                        .hint_text("Message, e.g. Insert magnets"),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    result = Some(true);
                }
                r
            };
            if !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
            ui.add_space(8.);
            ui.horizontal(|ui| {
                let ok = ui.add_enabled(
                    !custom || !d.text.trim().is_empty(),
                    egui::Button::new(if custom { "Add G-code" } else { "Add pause" }),
                );
                if ok.clicked() {
                    result = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    result = Some(false);
                }
            });
        });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        result = Some(false);
    }
    match result {
        Some(true) => {
            let d = state.dialog.take().unwrap();
            let item = Item {
                extra: if custom {
                    d.text.trim_end().to_string()
                } else {
                    d.text.trim().to_string()
                },
                ..Item::new(d.print_z, d.kind, g.base_tool + 1)
            };
            edits.push(Edit::Put {
                item,
                mode: g.mode(),
                label: if custom {
                    "Add custom G-code"
                } else {
                    "Add pause"
                },
            });
        }
        Some(false) => state.dialog = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(info: &Info, tools: usize, object_tools: usize, klipper: bool) -> LayerGCode<'_> {
        LayerGCode {
            info,
            tools,
            object_tools,
            base_tool: 0,
            klipper,
            has_template: false,
            tool_colors: &[[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        }
    }

    #[test]
    fn plus_adds_what_the_printer_can_do() {
        let info = Info::default();
        let (item, _) = context(&info, 1, 1, false).default_item(5.);
        assert_eq!(item.kind, Kind::ColorChange);
        assert_eq!(item.extruder, 1);
        assert!(!item.color.is_empty());
        // A single-tool plate on a tool changer switches to the next tool, Prusa and Orca.
        for klipper in [false, true] {
            let (item, _) = context(&info, 3, 1, klipper).default_item(5.);
            assert_eq!((item.kind, item.extruder), (Kind::ToolChange, 2));
            assert_eq!(item.color, "#00FF00");
        }
        // Objects on several tools: an M600 for the first tool, a pause on Klipper.
        assert_eq!(
            context(&info, 3, 2, false).default_item(5.).0.kind,
            Kind::ColorChange
        );
        assert_eq!(
            context(&info, 3, 2, true).default_item(5.).0.kind,
            Kind::PausePrint
        );
    }

    #[test]
    fn tool_changes_cycle_from_the_tool_printing_below() {
        let mut info = Info::default();
        info.put(Item::new(2., Kind::ToolChange, 3));
        let g = context(&info, 3, 1, false);
        assert_eq!(g.tool_at(1.), 0);
        assert_eq!(g.tool_at(2.), 0);
        assert_eq!(g.tool_at(4.), 2);
        assert_eq!(g.default_item(4.).0.extruder, 1);
        // Colours of successive single-tool colour changes differ.
        let mut info = Info::default();
        let a = context(&info, 1, 1, false).color_change(2., 0);
        info.put(a.clone());
        let b = context(&info, 1, 1, false).color_change(4., 0);
        assert_ne!(a.color, b.color);
    }
}
