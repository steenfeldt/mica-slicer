// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Printer camera players. A [`Player`] owns one thread that reads a camera (an MJPEG stream,
//! repeated JPEG snapshots, an RTSP H.264 stream, camera-streamer's WebRTC H.264 stream, or
//! snapshots fetched through a printer's own API), decodes it **off the caller's thread** and
//! keeps only the newest frame, at most at the frame rate the caller asked for (low for
//! thumbnails, full for a focused view).
//! Snapshots are polled at most [`SNAPSHOT_MAX_FPS`] times a second, one request at a time.
//! Dropping the player stops the stream. Errors back off (1 s doubling to 30 s) and retry.
//! A caller that [`Player::touch`]es its player every frame it shows it also stops snapshot
//! polling while it does not (a minimised window that draws nothing).
//!
//! Credentials are passed separately from the URL, are never part of a message or `Debug`
//! output, and URLs with `user:password@` are refused.

mod decode;
mod http;
mod mjpeg;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
#[cfg(any(test, feature = "mock"))]
mod mock_webrtc;
mod rtsp;
mod webrtc;

use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub use decode::Orientation;
pub use mjpeg::Multipart;
pub use rtsp::H264Decoder;
pub use webrtc::signal as webrtc_signal;

/// A decoded picture: tightly packed RGBA, `width * height * 4` bytes.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Counts frames published by this player, from 1.
    pub seq: u64,
}
impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frame({}x{} #{})", self.width, self.height, self.seq)
    }
}

/// Fetches one picture through someone else's connection (a printer API with its own
/// credentials). `Ok(None)`: no picture yet.
pub type FetchFn = Arc<dyn Fn() -> Result<Option<Vec<u8>>, String> + Send + Sync>;

#[derive(Clone)]
pub enum Source {
    /// `multipart/x-mixed-replace` JPEG over HTTP(S).
    Mjpeg(String),
    /// A JPEG (or PNG) picture over HTTP(S), fetched repeatedly.
    Snapshot(String),
    /// RTSP with H.264 video (TCP interleaved).
    Rtsp(String),
    /// camera-streamer's WebRTC (H.264 over RTP/UDP on the LAN), with the snapshot URL to
    /// fall back to when WebRTC does not connect.
    WebRtc(String, Option<String>),
    /// Pictures fetched by the caller's function, repeatedly.
    Fetch(FetchFn),
}
impl Source {
    /// Guesses the kind of a manually entered camera URL: `rtsp://` → RTSP; a path ending in
    /// `/webrtc` → camera-streamer's WebRTC; a path ending in `.jpg` / `.jpeg` / `.png` or
    /// naming a snapshot → snapshots; any other HTTP(S) → MJPEG.
    pub fn from_url(url: &str) -> Result<Self, String> {
        let u = url.trim();
        check_url(u)?;
        Ok(Self::classify(u))
    }
    /// The kind of an already checked URL (see [`Source::from_url`]).
    fn classify(u: &str) -> Self {
        let lower = u.to_ascii_lowercase();
        if lower.starts_with("rtsp://") || lower.starts_with("rtsps://") {
            return Self::Rtsp(u.to_string());
        }
        let path = lower.split('#').next().unwrap_or("");
        let (path_only, query) = path.split_once('?').unwrap_or((path, ""));
        if path_only.trim_end_matches('/').ends_with("/webrtc") {
            return Self::WebRtc(u.to_string(), None);
        }
        let snapshot = [".jpg", ".jpeg", ".png"]
            .iter()
            .any(|e| path_only.ends_with(e))
            || path_only.contains("snapshot")
            || query.contains("snapshot");
        if snapshot {
            Self::Snapshot(u.to_string())
        } else {
            Self::Mjpeg(u.to_string())
        }
    }
    /// The URL, if this source has one.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Mjpeg(u) | Self::Snapshot(u) | Self::Rtsp(u) | Self::WebRtc(u, _) => Some(u),
            Self::Fetch(_) => None,
        }
    }
    /// One word for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Mjpeg(_) => "MJPEG",
            Self::Snapshot(_) | Self::Fetch(_) => "snapshots",
            Self::Rtsp(_) => "RTSP",
            Self::WebRtc(..) => "WebRTC",
        }
    }
}
impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch(_) => f.write_str("Fetch"),
            s => write!(f, "{}({})", s.label(), s.url().unwrap_or("")),
        }
    }
}

