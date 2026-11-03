// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The cameras the Printers dashboard and the Machine view show, and the players behind the
//! pictures. Which camera to play is decided by `mica_printers::camera` (shared with the web
//! dashboard). A player runs only while its picture is on screen: every frame the views that
//! were drawn are marked, and the rest are dropped (which stops their streams). Decoding
//! happens on the players' threads; the UI only uploads the newest frame into a texture.

use super::monitor::Connector;
use eframe::egui;
use mica_camera::{Player, Rate};
pub use mica_printers::camera::{Choice, NoCamera, Plan, Role, choose};
use std::{collections::HashMap, time::Instant};

/// One camera picture on screen.
pub struct View {
    player: Player,
    /// What the player was started with; a different choice restarts it.
    plan: Plan,
    texture: Option<egui::TextureHandle>,
    pub size: [u32; 2],
    label: String,
    note: Option<String>,
    started: Instant,
    touched: bool,
    /// The snapshot URL a WebRTC player falls back to.
    fallback: Option<String>,
}
impl View {
    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }
    pub fn status(&self) -> mica_camera::Status {
        self.player.status()
    }
    /// "U1 camera · WebRTC", or "… · snapshots" once a WebRTC player fell back.
    pub fn label(&self) -> String {
        match self.player.status().fallback {
            Some(_) => self.label.replace("· WebRTC", "· snapshots"),
            None => self.label.clone(),
        }
    }
    /// Why the picture is not the best the camera offers.
    pub fn note(&self) -> Option<String> {
        match self.player.status().fallback {
            Some(why) => Some(format!("WebRTC did not connect ({why}); showing snapshots")),
            None => self.note.clone(),
        }
    }
    /// "Connecting…" for this long before an error is worth showing.
    pub fn starting(&self) -> bool {
        self.started.elapsed().as_secs_f32() < 1.5
    }
}

/// The players on screen, by printer id and role.
#[derive(Default)]
pub struct CameraViews {
    views: HashMap<(u64, Role), View>,
    /// Why a camera could not be started (bad URL, refused address), by printer and role.
    pub refused: HashMap<(u64, Role), String>,
}
impl CameraViews {
    pub fn len(&self) -> usize {
        self.views.len()
    }
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
    /// The URLs being played (checks: all on this computer in test runs).
    pub fn urls(&self) -> Vec<String> {
        self.views
            .values()
            .flat_map(|v| match &v.plan {
                Plan::Url(u, _) => [Some(u.clone()), v.fallback.clone()],
                Plan::PrinterApi(_) => [None, None],
            })
            .flatten()
            .collect()
    }
    /// Marks the start of a frame: views not shown again before `end_frame` stop.
    pub fn begin_frame(&mut self) {
        for v in self.views.values_mut() {
            v.touched = false;
        }
    }
    pub fn end_frame(&mut self) {
        self.views.retain(|_, v| v.touched);
    }
    pub fn clear(&mut self) {
        self.views.clear();
        self.refused.clear();
    }
    /// The view of printer `id` in `role`, (re)started for `choice` when needed, with the
    /// newest frame uploaded. `connector` and `auth` are read only when a player starts.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        role: Role,
        choice: &Choice,
        connector: impl FnOnce() -> Result<Connector, String>,
        auth: impl FnOnce() -> Result<Option<mica_camera::Auth>, String>,
        rate: Rate,
    ) -> Result<&View, String> {
        let key = (id, role);
        if self.views.get(&key).is_some_and(|v| v.plan != choice.plan) {
            self.views.remove(&key);
        }
        if !self.views.contains_key(&key) {
            // A refused start is not retried every frame; a new choice clears it.
            if let Some(e) = self.refused.get(&key) {
                return Err(e.clone());
            }
            match start(ctx, choice, connector, auth, choice.limit(rate)) {
                Ok(player) => {
                    self.views.insert(
                        key,
                        View {
                            player,
                            plan: choice.plan.clone(),
                            texture: None,
                            size: [0, 0],
                            label: choice.label.clone(),
                            note: choice.note.clone(),
                            started: Instant::now(),
                            touched: true,
                            fallback: choice.fallback.clone(),
                        },
                    );
                }
                Err(e) => {
                    self.refused.insert(key, e.clone());
                    return Err(e);
                }
            }
        }
        let view = self.views.get_mut(&key).expect("inserted above");
        view.touched = true;
        // Snapshot polling pauses when these touches stop (a minimised window draws nothing).
        view.player.touch();
        view.player.set_rate(choice.limit(rate));
        if let Some(frame) = view.player.take_frame() {
            let image = egui::ColorImage::from_rgba_premultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            view.size = [frame.width, frame.height];
            match &mut view.texture {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => {
                    view.texture = Some(ctx.load_texture(
                        format!("camera-{id}-{role:?}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }
        Ok(view)
    }
    /// Width over height of the picture printer `id` shows in `role`, once a frame arrived.
    pub fn aspect(&self, id: u64, role: Role) -> Option<f32> {
        let [w, h] = self.views.get(&(id, role))?.size;
        (w > 0 && h > 0).then(|| w as f32 / h as f32)
    }
    /// Forgets refusals of printer `id` (its settings changed).
    pub fn forget(&mut self, id: u64) {
        self.refused.retain(|(i, _), _| *i != id);
        self.views.retain(|(i, _), _| *i != id);
    }
}

fn start(
    ctx: &egui::Context,
    choice: &Choice,
    connector: impl FnOnce() -> Result<Connector, String>,
    auth: impl FnOnce() -> Result<Option<mica_camera::Auth>, String>,
    rate: Rate,
) -> Result<Player, String> {
    let wake = {
        let c = ctx.clone();
        move || c.request_repaint()
    };
    let (source, auth) = mica_printers::camera::source(choice, connector, auth)?;
    Player::start(source, auth, choice.orientation, rate, wake)
}
