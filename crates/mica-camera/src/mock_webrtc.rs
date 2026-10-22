// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A mock camera-streamer (feature `mock`): the HTTP signalling of camera-streamer's
//! `/webrtc` endpoint (`request` → offer, `answer`, `remote_candidate`; see `webrtc.rs`) and a
//! WebRTC sender made with str0m that streams H.264 (the recorded test clip, or live
//! OpenH264 with the development feature) and pings over a `keepalive` data channel, plus a
//! `/snapshot.jpg`. Loopback only. For tests, the mock printers and screenshots.

use crate::mock::{Renderer, jpeg};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use str0m::{
    Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig,
    change::{SdpAnswer, SdpPendingOffer},
    channel::ChannelId,
    media::{Direction, Frequency, MediaKind, MediaTime, Mid},
    net::{Protocol, Receive},
};

/// How the mock behaves.
#[derive(Clone)]
pub struct StreamerOptions {
    pub fps: f32,
    pub render: Renderer,
    /// Drop every n-th video packet once connected (packet loss). Only first transmissions
    /// count, so which packets are lost does not depend on timing (how RTCP, data channel
    /// and retransmitted packets interleave with them).
    pub lose_every: Option<u32>,
    /// Drop the retransmissions too (with `lose_every`): every gap stays, as when the NACK
    /// round trip is lost as well. `false`: every retransmission arrives, NACKs repair all.
    pub lose_resends: bool,
    /// `false`: POST `/webrtc` answers 404 (a camera without WebRTC; snapshots still work).
    pub webrtc: bool,
}

/// What the mock saw, for tests.
#[derive(Default)]
pub struct StreamerStats {
    /// Every POST body to `/webrtc`, in order.
    pub posts: Mutex<Vec<String>>,
    /// WebRTC sessions running now.
    pub sessions: AtomicUsize,
    pub frames_sent: AtomicUsize,
    pub pongs: AtomicUsize,
    pub keyframe_requests: AtomicUsize,
    pub snapshots: AtomicUsize,
    /// When each video frame went out (all sessions).
    pub sent: Mutex<Vec<Instant>>,
}

enum Cmd {
    Answer(String, mpsc::Sender<Result<(), String>>),
    Candidate(String),
}

type Sessions = Arc<Mutex<HashMap<String, mpsc::Sender<Cmd>>>>;

/// The mock camera. Dropping it stops the server and every session.
pub struct CameraStreamer {
    pub port: u16,
    pub stats: Arc<StreamerStats>,
    stop: Arc<AtomicBool>,
}
impl Drop for CameraStreamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}
impl CameraStreamer {
    /// Binds 127.0.0.1:`port` (0: any free port).
    pub fn start(port: u16, options: StreamerOptions) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(StreamerStats::default());
        let sessions: Sessions = Arc::default();
        let (s, st) = (stop.clone(), stats.clone());
        let started = Instant::now();
        thread::Builder::new()
            .name("mock-camera-streamer".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if s.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(conn) = conn else { continue };
                    let (o, st, se, s) = (options.clone(), st.clone(), sessions.clone(), s.clone());
                    thread::spawn(move || {
                        let _ = serve(conn, &o, &st, &se, &s, started);
                    });
                }
            })?;
        Ok(Self { port, stats, stop })
    }
    /// The stream URL Moonraker would list (`/webcam/webrtc`).
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/webcam/webrtc", self.port)
    }
    pub fn snapshot_url(&self) -> String {
        format!("http://127.0.0.1:{}/webcam/snapshot.jpg", self.port)
    }
}

