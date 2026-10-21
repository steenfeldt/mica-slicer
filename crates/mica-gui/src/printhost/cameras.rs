// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The cameras the Printers dashboard and the Machine view show: which camera of a printer
//! to play and how (its manual URL first, else what the printer reports), and the players
//! behind the pictures. A player runs only while its picture is on screen: every frame the
//! views that were drawn are marked, and the rest are dropped (which stops their streams).
//! Decoding happens on the players' threads; the UI only uploads the newest frame into a
//! texture.

use super::{config::Printer, monitor::Connector};
use eframe::egui;
use mica_camera::{Orientation, Player, Rate, Source};
use mica_printhost::{CameraInfo, CameraService, PrinterHost};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

/// Printer camera pictures are at most this large when fetched through the printer's API.
const PRINTER_SNAPSHOT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// A dashboard tile: a low frame rate, downscaled.
    Thumbnail,
    /// The Machine view: full rate and size.
    Focused,
}

/// How to get a printer's pictures.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// A camera URL (manual, or reported without needing the printer's credentials).
    Url(String, Kind),
    /// A snapshot path on the printer's own API (PrusaLink), fetched with its credentials.
    PrinterApi(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mjpeg,
    Snapshot,
    Rtsp,
    /// camera-streamer's WebRTC.
    WebRtc,
}

/// The camera to show, with the words the UI needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub plan: Plan,
    pub orientation: Orientation,
    /// "U1 camera · MJPEG", "Camera URL · RTSP".
    pub label: String,
    /// Why this is not the best the camera offers ("WebRTC is not supported yet; showing
    /// snapshots").
    pub note: Option<String>,
    /// The manual URL's login applies (never sent to a camera the printer reported).
    pub manual: bool,
    /// The frame rate the printer configured for this camera (Moonraker's `target_fps`):
    /// Mica asks for no more than this.
    pub target_fps: Option<f32>,
    /// For WebRTC: the snapshot URL shown when WebRTC does not connect.
    pub fallback: Option<String>,
}
impl Choice {
    /// `rate`, no faster than the camera's configured frame rate.
    pub fn limit(&self, rate: Rate) -> Rate {
        match self.target_fps {
            Some(t) if t > 0. && t < rate.fps => Rate { fps: t, ..rate },
            _ => rate,
        }
    }
}

/// Why there is nothing to show.
#[derive(Clone, Debug, PartialEq)]
pub enum NoCamera {
    /// The printer has not been asked yet.
    Unknown,
    /// The printer reports none and no URL was entered.
    None,
    /// The printer reports a camera Mica cannot play and that has no snapshot.
    Unsupported(String),
    /// The manual URL is not usable.
    BadUrl(String),
}

fn kind_of(source: &Source) -> Kind {
    match source {
        Source::Rtsp(_) => Kind::Rtsp,
        Source::WebRtc(..) => Kind::WebRtc,
        Source::Snapshot(_) | Source::Fetch(_) => Kind::Snapshot,
        Source::Mjpeg(_) => Kind::Mjpeg,
    }
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Mjpeg => "MJPEG",
        Kind::Snapshot => "snapshots",
        Kind::Rtsp => "RTSP",
        Kind::WebRtc => "WebRTC",
    }
}

/// Moonraker WebRTC services Mica plays: camera-streamer's (go2rtc, MediaMTX and Janus
/// signal differently and show snapshots).
fn plays_webrtc(service_name: &str) -> bool {
    service_name.eq_ignore_ascii_case("webrtc-camerastreamer")
}

