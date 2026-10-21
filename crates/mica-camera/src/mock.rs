// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Synthetic cameras for tests, the mock printers and screenshots (feature `mock`): a
//! moving test picture, JPEG encoding, MJPEG part framing and a small RTSP server that
//! streams H.264 over TCP-interleaved RTP. Loopback only; no downloaded media. The H.264 is
//! a short recorded clip of the test picture (`mica-videodec/testdata`, made with the
//! operating system's encoder), looped; with the development feature `openh264` the server
//! encodes its picture live instead.

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub use crate::mock_webrtc::{CameraStreamer, StreamerOptions, StreamerStats};

/// A picture as RGB bytes.
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Renders the picture at time `t` (seconds).
pub type Renderer = Arc<dyn Fn(f64) -> Rgb + Send + Sync>;

/// A test picture: four colour quadrants (red, green, blue, white: orientation tests can
/// tell them apart) and a dark bar that sweeps with time.
pub fn test_pattern(width: u32, height: u32, t: f64) -> Rgb {
    let mut data = vec![0u8; (width * height * 3) as usize];
    let bar = ((t * 0.5).fract() * width as f64) as u32;
    for y in 0..height {
        for x in 0..width {
            let mut c = match (x < width / 2, y < height / 2) {
                (true, true) => [220, 40, 40],
                (false, true) => [40, 200, 60],
                (true, false) => [50, 70, 220],
                (false, false) => [235, 235, 235],
            };
            if x.abs_diff(bar) < width / 40 + 1 {
                c = [c[0] / 3, c[1] / 3, c[2] / 3];
            }
            let i = ((y * width + x) * 3) as usize;
            data[i..i + 3].copy_from_slice(&c);
        }
    }
    Rgb {
        width,
        height,
        data,
    }
}

pub fn jpeg(rgb: &Rgb, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode(
        &rgb.data,
        rgb.width,
        rgb.height,
        image::ExtendedColorType::Rgb8,
    )
    .expect("JPEG encoding of a valid RGB buffer");
    out
}

/// The boundary the mock MJPEG streams use.
pub const BOUNDARY: &str = "micaframe";

/// Response head of an MJPEG stream.
pub fn mjpeg_head() -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary={BOUNDARY}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n"
    )
}

/// One MJPEG part (boundary line, headers, picture).
pub fn mjpeg_part(jpeg: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        jpeg.len()
    )
    .into_bytes();
    out.extend_from_slice(jpeg);
    out.extend_from_slice(b"\r\n");
    out
}

/// Writes an endless MJPEG stream (after the head) at `fps` until the client goes away.
pub fn serve_mjpeg(stream: &mut TcpStream, render: &Renderer, fps: f32) {
    let started = Instant::now();
    if stream.write_all(mjpeg_head().as_bytes()).is_err() {
        return;
    }
    let every = Duration::from_secs_f32(1. / fps.max(0.1));
    loop {
        let frame = render(started.elapsed().as_secs_f64());
        if stream.write_all(&mjpeg_part(&jpeg(&frame, 80))).is_err() || stream.flush().is_err() {
            return;
        }
        thread::sleep(every);
    }
}

/// The recorded clip: 20 access units (Annex B) of `test_pattern(320, 240, t)` at 10 fps,
/// the first an IDR with SPS / PPS, then P frames.
pub fn test_clip() -> Vec<Vec<u8>> {
    mica_videodec::access_units(include_bytes!(
        "../../mica-videodec/testdata/test-pattern-320x240.h264"
    ))
}

/// Frame rate of [`test_clip`].
pub const TEST_CLIP_FPS: f32 = 10.;

/// H.264 access units (Annex B) for `frames` pictures, the first an IDR with SPS / PPS:
/// a stream encoded here by OpenH264, for decoder tests (development feature `openh264`).
#[cfg(feature = "openh264")]
pub fn h264_frames(render: &Renderer, frames: usize, fps: f32) -> Vec<Vec<u8>> {
    let mut enc = encoder(fps);
    (0..frames)
        .map(|i| encode(&mut enc, &render(i as f64 / fps as f64)))
        .collect()
}

#[cfg(feature = "openh264")]
fn encoder(fps: f32) -> openh264::encoder::Encoder {
    use openh264::encoder::{BitRate, EncoderConfig, FrameRate, IntraFramePeriod};
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(1_000_000))
        .max_frame_rate(FrameRate::from_hz(fps))
        .skip_frames(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps.ceil() as u32));
    openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
        .expect("OpenH264 encoder")
}

