// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The macOS system menu bar, built with `muda` from the one menu description in
//! [`super::menus`]. A thin layer: it builds the items, turns clicks into [`Action`]s for the
//! egui loop to carry out, and mirrors enabled / checked / label state, calling a `muda` setter
//! only when a value changed. Nothing here decides what an item does or when it is enabled.

use super::{Action, Entry, Kind, Menu, Native, Role, State};
use crate::keymap::Mods;
use eframe::egui::{self, Key};
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use std::path::PathBuf;
use std::sync::mpsc;

enum Widget {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

struct Slot {
    action: Action,
    label: String,
    widget: Widget,
    /// What the item shows now; `None` forces the next refresh to set everything.
    shown: Option<State>,
}

pub struct NativeMenu {
    /// Kept alive: dropping it would leave AppKit with dangling items.
    _bar: muda::Menu,
    _submenus: Vec<Submenu>,
    slots: Vec<Slot>,
    recent: Submenu,
    /// The recent files and their enabled state the submenu shows.
    recent_shown: Option<(Vec<PathBuf>, bool)>,
    events: mpsc::Receiver<MenuId>,
}

const RECENT: &str = "recent:";
const CLEAR_RECENT: &str = "recent:clear";

impl NativeMenu {
    /// Builds the menu bar and installs it as the application's main menu. Must run on the main
    /// thread once the application exists (eframe's app creator does).
    pub fn new(ctx: &egui::Context, menus: &[Menu]) -> Result<Self, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Self::build(ctx, menus)))
            .unwrap_or_else(|_| Err("muda panicked while building the menu".into()))
    }

    fn build(ctx: &egui::Context, menus: &[Menu]) -> Result<Self, String> {
        let err = |e: muda::Error| e.to_string();
        let bar = muda::Menu::new();
        let mut slots = vec![];
        let mut submenus = vec![];
        let mut recent = None;
        let mut window = None;
        let mut help = None;
        for menu in menus {
            let sub = Submenu::new(menu.title, true);
            for entry in &menu.entries {
                match entry {
                    Entry::Item(item) => {
                        let id = MenuId::new(format!("item:{}", slots.len()));
                        let accel = super::native_chord(item.action).and_then(accelerator);
                        let widget = match item.kind {
                            Kind::Button => {
                                let w = MenuItem::with_id(id, &item.label, true, accel);
                                sub.append(&w).map_err(err)?;
                                Widget::Plain(w)
                            }
                            Kind::Check | Kind::Radio => {
                                let w = CheckMenuItem::with_id(id, &item.label, true, false, accel);
                                sub.append(&w).map_err(err)?;
                                Widget::Check(w)
                            }
                        };
                        slots.push(Slot {
                            action: item.action,
                            label: item.label.clone(),
                            widget,
                            shown: None,
                        });
                    }
                    Entry::Separator => {
                        sub.append(&PredefinedMenuItem::separator()).map_err(err)?;
                    }
                    Entry::Note(text) => {
                        sub.append(&MenuItem::new(*text, false, None))
                            .map_err(err)?;
                    }
                    Entry::Recent => {
                        let r = Submenu::new("Open recent", true);
                        sub.append(&r).map_err(err)?;
                        recent = Some(r);
                    }
                    Entry::Native(n) => {
                        sub.append(&predefined(*n)).map_err(err)?;
                    }
                }
            }
            bar.append(&sub).map_err(err)?;
            match menu.role {
                Role::Window => window = Some(sub.clone()),
                Role::Help => help = Some(sub.clone()),
                Role::App | Role::Normal => {}
            }
            submenus.push(sub);
        }
        let recent = recent.ok_or("the description has no Open recent submenu")?;
        // Clicks arrive on the main thread while the app may be idle: queue them and wake egui.
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id);
            wake.request_repaint();
        }));
        bar.init_for_nsapp();
        if let Some(w) = window {
            w.set_as_windows_menu_for_nsapp();
        }
        if let Some(h) = help {
            h.set_as_help_menu_for_nsapp();
        }
        Ok(Self {
            _bar: bar,
            _submenus: submenus,
            slots,
            recent,
            recent_shown: None,
            events: rx,
        })
    }

    /// The items chosen since the last call, oldest first.
    pub fn events(&mut self) -> Vec<Action> {
        let mut out = vec![];
        while let Ok(id) = self.events.try_recv() {
            let id = id.as_ref();
            if id == CLEAR_RECENT {
                out.push(Action::ClearRecent);
            } else if let Some(i) = id.strip_prefix(RECENT).and_then(|i| i.parse().ok()) {
                out.push(Action::Recent(i));
            } else if let Some(slot) = id
                .strip_prefix("item:")
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|i| self.slots.get(i))
            {
                out.push(slot.action);
            }
        }
        if !out.is_empty() {
            self.invalidate_checks();
        }
        out
    }

    /// Mirrors each item's state; a `muda` setter runs only for a value that changed.
    pub fn refresh(&mut self, state: impl Fn(Action) -> State, recent: &[PathBuf]) {
        for slot in &mut self.slots {
            let new = state(slot.action);
            let old = slot.shown.as_ref();
            if old == Some(&new) {
                continue;
            }
            let enabled = old.is_none_or(|o| o.enabled != new.enabled);
            let checked = old.is_none_or(|o| o.checked != new.checked);
            let label = old.is_none_or(|o| o.label != new.label);
            let text = new.label.as_deref().unwrap_or(&slot.label);
            match &slot.widget {
                Widget::Plain(w) => {
                    if enabled {
                        w.set_enabled(new.enabled);
                    }
                    if label {
                        w.set_text(text);
                    }
                }
                Widget::Check(w) => {
                    if enabled {
                        w.set_enabled(new.enabled);
                    }
                    if checked {
                        w.set_checked(new.checked);
                    }
                    if label {
                        w.set_text(text);
                    }
                }
            }
            slot.shown = Some(new);
        }
        let enabled = state(Action::Recent(0)).enabled;
        if self
            .recent_shown
            .as_ref()
            .is_none_or(|(r, e)| r != recent || *e != enabled)
        {
            self.rebuild_recent(recent, enabled);
            self.recent_shown = Some((recent.to_vec(), enabled));
        }
    }

    /// After a click AppKit has flipped a check item itself: forget what every check item
    /// shows so the next refresh puts Mica's state back.
    fn invalidate_checks(&mut self) {
        for slot in &mut self.slots {
            if matches!(slot.widget, Widget::Check(_)) {
                slot.shown = None;
            }
        }
    }

    fn rebuild_recent(&self, recent: &[PathBuf], enabled: bool) {
        while self.recent.remove_at(0).is_some() {}
        if recent.is_empty() {
            let _ = self
                .recent
                .append(&MenuItem::new("No recent files", false, None));
            return;
        }
        for (i, path) in recent.iter().enumerate() {
            let item = MenuItem::with_id(
                MenuId::new(format!("{RECENT}{i}")),
                super::recent_label(path),
                enabled,
                None,
            );
            let _ = self.recent.append(&item);
        }
        let _ = self.recent.append(&PredefinedMenuItem::separator());
        let _ = self
            .recent
            .append(&MenuItem::with_id(CLEAR_RECENT, "Clear list", true, None));
    }
}