/// A camera login. Redacted in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Auth {
    pub username: String,
    pub password: String,
}
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Auth([redacted])")
    }
}

/// How often and how large: frames are published at most `fps` times a second and
/// downscaled to at most `max_width` pixels wide (0: full size).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub fps: f32,
    pub max_width: u32,
}
impl Rate {
    pub const THUMBNAIL: Self = Self {
        fps: 1.,
        max_width: 480,
    };
    pub const FOCUSED: Self = Self {
        fps: 15.,
        max_width: 0,
    };
}

/// Snapshots (and pictures fetched through a printer's API) are asked for at most this
/// often, whatever the rate: a focused camera that only offers snapshots refreshes as fast
/// as the camera answers, up to this.
pub const SNAPSHOT_MAX_FPS: f32 = 4.;

/// A touched player that has not been touched for this long stops polling snapshots.
const LEASE: Duration = Duration::from_secs(3);

/// What a player is doing, for the UI.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Frames published so far.
    pub frames: u64,
    pub last_frame: Option<Instant>,
    /// The last failure (sanitised: no credentials), cleared by the next frame.
    pub error: Option<String>,
    /// Connection attempts made.
    pub attempts: u32,
    /// Why a WebRTC player shows its camera's snapshots instead (WebRTC did not connect).
    pub fallback: Option<String>,
}

static LOOPBACK_ONLY: AtomicBool = AtomicBool::new(false);

/// Refuses every camera URL of this process that is not on this computer (tests, the monkey).
pub fn restrict_to_loopback() {
    LOOPBACK_ONLY.store(true, Ordering::Relaxed);
}

/// Whether [`restrict_to_loopback`] was called.
pub(crate) fn loopback_only() -> bool {
    LOOPBACK_ONLY.load(Ordering::Relaxed)
}

/// The host of `scheme://[user@]host[:port]/…`, without brackets.
fn host(url: &str) -> &str {
    let a = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = a.split(['/', '?', '#']).next().unwrap_or("");
    let h = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        h.split(':').next().unwrap_or("")
    }
}