/// Requests on one kept-alive HTTP connection.
fn serve(
    conn: TcpStream,
    o: &StreamerOptions,
    stats: &Arc<StreamerStats>,
    sessions: &Sessions,
    stop: &Arc<AtomicBool>,
    started: Instant,
) -> std::io::Result<()> {
    let mut out = conn.try_clone()?;
    let mut r = BufReader::new(conn);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (
            parts.next().unwrap_or("").to_string(),
            parts.next().unwrap_or("").to_string(),
        );
        let mut length = 0usize;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h)? == 0 {
                return Ok(());
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length.min(1 << 20)];
        r.read_exact(&mut body)?;
        let path = path.split('?').next().unwrap_or("");
        let (code, kind, reply): (u16, &str, Vec<u8>) = match (method.as_str(), path) {
            ("GET", "/webcam/snapshot.jpg") => {
                stats.snapshots.fetch_add(1, Ordering::Relaxed);
                let t = started.elapsed().as_secs_f64();
                (200, "image/jpeg", jpeg(&(o.render)(t), 80))
            }
            ("GET", "/webcam/webrtc") => (
                200,
                "text/html",
                b"<!doctype html><title>camera-streamer (Mica mock)</title>".to_vec(),
            ),
            ("POST", "/webcam/webrtc") if o.webrtc => {
                let text = String::from_utf8_lossy(&body).into_owned();
                stats.posts.lock().unwrap().push(text.clone());
                match signal(&text, o, stats, sessions, stop) {
                    Ok(v) => (200, "application/json", v.to_string().into_bytes()),
                    Err((code, why)) => (code, "text/plain", why.into_bytes()),
                }
            }
            _ => (404, "text/plain", b"Not found".to_vec()),
        };
        write!(
            out,
            "HTTP/1.1 {code} {}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n",
            if code == 200 { "OK" } else { "Error" },
            reply.len()
        )?;
        out.write_all(&reply)?;
    }
}

/// camera-streamer's `http_webrtc_offer` dispatch.
fn signal(
    text: &str,
    o: &StreamerOptions,
    stats: &Arc<StreamerStats>,
    sessions: &Sessions,
    stop: &Arc<AtomicBool>,
) -> Result<Value, (u16, String)> {
    let v: Value = serde_json::from_str(text).map_err(|_| (400, "bad json".to_string()))?;
    let id = v["id"].as_str().unwrap_or("").to_string();
    let session = || {
        sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or((404, "No client found".to_string()))
    };
    match v["type"].as_str() {
        Some("request") => {
            let keep_alive = v["keepAlive"].as_bool().unwrap_or(false);
            let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| (500, e.to_string()))?;
            let local = socket.local_addr().map_err(|e| (500, e.to_string()))?;
            let mut rtc = RtcConfig::new()
                .set_crypto_provider(crate::webrtc::provider())
                .clear_codecs()
                .enable_h264(true)
                .build(Instant::now());
            rtc.add_local_candidate(
                Candidate::host(local, "udp").map_err(|e| (500, e.to_string()))?,
            );
            let mut api = rtc.sdp_api();
            let mid = api.add_media(
                MediaKind::Video,
                Direction::SendOnly,
                Some("stream".into()),
                Some("video".into()),
                None,
            );
            if keep_alive {
                api.add_channel("keepalive".into());
            }
            let (offer, pending) = api.apply().ok_or((500, "no offer".to_string()))?;
            static N: AtomicUsize = AtomicUsize::new(0);
            let id = format!("rtc-mock{:012}", N.fetch_add(1, Ordering::Relaxed));
            let (tx, rx) = mpsc::channel();
            sessions.lock().unwrap().insert(id.clone(), tx);
            let (o, stats, sessions, stop, sid) = (
                o.clone(),
                stats.clone(),
                sessions.clone(),
                stop.clone(),
                id.clone(),
            );
            stats.sessions.fetch_add(1, Ordering::Relaxed);
            thread::spawn(move || {
                send(rtc, socket, pending, mid, rx, &o, &stats, &stop, keep_alive);
                sessions.lock().unwrap().remove(&sid);
                stats.sessions.fetch_sub(1, Ordering::Relaxed);
            });
            Ok(json!({
                "id": id,
                "type": "offer",
                "sdp": offer.to_sdp_string(),
                "keepAlive": keep_alive,
                "iceServers": [],
            }))
        }
        Some("answer") => {
            let sdp = v["sdp"].as_str().ok_or((400, "no sdp or id".to_string()))?;
            let (tx, rx) = mpsc::channel();
            session()?
                .send(Cmd::Answer(sdp.to_string(), tx))
                .map_err(|_| (404, "No client found".to_string()))?;
            rx.recv_timeout(Duration::from_secs(3))
                .map_err(|_| (500, "timeout".to_string()))?
                .map_err(|e| (500, e))?;
            Ok(json!({}))
        }
        Some("remote_candidate") => {
            let s = session()?;
            for c in v["candidates"].as_array().into_iter().flatten() {
                if let Some(c) = c["candidate"].as_str() {
                    let _ = s.send(Cmd::Candidate(c.to_string()));
                }
            }
            Ok(json!({}))
        }
        _ => Err((400, "Not expected".into())),
    }
}

