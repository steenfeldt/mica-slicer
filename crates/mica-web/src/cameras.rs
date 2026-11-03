// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Printer cameras for the browser: one `mica_camera::Player` per printer, started by the
//! first viewer and stopped when the last one leaves, its frames re-encoded once as JPEG and
//! sent to every viewer as MJPEG (`multipart/x-mixed-replace`). The camera's address and
//! login stay on the server; the browser only ever sees this server's URL.

use crate::fleet::{Entry, lock};
use crate::http::Response;
use mica_camera::{Frame, Player, Rate};
use mica_printers::camera::{self, Role};
use std::{
    io::Write,
    net::TcpStream,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

/// What the browser gets: at most this many frames a second, at most this wide.
const RATE: Rate = Rate {
    fps: 10.,
    max_width: 1280,
};
const JPEG_QUALITY: u8 = 80;
/// The newest frame is sent again after this long without a new one, so a viewer that left
/// is noticed (the write fails) even when the camera is stuck.
const KEEPALIVE: Duration = Duration::from_secs(5);
/// How long a new viewer waits for the printer to report its cameras.
const DISCOVERY: Duration = Duration::from_secs(8);
const BOUNDARY: &str = "micaframe";

/// One printer's camera, shared by its viewers.
#[derive(Default)]
pub struct Hub {
    state: Mutex<HubState>,
    cond: Condvar,
}

#[derive(Default)]
struct HubState {
    player: Option<Player>,
    viewers: usize,
    /// The newest frame as JPEG, with the player's frame number.
    jpeg: Option<(u64, Arc<Vec<u8>>)>,
    error: Option<String>,
}

impl Hub {
    /// Whether a player is running (tests: it stops with the last viewer).
    pub fn playing(&self) -> bool {
        lock(&self.state).player.is_some()
    }
    pub fn viewers(&self) -> usize {
        lock(&self.state).viewers
    }
}

/// RGBA → JPEG.
pub fn encode(frame: &Frame) -> Option<Vec<u8>> {
    let rgb: Vec<u8> = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(
            &rgb,
            frame.width,
            frame.height,
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(out)
}

/// A viewer leaves: the last one stops the player.
struct Leave<'a>(&'a Hub);
impl Drop for Leave<'_> {
    fn drop(&mut self) {
        let mut st = lock(&self.0.state);
        st.viewers = st.viewers.saturating_sub(1);
        if st.viewers == 0 {
            st.player = None;
            st.jpeg = None;
            st.error = None;
        }
    }
}

/// Serves the camera of `entry` as MJPEG on `stream` until the viewer leaves. Returns a
/// response instead when there is nothing to show.
pub fn serve(entry: &Arc<Entry>, stream: &mut TcpStream) -> Option<Response> {
    entry.watch();
    let manual = entry
        .printer
        .camera
        .as_deref()
        .is_some_and(|u| !u.trim().is_empty());
    let deadline = Instant::now() + DISCOVERY;
    let choice = loop {
        let reported = lock(&entry.live).cameras.clone();
        match camera::choose(&entry.printer, reported.as_deref(), Role::Focused) {
            Err(camera::NoCamera::Unknown) if !manual && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(c) => break c,
            Err(camera::NoCamera::Unknown) => {
                return Some(Response::error(
                    404,
                    "The printer has not said which cameras it has yet.",
                ));
            }
            Err(camera::NoCamera::None) => {
                return Some(Response::error(404, "This printer has no camera."));
            }
            Err(camera::NoCamera::Unsupported(why) | camera::NoCamera::BadUrl(why)) => {
                return Some(Response::error(404, &why));
            }
        }
    };
    let hub = entry.camera.clone();
    {
        let mut st = lock(&hub.state);
        if st.player.is_none() {
            let started = camera::source(
                &choice,
                || entry.connector.clone(),
                || entry.camera_auth.clone(),
            )
            .and_then(|(source, auth)| {
                let weak = Arc::downgrade(&hub);
                Player::start(
                    source,
                    auth,
                    choice.orientation,
                    choice.limit(RATE),
                    move || {
                        if let Some(h) = weak.upgrade() {
                            h.cond.notify_all();
                        }
                    },
                )
            });
            match started {
                Ok(p) => {
                    st.player = Some(p);
                    st.error = None;
                }
                Err(e) => {
                    return Some(Response::error(
                        502,
                        &format!("The camera did not start: {e}"),
                    ));
                }
            }
        }
        st.viewers += 1;
    }
    let _leave = Leave(&hub);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary={BOUNDARY}\r\nCache-Control: no-store\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return None;
    }
    let mut sent_seq = 0u64;
    let mut last_sent = Instant::now();
    loop {
        entry.watch();
        // Take a new frame (whichever viewer gets it encodes it for all).
        let frame = {
            let st = lock(&hub.state);
            let p = st.player.as_ref()?;
            p.touch();
            p.take_frame()
        };
        if let Some(f) = frame
            && let Some(jpeg) = encode(&f)
        {
            let mut st = lock(&hub.state);
            if st.jpeg.as_ref().is_none_or(|(s, _)| *s < f.seq) {
                st.jpeg = Some((f.seq, Arc::new(jpeg)));
            }
            hub.cond.notify_all();
        }
        let newest = {
            let mut st = lock(&hub.state);
            st.error = st.player.as_ref().and_then(|p| p.status().error);
            st.jpeg.clone()
        };
        let fresh = newest.as_ref().is_some_and(|(s, _)| *s > sent_seq);
        if let Some((seq, jpeg)) = newest
            && (fresh || last_sent.elapsed() >= KEEPALIVE)
        {
            let part = format!(
                "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                jpeg.len()
            );
            let ok = stream.write_all(part.as_bytes()).is_ok()
                && stream.write_all(&jpeg).is_ok()
                && stream.write_all(b"\r\n").is_ok()
                && stream.flush().is_ok();
            if !ok {
                return None;
            }
            sent_seq = seq;
            last_sent = Instant::now();
            continue;
        }
        if last_sent.elapsed() >= KEEPALIVE && probe(stream).is_err() {
            return None;
        }
        let st = lock(&hub.state);
        let _ = hub
            .cond
            .wait_timeout(st, Duration::from_millis(500))
            .map(|(g, _)| drop(g));
    }
}

/// No picture for a while: whether the viewer is still there, without writing (a closed
/// connection peeks as end of file or an error).
fn probe(stream: &TcpStream) -> std::io::Result<()> {
    stream.set_nonblocking(true)?;
    let mut b = [0u8; 1];
    let r = stream.peek(&mut b);
    stream.set_nonblocking(false)?;
    match r {
        Ok(0) => Err(std::io::ErrorKind::UnexpectedEof.into()),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e),
    }
}

/// The camera's last problem, for the printer card.
pub fn status_error(hub: &Hub) -> Option<String> {
    lock(&hub.state).error.clone()
}