/// Decides what to play for `printer` in `role`, from its manual URL or the cameras it
/// reports (`None`: not asked yet). Thumbnails prefer a snapshot URL (cheap), the focused
/// view the stream.
pub fn choose(
    printer: &Printer,
    reported: Option<&[CameraInfo]>,
    role: Role,
) -> Result<Choice, NoCamera> {
    if let Some(url) = printer.camera.as_deref().filter(|u| !u.trim().is_empty()) {
        let source = Source::from_url(url).map_err(NoCamera::BadUrl)?;
        let kind = kind_of(&source);
        return Ok(Choice {
            plan: Plan::Url(url.trim().to_string(), kind),
            orientation: Orientation::default(),
            label: format!("Camera URL · {}", kind_label(kind)),
            note: None,
            manual: true,
            target_fps: None,
            fallback: None,
        });
    }
    let reported = reported.ok_or(NoCamera::Unknown)?;
    let Some(c) = reported.first() else {
        return Err(NoCamera::None);
    };
    let orientation = Orientation {
        rotation: c.rotation,
        flip_horizontal: c.flip_horizontal,
        flip_vertical: c.flip_vertical,
    };
    let choice = |plan: Plan, note: Option<String>| {
        let how = match &plan {
            Plan::Url(_, k) => kind_label(*k),
            Plan::PrinterApi(_) => "snapshots",
        };
        let fallback = match &plan {
            Plan::Url(_, Kind::WebRtc) => c.snapshot_url.clone(),
            _ => None,
        };
        Ok(Choice {
            label: format!("{} · {how}", c.name),
            plan,
            orientation,
            note,
            manual: false,
            target_fps: c.target_fps,
            fallback,
        })
    };
    if let Some(path) = &c.printer_snapshot {
        return choice(Plan::PrinterApi(path.clone()), None);
    }
    let snapshot = c.snapshot_url.clone();
    let stream = match (c.service, &c.stream_url) {
        (CameraService::Mjpeg, Some(u)) => Some(Plan::Url(u.clone(), Kind::Mjpeg)),
        (CameraService::Rtsp, Some(u)) => Some(Plan::Url(u.clone(), Kind::Rtsp)),
        (CameraService::WebRtc, Some(u)) if plays_webrtc(&c.service_name) => {
            Some(Plan::Url(u.clone(), Kind::WebRtc))
        }
        _ => None,
    };
    let unsupported = match c.service {
        CameraService::WebRtc => !plays_webrtc(&c.service_name),
        CameraService::Hls | CameraService::Other => true,
        _ => false,
    };
    let note = unsupported.then(|| {
        format!(
            "{} ({}) is not supported yet; showing snapshots",
            c.service.label(),
            c.service_name
        )
    });
    match (role, stream, snapshot) {
        (Role::Thumbnail, _, Some(s)) => choice(Plan::Url(s, Kind::Snapshot), None),
        (_, Some(stream), _) => choice(stream, None),
        (Role::Focused, None, Some(s)) => choice(Plan::Url(s, Kind::Snapshot), note),
        (_, None, None) => Err(NoCamera::Unsupported(format!(
            "{} streams {} ({}), which Mica cannot show yet, and offers no snapshot",
            c.name,
            c.service.label(),
            c.service_name
        ))),
    }
}

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
    let (source, auth) = match &choice.plan {
        Plan::Url(url, kind) => {
            let source = match kind {
                Kind::Mjpeg => Source::Mjpeg(url.clone()),
                Kind::Snapshot => Source::Snapshot(url.clone()),
                Kind::Rtsp => Source::Rtsp(url.clone()),
                Kind::WebRtc => Source::WebRtc(url.clone(), choice.fallback.clone()),
            };
            // A login typed for the manual URL goes to that URL only.
            let auth = if choice.manual { auth()? } else { None };
            (source, auth)
        }
        Plan::PrinterApi(path) => {
            let connect = connector()?;
            let host: Arc<Mutex<Option<Box<dyn PrinterHost>>>> = Arc::default();
            let path = path.clone();
            let fetch: mica_camera::FetchFn = Arc::new(move || {
                let mut h = host.lock().unwrap();
                if h.is_none() {
                    *h = Some(connect()?);
                }
                let result = h
                    .as_ref()
                    .expect("connected")
                    .camera_snapshot(&path, PRINTER_SNAPSHOT_LIMIT);
                match result {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        // Reconnect next time (a new key, a restarted printer).
                        *h = None;
                        Err(e.message)
                    }
                }
            });
            (Source::Fetch(fetch), None)
        }
    };
    Player::start(source, auth, choice.orientation, rate, wake)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printhost::config::Kind as PrinterKind;

    fn cam(service: CameraService, stream: Option<&str>, snapshot: Option<&str>) -> CameraInfo {
        CameraInfo {
            name: "Cam".into(),
            service,
            service_name: match service {
                CameraService::WebRtc => "webrtc-camerastreamer".into(),
                _ => "mjpegstreamer".into(),
            },
            stream_url: stream.map(str::to_string),
            snapshot_url: snapshot.map(str::to_string),
            printer_snapshot: None,
            rotation: 0,
            flip_horizontal: false,
            flip_vertical: false,
            target_fps: None,
        }
    }

    #[test]
    fn manual_urls_win_thumbnails_prefer_snapshots_and_webrtc_falls_back() {
        let mut p = Printer::new(PrinterKind::SnapmakerU1);
        let mj = [cam(
            CameraService::Mjpeg,
            Some("http://h/stream"),
            Some("http://h/snap"),
        )];
        assert_eq!(
            choose(&p, Some(&mj), Role::Thumbnail).unwrap().plan,
            Plan::Url("http://h/snap".into(), Kind::Snapshot)
        );
        assert_eq!(
            choose(&p, Some(&mj), Role::Focused).unwrap().plan,
            Plan::Url("http://h/stream".into(), Kind::Mjpeg)
        );
        let rtc = [cam(
            CameraService::WebRtc,
            Some("http://h/webrtc"),
            Some("http://h/snap"),
        )];
        // camera-streamer's WebRTC plays live in the Machine view, falling back to the
        // snapshot; thumbnails keep the snapshot.
        let c = choose(&p, Some(&rtc), Role::Focused).unwrap();
        assert_eq!(c.plan, Plan::Url("http://h/webrtc".into(), Kind::WebRtc));
        assert_eq!(c.fallback.as_deref(), Some("http://h/snap"));
        assert_eq!(c.label, "Cam · WebRTC");
        assert!(c.note.is_none());
        assert_eq!(
            choose(&p, Some(&rtc), Role::Thumbnail).unwrap().plan,
            Plan::Url("http://h/snap".into(), Kind::Snapshot)
        );
        // Other WebRTC services (go2rtc, MediaMTX, Janus) show snapshots, saying why.
        let mut go2rtc = rtc.clone();
        go2rtc[0].service_name = "webrtc-go2rtc".into();
        let g = choose(&p, Some(&go2rtc), Role::Focused).unwrap();
        assert_eq!(g.plan, Plan::Url("http://h/snap".into(), Kind::Snapshot));
        assert!(g.note.as_ref().unwrap().contains("webrtc-go2rtc"));
        // The camera's configured frame rate caps what Mica asks for.
        assert_eq!(c.limit(Rate::FOCUSED).fps, Rate::FOCUSED.fps);
        let slow = Choice {
            target_fps: Some(2.),
            ..c
        };
        assert_eq!(slow.limit(Rate::FOCUSED).fps, 2.);
        assert_eq!(slow.limit(Rate::THUMBNAIL).fps, 1.);
        let mut rtc_only = [cam(CameraService::WebRtc, Some("http://h/webrtc"), None)];
        assert_eq!(
            choose(&p, Some(&rtc_only), Role::Focused).unwrap().plan,
            Plan::Url("http://h/webrtc".into(), Kind::WebRtc)
        );
        rtc_only[0].service_name = "webrtc-mediamtx".into();
        assert!(matches!(
            choose(&p, Some(&rtc_only), Role::Focused),
            Err(NoCamera::Unsupported(_))
        ));
        assert_eq!(choose(&p, None, Role::Focused), Err(NoCamera::Unknown));
        assert_eq!(choose(&p, Some(&[]), Role::Focused), Err(NoCamera::None));
        // The manual URL wins over everything the printer reports.
        p.camera = Some("rtsp://127.0.0.1:8554/cam".into());
        let c = choose(&p, Some(&mj), Role::Thumbnail).unwrap();
        assert_eq!(
            c.plan,
            Plan::Url("rtsp://127.0.0.1:8554/cam".into(), Kind::Rtsp)
        );
        assert!(c.manual);
        p.camera = Some("ftp://x".into());
        assert!(matches!(
            choose(&p, None, Role::Focused),
            Err(NoCamera::BadUrl(_))
        ));
    }
}