/// One WebRTC session of the mock.
struct Sender<'a> {
    rtc: Rtc,
    socket: UdpSocket,
    o: &'a StreamerOptions,
    stats: &'a StreamerStats,
    channel: Option<ChannelId>,
    last_pong: Instant,
    connected: bool,
    /// Video packets sent for the first time (for the loss option).
    packets: u32,
    /// The video payload type (retransmissions use another, RTX, or repeat a lost sequence
    /// number).
    video_pt: Option<u8>,
    /// Sequence numbers of the video packets lost, newest last (a few hundred at most).
    lost: std::collections::VecDeque<u16>,
}

/// What a datagram the mock sends is, for the loss option. SRTP leaves the RTP header in the
/// clear (RFC 3711); RFC 7983 tells RTP and RTCP from STUN, DTLS and the rest.
#[derive(Debug, PartialEq, Eq)]
enum Packet {
    Video { seq: u16 },
    Resend,
    Other,
}
fn classify(d: &[u8], video_pt: Option<u8>) -> Packet {
    // RTP/RTCP: version 2 (first byte 128..=191), 12-byte header at least.
    if d.len() < 12 || !(128..=191).contains(&d[0]) {
        return Packet::Other;
    }
    // RTCP packet types 192..=223 sit where RTP has the marker bit and payload type.
    if (192..=223).contains(&d[1]) {
        return Packet::Other;
    }
    let pt = d[1] & 0x7f;
    match video_pt {
        Some(v) if v == pt => Packet::Video {
            seq: u16::from_be_bytes([d[2], d[3]]),
        },
        // Another payload type on the video stream: RTX (retransmissions, padding).
        Some(_) => Packet::Resend,
        None => Packet::Other,
    }
}

impl Sender<'_> {
    /// Whether the loss option drops this datagram.
    fn lose(&mut self, d: &[u8], every: u32) -> bool {
        match classify(d, self.video_pt) {
            Packet::Video { seq } if self.lost.contains(&seq) => self.o.lose_resends,
            Packet::Video { seq } => {
                self.packets += 1;
                if !self.packets.is_multiple_of(every) {
                    return false;
                }
                if self.lost.len() >= 512 {
                    self.lost.pop_front();
                }
                self.lost.push_back(seq);
                true
            }
            Packet::Resend => self.o.lose_resends,
            Packet::Other => false,
        }
    }
    /// Drains str0m's output; `Err` when the session is over.
    fn drain(&mut self) -> Result<Instant, ()> {
        loop {
            match self.rtc.poll_output().map_err(|_| ())? {
                Output::Timeout(t) => return Ok(t),
                Output::Transmit(t) => {
                    // Loss: every n-th video packet once connected (ICE and DTLS are done).
                    if self.connected
                        && let Some(n) = self.o.lose_every
                        && self.lose(&t.contents, n)
                    {
                        continue;
                    }
                    let _ = self.socket.send_to(&t.contents, t.destination);
                }
                Output::Event(e) => match e {
                    Event::Connected => self.connected = true,
                    // The client left: closed (DTLS close_notify) or gone (ICE).
                    Event::Closed
                    | Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        return Err(());
                    }
                    Event::ChannelOpen(id, label) if label == "keepalive" => {
                        self.channel = Some(id);
                    }
                    Event::ChannelData(d) if d.data == b"pong" => {
                        self.last_pong = Instant::now();
                        self.stats.pongs.fetch_add(1, Ordering::Relaxed);
                    }
                    Event::KeyframeRequest(_) => {
                        self.stats.keyframe_requests.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                },
            }
        }
    }
}