/// Whether a URL names this computer by a loopback address or `localhost`.
pub fn is_loopback(url: &str) -> bool {
    let h = host(url.trim());
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A URL a player may open: HTTP(S) or RTSP(S), a host, no inline credentials, and on this
/// computer when the process is restricted to loopback.
pub fn check_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if !["http://", "https://", "rtsp://", "rtsps://"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return Err("a camera URL starts with rtsp://, http:// or https://".into());
    }
    let authority = u
        .split_once("://")
        .map(|(_, r)| r.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    if authority.contains('@') {
        return Err(
            "put the camera's user name and password in their own fields, not in the URL".into(),
        );
    }
    if host(u).is_empty() || u.contains(char::is_whitespace) {
        return Err("the camera URL needs a host name or IP address".into());
    }
    if LOOPBACK_ONLY.load(Ordering::Relaxed) && !is_loopback(u) {
        return Err("this run only opens cameras on this computer (127.0.0.1)".into());
    }
    Ok(())
}

/// Removes anything that looks like a password or `user:pass@` from a message.
pub(crate) fn redact(message: &str, auth: Option<&Auth>) -> String {
    let mut m = message.to_string();
    if let Some(a) = auth
        && !a.password.is_empty()
    {
        m = m.replace(&a.password, "[redacted]");
    }
    // `scheme://user:pass@host` → `scheme://host`.
    let mut out = String::with_capacity(m.len());
    let mut rest = m.as_str();
    while let Some(i) = rest.find("://") {
        out.push_str(&rest[..i + 3]);
        rest = &rest[i + 3..];
        let end = rest
            .find(|c: char| c == '/' || c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(rest.len());
        if let Some(at) = rest[..end].rfind('@') {
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out
}

struct Shared {
    stop: AtomicBool,
    /// Frame rate in thousandths of a frame per second.
    fps_milli: AtomicU32,
    max_width: AtomicU32,
    frame: Mutex<Option<Frame>>,
    status: Mutex<Status>,
    seq: AtomicU64,
    /// When the caller last showed the picture ([`Player::touch`]); `None`: never touched,
    /// so always watched.
    touched: Mutex<Option<Instant>>,
}

/// What a source loop sees: the stop flag, the rate and where frames go.
pub(crate) struct Ctx<'a> {
    shared: &'a Shared,
    orientation: Orientation,
    wake: &'a (dyn Fn() + Send + Sync),
    last: Option<Instant>,
    /// The highest rate this source is asked at (snapshots: [`SNAPSHOT_MAX_FPS`]).
    max_fps: f32,
    /// When the last picture was asked for (polling sources).
    last_request: Option<Instant>,
}
impl Ctx<'_> {
    /// Waits for the next picture request of a polling source: one interval after the
    /// previous one began (so a camera that takes 0.3 s to answer is asked again at once,
    /// not 0.25 s after its answer), and only while the picture is watched. False when
    /// stopped meanwhile.
    pub fn next_request(&mut self) -> bool {
        let wait = self.last_request.map_or(Duration::ZERO, |t| {
            self.interval().saturating_sub(t.elapsed())
        });
        if !self.sleep(wait) || !self.wait_watched() {
            return false;
        }
        self.last_request = Some(Instant::now());
        true
    }
    pub fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Relaxed)
    }
    pub fn interval(&self) -> Duration {
        let milli = self
            .shared
            .fps_milli
            .load(Ordering::Relaxed)
            .min((self.max_fps * 1000.) as u32)
            .max(1);
        Duration::from_secs_f64(1000. / milli as f64)
    }
    /// Whether the caller still shows the picture (always, for a caller that never touches).
    pub fn watched(&self) -> bool {
        self.shared
            .touched
            .lock()
            .unwrap()
            .is_none_or(|t| t.elapsed() < LEASE)
    }
    /// Waits until the picture is shown again; false when stopped meanwhile.
    pub fn wait_watched(&self) -> bool {
        while !self.watched() {
            if !self.sleep(Duration::from_millis(100)) {
                return false;
            }
        }
        !self.stopped()
    }
    /// Whether a new frame may be published now. A frame up to a quarter interval early
    /// counts: a camera sending at exactly the asked rate jitters around the interval, and
    /// a strict test would drop every other frame.
    pub fn due(&self) -> bool {
        self.last
            .is_none_or(|t| t.elapsed() >= self.interval().mul_f32(0.75))
    }
    /// Sleeps in small steps; false when stopped meanwhile.
    pub fn sleep(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.stopped() {
                return false;
            }
            thread::sleep((end - Instant::now()).min(Duration::from_millis(50)));
        }
        !self.stopped()
    }
    fn max_width(&self) -> u32 {
        self.shared.max_width.load(Ordering::Relaxed)
    }
    /// Decodes an encoded picture (JPEG, PNG) and publishes it.
    pub fn publish_encoded(&mut self, bytes: &[u8]) -> Result<(), String> {
        let image = decode::decode(bytes, self.orientation, self.max_width())?;
        self.publish(image);
        Ok(())
    }
    /// Publishes a decoded RGBA picture (downscaled and oriented here).
    pub fn publish_rgba(&mut self, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), String> {
        let image = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or("decoded frame has the wrong size")?;
        let image = decode::finish(image, self.orientation, self.max_width());
        self.publish(image);
        Ok(())
    }
    fn publish(&mut self, image: image::RgbaImage) {
        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (width, height) = image.dimensions();
        *self.shared.frame.lock().unwrap() = Some(Frame {
            width,
            height,
            rgba: image.into_raw(),
            seq,
        });
        let now = Instant::now();
        self.last = Some(now);
        {
            let mut s = self.shared.status.lock().unwrap();
            s.frames += 1;
            s.last_frame = Some(now);
            s.error = None;
        }
        (self.wake)();
    }
}