#[cfg(feature = "openh264")]
fn encode(enc: &mut openh264::encoder::Encoder, rgb: &Rgb) -> Vec<u8> {
    use openh264::formats::{RgbSliceU8, YUVBuffer};
    let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(
        &rgb.data,
        (rgb.width as usize, rgb.height as usize),
    ));
    enc.encode(&yuv).map(|b| b.to_vec()).unwrap_or_default()
}

pub use mica_videodec::nal_units;

/// RTP packets (RFC 6184: single NAL units, FU-A above `mtu`) for one access unit.
fn packetize(au: &[u8], seq: &mut u16, timestamp: u32, mtu: usize) -> Vec<Vec<u8>> {
    let nals = nal_units(au);
    let mut out = vec![];
    let header = |seq: u16, marker: bool| {
        let mut h = vec![0x80, 96 | if marker { 0x80 } else { 0 }];
        h.extend_from_slice(&seq.to_be_bytes());
        h.extend_from_slice(&timestamp.to_be_bytes());
        h.extend_from_slice(&0x4D49_4341u32.to_be_bytes());
        h
    };
    for (k, nal) in nals.iter().enumerate() {
        let last_nal = k + 1 == nals.len();
        if nal.len() <= mtu {
            let mut p = header(*seq, last_nal);
            p.extend_from_slice(nal);
            out.push(p);
            *seq = seq.wrapping_add(1);
            continue;
        }
        let indicator = (nal[0] & 0xE0) | 28;
        let kind = nal[0] & 0x1F;
        let chunks: Vec<&[u8]> = nal[1..].chunks(mtu - 2).collect();
        for (j, c) in chunks.iter().enumerate() {
            let start = j == 0;
            let end = j + 1 == chunks.len();
            let mut p = header(*seq, last_nal && end);
            p.push(indicator);
            p.push(kind | if start { 0x80 } else { 0 } | if end { 0x40 } else { 0 });
            p.extend_from_slice(c);
            out.push(p);
            *seq = seq.wrapping_add(1);
        }
    }
    out
}

/// A small RTSP 1.0 server: one H.264 video track over TCP-interleaved RTP. Optional Basic
/// login. With the development feature `openh264` the video is encoded live from `render`
/// (each client gets its own encoder, starting on an IDR frame); otherwise it is
/// [`test_clip`] in a loop and `render` is not used.
pub struct RtspServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
}
impl Drop for RtspServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock accept().
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}
impl RtspServer {
    /// Binds 127.0.0.1:`port` (0: any free port).
    pub fn start(
        port: u16,
        path: &str,
        fps: f32,
        login: Option<(String, String)>,
        render: Renderer,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let path = path.trim_start_matches('/').to_string();
        thread::Builder::new()
            .name("mock-rtsp".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if s.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else { continue };
                    let (path, login, render, s) =
                        (path.clone(), login.clone(), render.clone(), s.clone());
                    thread::spawn(move || {
                        let _ = rtsp_connection(stream, &path, fps, login.as_ref(), &render, &s);
                    });
                }
            })?;
        Ok(Self { port, stop })
    }
    pub fn url(&self, path: &str) -> String {
        format!(
            "rtsp://127.0.0.1:{}/{}",
            self.port,
            path.trim_start_matches('/')
        )
    }
}