/// One WebRTC session: waits for the answer, then streams until the client goes (ICE
/// disconnects, DTLS closes, or no `pong` for 30 s), as camera-streamer does.
#[allow(clippy::too_many_arguments)]
fn send(
    rtc: Rtc,
    socket: UdpSocket,
    pending: SdpPendingOffer,
    mid: Mid,
    rx: mpsc::Receiver<Cmd>,
    o: &StreamerOptions,
    stats: &StreamerStats,
    stop: &AtomicBool,
    keep_alive: bool,
) {
    let local = socket.local_addr().expect("bound");
    let mut s = Sender {
        rtc,
        socket,
        o,
        stats,
        channel: None,
        last_pong: Instant::now(),
        connected: false,
        packets: 0,
        video_pt: None,
        lost: Default::default(),
    };
    let mut pending = Some(pending);
    let mut next_au = crate::mock::video(&o.render, o.fps);
    let every = Duration::from_secs_f32(1. / o.fps.max(0.1));
    let mut frame = 0u64;
    let mut next_frame: Option<Instant> = None;
    let mut last_ping = Instant::now();
    let created = Instant::now();
    let mut buf = vec![0u8; 2000];
    loop {
        if stop.load(Ordering::Relaxed) || !s.rtc.is_alive() {
            return;
        }
        // Signalling from the HTTP side; every mutation is drained before the next.
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Answer(sdp, reply) => {
                    let r = match (SdpAnswer::from_sdp_string(&sdp), pending.take()) {
                        (Ok(a), Some(p)) => s
                            .rtc
                            .sdp_api()
                            .accept_answer(p, a)
                            .map_err(|e| e.to_string()),
                        (Err(e), _) => Err(e.to_string()),
                        (_, None) => Err("answered twice".into()),
                    };
                    let _ = reply.send(r);
                }
                Cmd::Candidate(c) => {
                    if let Ok(c) = Candidate::from_sdp_string(&c) {
                        s.rtc.add_remote_candidate(c);
                    }
                }
            }
            if s.drain().is_err() {
                return;
            }
        }
        if !s.connected && created.elapsed() > Duration::from_secs(15) {
            return;
        }
        if keep_alive && s.connected && s.last_pong.elapsed() > Duration::from_secs(30) {
            return;
        }
        // Video once connected, starting with the clip's key frame.
        let now = Instant::now();
        if s.connected && next_frame.is_none_or(|t| now >= t) {
            let au = next_au(frame);
            let pt = s
                .rtc
                .writer(mid)
                .and_then(|w| w.payload_params().next().map(|p| p.pt()));
            if let Some(pt) = pt
                && let Some(w) = s.rtc.writer(mid)
            {
                s.video_pt = Some(*pt);
                let time = MediaTime::new(
                    (frame as f64 * 90_000. / o.fps.max(0.1) as f64) as u64,
                    Frequency::NINETY_KHZ,
                );
                if w.write(pt, now, time, au).is_ok() {
                    stats.sent.lock().unwrap().push(now);
                    stats.frames_sent.fetch_add(1, Ordering::Relaxed);
                }
            }
            frame += 1;
            next_frame = Some(next_frame.unwrap_or(now) + every);
            if s.drain().is_err() {
                return;
            }
        }
        if let Some(id) = s.channel
            && last_ping.elapsed() >= Duration::from_secs(1)
        {
            last_ping = Instant::now();
            if let Some(mut ch) = s.rtc.channel(id) {
                let _ = ch.write(false, b"ping");
            }
            if s.drain().is_err() {
                return;
            }
        }
        let Ok(timeout) = s.drain() else { return };
        let mut wait = timeout.saturating_duration_since(Instant::now());
        if let Some(t) = next_frame {
            wait = wait.min(t.saturating_duration_since(Instant::now()));
        }
        let wait = wait.clamp(Duration::from_millis(1), Duration::from_millis(20));
        let _ = s.socket.set_read_timeout(Some(wait));
        let input = match s.socket.recv_from(&mut buf) {
            Ok((n, source)) => match buf[..n].try_into() {
                Ok(contents) => Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: local,
                        contents,
                    },
                ),
                Err(_) => continue,
            },
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Input::Timeout(Instant::now())
            }
            Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        if s.rtc.handle_input(input).is_err() {
            return;
        }
    }
}