fn predefined(n: Native) -> PredefinedMenuItem {
    match n {
        Native::Services => PredefinedMenuItem::services(None),
        Native::Hide => PredefinedMenuItem::hide(Some("Hide Mica")),
        Native::HideOthers => PredefinedMenuItem::hide_others(None),
        Native::ShowAll => PredefinedMenuItem::show_all(None),
        Native::Minimize => PredefinedMenuItem::minimize(None),
        Native::Zoom => PredefinedMenuItem::maximize(Some("Zoom")),
        Native::Fullscreen => PredefinedMenuItem::fullscreen(None),
        Native::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
    }
}

/// A keymap chord as a `muda` accelerator (the keys menu items use).
fn accelerator((key, mods): (Key, Mods)) -> Option<Accelerator> {
    let code = match key {
        Key::A => Code::KeyA,
        Key::B => Code::KeyB,
        Key::C => Code::KeyC,
        Key::D => Code::KeyD,
        Key::E => Code::KeyE,
        Key::F => Code::KeyF,
        Key::G => Code::KeyG,
        Key::H => Code::KeyH,
        Key::I => Code::KeyI,
        Key::J => Code::KeyJ,
        Key::K => Code::KeyK,
        Key::L => Code::KeyL,
        Key::M => Code::KeyM,
        Key::N => Code::KeyN,
        Key::O => Code::KeyO,
        Key::P => Code::KeyP,
        Key::Q => Code::KeyQ,
        Key::R => Code::KeyR,
        Key::S => Code::KeyS,
        Key::T => Code::KeyT,
        Key::U => Code::KeyU,
        Key::V => Code::KeyV,
        Key::W => Code::KeyW,
        Key::X => Code::KeyX,
        Key::Y => Code::KeyY,
        Key::Z => Code::KeyZ,
        Key::Num0 => Code::Digit0,
        Key::Num1 => Code::Digit1,
        Key::Num2 => Code::Digit2,
        Key::Num3 => Code::Digit3,
        Key::Num4 => Code::Digit4,
        Key::Num5 => Code::Digit5,
        Key::Num6 => Code::Digit6,
        Key::Num7 => Code::Digit7,
        Key::Num8 => Code::Digit8,
        Key::Num9 => Code::Digit9,
        Key::Comma => Code::Comma,
        Key::Backspace => Code::Backspace,
        Key::Delete => Code::Delete,
        _ => return None,
    };
    let mut m = Modifiers::empty();
    for (k, v) in [
        (Mods::CMD, Modifiers::SUPER),
        (Mods::SHIFT, Modifiers::SHIFT),
        (Mods::ALT, Modifiers::ALT),
        (Mods::CTRL, Modifiers::CONTROL),
    ] {
        if mods.has(k) {
            m |= v;
        }
    }
    Some(Accelerator::new(Some(m), code))
}

/// An Edit item chosen while a text field has the keyboard: the field gets it, as egui's own
/// key handling would have (the menu took the key press).
pub fn forward_to_text_field(ctx: &egui::Context, action: Action) {
    use crate::keymap::Act;
    let cmd = egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND;
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let event = match action {
        Action::Act(Act::Undo) => key(Key::Z, cmd),
        Action::Act(Act::Redo) => key(Key::Z, cmd | egui::Modifiers::SHIFT),
        Action::Act(Act::SelectAll) => key(Key::A, cmd),
        Action::Act(Act::CutToClipboard) => egui::Event::Cut,
        Action::Act(Act::Copy) => egui::Event::Copy,
        Action::Act(Act::Paste) => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => egui::Event::Paste(text),
            Err(e) => {
                eprintln!("mica-gui: paste: the clipboard could not be read ({e})");
                return;
            }
        },
        _ => return,
    };
    ctx.input_mut(|i| i.events.push(event));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_native_chord_has_an_accelerator() {
        for &chord in super::super::native_chords() {
            assert!(accelerator(chord).is_some(), "{chord:?}");
        }
    }
}