fn rtsp_connection(
    stream: TcpStream,
    path: &str,
    fps: f32,
    login: Option<&(String, String)>,
    render: &Renderer,
    server_stop: &AtomicBool,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    let mut reader = BufReader::new(stream);
    let playing = Arc::new(AtomicBool::new(false));
    let session = "4D494341";
    let expected_auth = login.map(|(u, p)| {
        format!(
            "Basic {}",
            crate::http::base64(format!("{u}:{p}").as_bytes())
        )
    });
    loop {
        // Skip interleaved RTCP the client may send ($ channel len data).
        let mut line = String::new();
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                playing.store(false, Ordering::Relaxed);
                return Ok(());
            }
            if buf[0] == b'$' {
                let mut head = [0u8; 4];
                std::io::Read::read_exact(&mut reader, &mut head)?;
                let mut skip = vec![0u8; u16::from_be_bytes([head[2], head[3]]) as usize];
                std::io::Read::read_exact(&mut reader, &mut skip)?;
                continue;
            }
            break;
        }
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let uri = parts.next().unwrap_or("").to_string();
        let mut cseq = String::from("0");
        let mut auth = None;
        let mut content_length = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 {
                return Ok(());
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "cseq" => cseq = v.trim().to_string(),
                    "authorization" => auth = Some(v.trim().to_string()),
                    "content-length" => content_length = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        if content_length > 0 {
            let mut body = vec![0u8; content_length.min(65536)];
            std::io::Read::read_exact(&mut reader, &mut body)?;
        }
        let base = uri.split('?').next().unwrap_or("").trim_end_matches('/');
        let reply = |status: &str, extra: &str, body: &str| {
            let mut w = writer.lock().unwrap();
            let text = format!(
                "RTSP/1.0 {status}\r\nCSeq: {cseq}\r\nServer: Mica mock\r\n{extra}{}\r\n{body}",
                if body.is_empty() {
                    String::new()
                } else {
                    format!("Content-Length: {}\r\n", body.len())
                }
            );
            w.write_all(text.as_bytes())
        };
        if method != "OPTIONS"
            && let Some(expected) = &expected_auth
            && auth.as_deref() != Some(expected.as_str())
        {
            reply(
                "401 Unauthorized",
                "WWW-Authenticate: Basic realm=\"mica-mock\"\r\n",
                "",
            )?;
            continue;
        }
        let on_path = |u: &str| {
            u.split_once("://")
                .map(|(_, r)| r.split_once('/').map_or("", |(_, p)| p))
                .unwrap_or("")
                .trim_end_matches('/')
                .starts_with(path)
        };
        match method.as_str() {
            "OPTIONS" => reply(
                "200 OK",
                "Public: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER\r\n",
                "",
            )?,
            "DESCRIBE" if !on_path(base) => reply("404 Not Found", "", "")?,
            "DESCRIBE" => {
                let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=Mica mock camera\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\n\
                           m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
                           a=fmtp:96 packetization-mode=1\r\na=control:trackID=0\r\n";
                reply(
                    "200 OK",
                    &format!("Content-Base: {base}/\r\nContent-Type: application/sdp\r\n"),
                    sdp,
                )?;
            }
            "SETUP" => reply(
                "200 OK",
                &format!(
                    "Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\nSession: {session};timeout=60\r\n"
                ),
                "",
            )?,
            "PLAY" => {
                reply(
                    "200 OK",
                    &format!(
                        "Session: {session}\r\nRTP-Info: url={base}/trackID=0;seq=1;rtptime=0\r\n"
                    ),
                    "",
                )?;
                if !playing.swap(true, Ordering::Relaxed) {
                    let (w, p, r) = (writer.clone(), playing.clone(), render.clone());
                    thread::spawn(move || stream_h264(&w, &p, &r, fps));
                }
            }
            "TEARDOWN" => {
                playing.store(false, Ordering::Relaxed);
                reply("200 OK", &format!("Session: {session}\r\n"), "")?;
                return Ok(());
            }
            _ => reply("200 OK", &format!("Session: {session}\r\n"), "")?,
        }
        if server_stop.load(Ordering::Relaxed) {
            playing.store(false, Ordering::Relaxed);
            return Ok(());
        }
    }
}

/// Access units for one client: encoded live (feature `openh264`) or the recorded clip.
#[cfg(feature = "openh264")]
pub(crate) fn video(render: &Renderer, fps: f32) -> impl FnMut(u64) -> Vec<u8> {
    let mut enc = encoder(fps);
    let render = render.clone();
    move |n| encode(&mut enc, &render(n as f64 / fps as f64))
}
#[cfg(not(feature = "openh264"))]
pub(crate) fn video(_render: &Renderer, _fps: f32) -> impl FnMut(u64) -> Vec<u8> {
    let clip = test_clip();
    move |n| clip[n as usize % clip.len()].clone()
}

fn stream_h264(writer: &Mutex<TcpStream>, playing: &AtomicBool, render: &Renderer, fps: f32) {
    let mut next_au = video(render, fps);
    let started = Instant::now();
    let every = Duration::from_secs_f32(1. / fps.max(0.1));
    let mut seq = 1u16;
    let mut n = 0u64;
    while playing.load(Ordering::Relaxed) {
        let t = n as f64 / fps as f64;
        let au = next_au(n);
        let ts = (t * 90_000.) as u32;
        for p in packetize(&au, &mut seq, ts, 1400) {
            let mut frame = vec![b'$', 0];
            frame.extend_from_slice(&(p.len() as u16).to_be_bytes());
            frame.extend_from_slice(&p);
            if writer.lock().unwrap().write_all(&frame).is_err() {
                playing.store(false, Ordering::Relaxed);
                return;
            }
        }
        n += 1;
        let next = every * n as u32;
        if let Some(wait) = next.checked_sub(started.elapsed()) {
            thread::sleep(wait);
        }
    }
}