/// One camera being played on its own thread. Dropping it stops the thread (within one
/// read or sleep step; the caller never waits for it).
pub struct Player {
    shared: Arc<Shared>,
    label: &'static str,
}
impl Player {
    /// Starts playing. Fails at once (without any network access) for a URL that
    /// [`check_url`] refuses.
    pub fn start(
        source: Source,
        auth: Option<Auth>,
        orientation: Orientation,
        rate: Rate,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        if let Some(u) = source.url() {
            check_url(u)?;
        }
        if let Source::WebRtc(_, Some(snapshot)) = &source {
            check_url(snapshot)?;
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            fps_milli: AtomicU32::new(0),
            max_width: AtomicU32::new(0),
            frame: Mutex::new(None),
            status: Mutex::new(Status::default()),
            seq: AtomicU64::new(0),
            touched: Mutex::new(None),
        });
        let label = source.label();
        let player = Self { shared, label };
        player.set_rate(rate);
        let worker = player.shared.clone();
        thread::Builder::new()
            .name("mica-camera".into())
            .spawn(move || run(&worker, &source, auth.as_ref(), orientation, &wake))
            .map_err(|_| "cannot start the camera thread".to_string())?;
        Ok(player)
    }
    pub fn set_rate(&self, rate: Rate) {
        let milli = (rate.fps.clamp(0.05, 60.) * 1000.) as u32;
        self.shared.fps_milli.store(milli, Ordering::Relaxed);
        self.shared
            .max_width
            .store(rate.max_width, Ordering::Relaxed);
    }
    /// The caller shows this player's picture now. Once touched, a snapshot player polls
    /// only while touches keep coming (at least every few seconds).
    pub fn touch(&self) {
        *self.shared.touched.lock().unwrap() = Some(Instant::now());
    }
    /// The newest frame not taken yet.
    pub fn take_frame(&self) -> Option<Frame> {
        self.shared.frame.lock().unwrap().take()
    }
    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }
    /// "MJPEG", "RTSP", "WebRTC", "snapshots" (also for a WebRTC player that fell back).
    pub fn label(&self) -> &'static str {
        if self.shared.status.lock().unwrap().fallback.is_some() {
            "snapshots"
        } else {
            self.label
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}
impl fmt::Debug for Player {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Player({})", self.label)
    }
}

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

fn run(
    shared: &Shared,
    source: &Source,
    auth: Option<&Auth>,
    orientation: Orientation,
    wake: &(dyn Fn() + Send + Sync),
) {
    let mut ctx = Ctx {
        shared,
        orientation,
        wake,
        last: None,
        max_fps: match source {
            Source::Snapshot(_) | Source::Fetch(_) => SNAPSHOT_MAX_FPS,
            _ => 60.,
        },
        last_request: None,
    };
    let mut backoff = BACKOFF_MIN;
    // A WebRTC player that never got a frame shows its snapshots instead.
    let mut fallen_back = false;
    while !ctx.stopped() {
        shared.status.lock().unwrap().attempts += 1;
        let before = shared.seq.load(Ordering::Relaxed);
        let result = match source {
            Source::Mjpeg(url) => mjpeg::run(&mut ctx, url, auth),
            Source::Snapshot(url) => http::snapshots(&mut ctx, url, auth),
            Source::Rtsp(url) => rtsp::run(&mut ctx, url, auth),
            Source::WebRtc(_, Some(snapshot)) if fallen_back => {
                http::snapshots(&mut ctx, snapshot, auth)
            }
            Source::WebRtc(url, _) => webrtc::run(&mut ctx, url, auth),
            Source::Fetch(f) => fetch(&mut ctx, f),
        };
        let e = match result {
            // Stopped; or a stream that let go of the camera while nobody looked: it
            // connects again once the picture is shown again.
            Ok(()) => {
                if ctx.stopped() || !ctx.wait_watched() {
                    return;
                }
                continue;
            }
            Err(e) => e,
        };
        if ctx.stopped() {
            return;
        }
        let delivered = shared.seq.load(Ordering::Relaxed) > before;
        if let Source::WebRtc(_, Some(_)) = source
            && !fallen_back
            && !delivered
            && shared.seq.load(Ordering::Relaxed) == 0
        {
            fallen_back = true;
            ctx.max_fps = SNAPSHOT_MAX_FPS;
            let mut s = shared.status.lock().unwrap();
            s.fallback = Some(redact(&e, auth));
            s.error = None;
            drop(s);
            wake();
            continue;
        }
        shared.status.lock().unwrap().error = Some(redact(&e, auth));
        wake();
        // A source that delivered frames before failing starts over at the short delay.
        if delivered {
            backoff = BACKOFF_MIN;
        }
        if !ctx.sleep(backoff) {
            return;
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Repeated pictures through the caller's function.
fn fetch(ctx: &mut Ctx<'_>, f: &FetchFn) -> Result<(), String> {
    loop {
        if ctx.stopped() {
            return Ok(());
        }
        if !ctx.next_request() {
            return Ok(());
        }
        match f()? {
            Some(bytes) => ctx.publish_encoded(&bytes)?,
            // Nothing yet: try again in a while, not in a tight loop.
            None => {
                if !ctx.sleep(ctx.interval().max(Duration::from_secs(1))) {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
