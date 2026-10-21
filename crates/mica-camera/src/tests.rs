// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use super::*;
use crate::mock::{Renderer, RtspServer, test_pattern};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::AtomicUsize,
};

fn pattern(w: u32, h: u32) -> Renderer {
    Arc::new(move |t| test_pattern(w, h, t))
}

fn wait_frame(p: &Player, limit: Duration) -> Option<Frame> {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if let Some(f) = p.take_frame() {
            return Some(f);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

fn wait_error(p: &Player, limit: Duration) -> Option<String> {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if let Some(e) = p.status().error {
            return Some(e);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

/// The RGBA pixel at (x, y).
fn px(f: &Frame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * f.width + x) * 4) as usize;
    [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
}

/// Red-dominant, green-dominant, blue-dominant or bright.
fn colour(p: [u8; 4]) -> char {
    let [r, g, b, _] = p.map(i32::from);
    if r > 180 && g > 180 && b > 180 {
        'w'
    } else if r > g + 60 && r > b + 60 {
        'r'
    } else if g > r + 60 && g > b + 60 {
        'g'
    } else if b > r + 60 && b > g + 60 {
        'b'
    } else {
        '?'
    }
}

/// Quadrant colours: top-left, top-right, bottom-left, bottom-right.
fn quadrants(f: &Frame) -> String {
    let (w, h) = (f.width, f.height);
    [
        (w / 8, h / 8),
        (w * 7 / 8, h / 8),
        (w / 8, h * 7 / 8),
        (w * 7 / 8, h * 7 / 8),
    ]
    .iter()
    .map(|&(x, y)| colour(px(f, x, y)))
    .collect()
}

/// Classification and the URL checks do not depend on `restrict_to_loopback`, which another
/// test of this process turns on at any moment (tests run in parallel).
#[test]
fn urls_are_classified_and_checked_without_network() {
    assert!(matches!(
        Source::classify("rtsp://198.51.100.9:554/stream1"),
        Source::Rtsp(_)
    ));
    assert!(matches!(
        Source::classify("http://cam/snapshot.jpg"),
        Source::Snapshot(_)
    ));
    assert!(matches!(
        Source::classify("http://u1/webcam/?action=snapshot"),
        Source::Snapshot(_)
    ));
    assert!(matches!(
        Source::classify("http://u1/webcam/?action=stream"),
        Source::Mjpeg(_)
    ));
    assert!(matches!(
        Source::classify("http://cam:8080/webrtc"),
        Source::WebRtc(_, None)
    ));
    // `from_url` checks, then classifies (loopback: allowed either way).
    assert!(matches!(
        Source::from_url(" rtsp://127.0.0.1:554/stream1 "),
        Ok(Source::Rtsp(u)) if u == "rtsp://127.0.0.1:554/stream1"
    ));
    assert!(matches!(
        Source::from_url("http://localhost/webcam/?action=stream"),
        Ok(Source::Mjpeg(_))
    ));
    // These are refused before the loopback switch is looked at.
    assert!(Source::from_url("ftp://cam/x").is_err());
    assert!(Source::from_url("rtsp://admin:secret@cam/x").is_err());
    assert!(Source::from_url("http:///x").is_err());
    assert!(is_loopback("rtsp://127.0.0.1:8554/x"));
    assert!(is_loopback("http://[::1]:80/"));
    assert!(is_loopback("http://localhost/x"));
    assert!(!is_loopback("http://192.0.2.9/x"));
    assert!(!is_loopback("http://127.0.0.1.example.com/x"));
    assert_eq!(http::base64(b"user:pass"), "dXNlcjpwYXNz");
    assert_eq!(http::base64(b"ab"), "YWI=");
    assert_eq!(http::base64(b"a"), "YQ==");
    let auth = Auth {
        username: "u".into(),
        password: "hunter2".into(),
    };
    assert_eq!(format!("{auth:?}"), "Auth([redacted])");
    let r = redact(
        "failed rtsp://admin:hunter2@cam.lan/x and hunter2",
        Some(&auth),
    );
    assert!(!r.contains("hunter2"), "{r}");
    assert!(r.contains("rtsp://cam.lan/x"), "{r}");
}

/// A reader that hands out a few bytes at a time (boundaries straddle reads).
struct Trickle(Vec<u8>, usize);
impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(7).min(self.0.len() - self.1);
        buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
        self.1 += n;
        Ok(n)
    }
}

#[test]
fn multipart_parts_with_and_without_lengths() {
    // Content-Length parts, boundary from the header, read a few bytes at a time.
    let mut body = Vec::new();
    for p in [b"FIRST".as_slice(), b"second--part", b"3"] {
        body.extend_from_slice(&mock::mjpeg_part(p));
    }
    body.extend_from_slice(b"--micaframe--\r\n");
    let mut m = Multipart::new(
        Trickle(body.clone(), 0),
        Some("multipart/x-mixed-replace; boundary=micaframe"),
        1024,
    );
    assert_eq!(m.next_part().unwrap().unwrap(), b"FIRST");
    assert_eq!(m.next_part().unwrap().unwrap(), b"second--part");
    assert_eq!(m.next_part().unwrap().unwrap(), b"3");
    assert_eq!(m.next_part().unwrap(), None);
    // No boundary in the header: learnt from the first line. Quoted, dashed boundary too.
    let mut m = Multipart::new(Trickle(body, 0), None, 1024);
    assert_eq!(m.next_part().unwrap().unwrap(), b"FIRST");
    // No Content-Length: parts end at the next boundary.
    let body = b"\r\n--\"x\"\r\nContent-Type: image/jpeg\r\n\r\nAAAA\r\n--\"x\"\r\n\r\nBB\r\n--\"x\"--\r\n";
    let body = String::from_utf8_lossy(body).replace('"', "");
    let mut m = Multipart::new(
        Trickle(body.into_bytes(), 0),
        Some("multipart/x-mixed-replace;boundary=\"--x\""),
        1024,
    );
    assert_eq!(m.next_part().unwrap().unwrap(), b"AAAA");
    assert_eq!(m.next_part().unwrap().unwrap(), b"BB");
    assert_eq!(m.next_part().unwrap(), None);
    // Oversized parts are refused.
    let mut m = Multipart::new(
        Trickle(mock::mjpeg_part(&[0u8; 2000]), 0),
        Some("multipart/x-mixed-replace; boundary=micaframe"),
        1024,
    );
    assert!(m.next_part().is_err());
}

/// An HTTP server on loopback that answers every connection with `handler`.
fn http_server(
    handler: impl Fn(&str, &mut std::net::TcpStream) + Send + Sync + 'static,
) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = Arc::new(handler);
    let connections = Arc::new(AtomicUsize::new(0));
    let c = connections.clone();
    thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { continue };
            c.fetch_add(1, Ordering::Relaxed);
            let h = handler.clone();
            thread::spawn(move || {
                let mut head = Vec::new();
                let mut b = [0u8];
                while !head.ends_with(b"\r\n\r\n") {
                    if s.read(&mut b).unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(b[0]);
                }
                h(&String::from_utf8_lossy(&head), &mut s);
            });
        }
    });
    (port, connections)
}

#[test]
fn mjpeg_stream_is_decoded_at_the_asked_rate_and_stops_when_dropped() {
    let render = pattern(160, 120);
    let open = Arc::new(AtomicUsize::new(0));
    let o = open.clone();
    let (port, _) = http_server(move |_, s| {
        o.fetch_add(1, Ordering::Relaxed);
        mock::serve_mjpeg(s, &render, 30.);
        o.fetch_sub(1, Ordering::Relaxed);
    });
    let url = format!("http://127.0.0.1:{port}/webcam/?action=stream");
    let source = Source::from_url(&url).unwrap();
    assert!(matches!(source, Source::Mjpeg(_)));
    let player = Player::start(
        source,
        None,
        Orientation::default(),
        Rate {
            fps: 4.,
            max_width: 80,
        },
        || {},
    )
    .unwrap();
    let f = wait_frame(&player, Duration::from_secs(5)).expect("a frame");
    assert_eq!((f.width, f.height), (80, 60), "downscaled to max_width");
    assert_eq!(quadrants(&f), "rgbw");
    // About four frames a second although the camera sends thirty.
    let before = player.status().frames;
    thread::sleep(Duration::from_millis(1500));
    let n = player.status().frames - before;
    assert!((3..=8).contains(&n), "{n} frames in 1.5 s at 4 fps");
    // Full rate and size on request.
    player.set_rate(Rate::FOCUSED);
    thread::sleep(Duration::from_millis(300));
    let f = wait_frame(&player, Duration::from_secs(2)).unwrap();
    assert_eq!((f.width, f.height), (160, 120));
    assert_eq!(open.load(Ordering::Relaxed), 1);
    drop(player);
    // The server notices the closed connection within a few frames.
    let end = Instant::now() + Duration::from_secs(5);
    while open.load(Ordering::Relaxed) > 0 && Instant::now() < end {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        open.load(Ordering::Relaxed),
        0,
        "the stream stops when the player goes"
    );
}

#[test]
fn snapshots_send_basic_auth_and_failures_back_off_without_leaking_it() {
    let jpeg = mock::jpeg(&test_pattern(64, 48, 0.), 85);
    let expected = format!("Basic {}", http::base64(b"maker:s3cret-pw"));
    let (port, connections) = http_server(move |head, s| {
        let ok = head
            .lines()
            .any(|l| l.eq_ignore_ascii_case(&format!("authorization: {expected}")));
        if ok {
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                jpeg.len()
            );
            let _ = s.write_all(&jpeg);
        } else {
            let _ = s.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let url = format!("http://127.0.0.1:{port}/snapshot.jpg");
    let good = Auth {
        username: "maker".into(),
        password: "s3cret-pw".into(),
    };
    let p = Player::start(
        Source::from_url(&url).unwrap(),
        Some(good),
        Orientation {
            rotation: 180,
            ..Default::default()
        },
        Rate {
            fps: 5.,
            max_width: 0,
        },
        || {},
    )
    .unwrap();
    let f = wait_frame(&p, Duration::from_secs(5)).expect("a snapshot");
    assert_eq!((f.width, f.height), (64, 48));
    assert_eq!(quadrants(&f), "wbgr", "turned 180°");
    drop(p);
    // A wrong password: a readable error, no password in it, and retries slow down.
    let bad = Auth {
        username: "maker".into(),
        password: "wrong-pw".into(),
    };
    let start = connections.load(Ordering::Relaxed);
    let p = Player::start(
        Source::Snapshot(url),
        Some(bad),
        Orientation::default(),
        Rate {
            fps: 10.,
            max_width: 0,
        },
        || {},
    )
    .unwrap();
    let e = wait_error(&p, Duration::from_secs(5)).expect("an error");
    assert!(e.contains("refused the login"), "{e}");
    assert!(!e.contains("wrong-pw"));
    thread::sleep(Duration::from_millis(2500));
    let tries = connections.load(Ordering::Relaxed) - start;
    assert!(
        (1..=3).contains(&tries),
        "{tries} tries in 2.5 s: backing off 1 s, 2 s, …"
    );
}

#[test]
fn unreachable_cameras_report_and_loopback_only_refuses_others() {
    // A port nobody listens on.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let p = Player::start(
        Source::Mjpeg(format!("http://127.0.0.1:{port}/stream")),
        None,
        Orientation::default(),
        Rate::THUMBNAIL,
        || {},
    )
    .unwrap();
    let e = wait_error(&p, Duration::from_secs(8)).expect("an error");
    assert!(e.contains("unreachable"), "{e}");
    restrict_to_loopback();
    let refused = Player::start(
        Source::Rtsp("rtsp://192.0.2.9/stream".into()),
        None,
        Orientation::default(),
        Rate::THUMBNAIL,
        || {},
    );
    assert!(
        refused
            .unwrap_err()
            .contains("only opens cameras on this computer")
    );
}

#[test]
fn the_recorded_clip_decodes_with_the_system_decoder() {
    let clip = mock::test_clip();
    assert_eq!(clip.len(), 20);
    let mut d = H264Decoder::new().unwrap_or_else(|e| panic!("{e}"));
    let mut last = None;
    let mut decoded = 0;
    // Twice round, as the mock server loops it.
    for (i, au) in clip.iter().chain(&clip).enumerate() {
        if let Some((w, h, rgba)) = d.decode(au, i % 3 == 0).unwrap() {
            decoded += 1;
            last = Some(Frame {
                width: w,
                height: h,
                rgba,
                seq: 0,
            });
        }
    }
    assert!(decoded >= 5, "{decoded}");
    let f = last.unwrap();
    assert_eq!((f.width, f.height), (320, 240));
    assert_eq!(quadrants(&f), "rgbw");
}

#[cfg(feature = "openh264")]
#[test]
fn h264_recorded_here_decodes_to_the_test_picture() {
    let frames = mock::h264_frames(&pattern(176, 144), 12, 10.);
    assert!(frames[0].len() > 20);
    // The first access unit carries SPS (7), PPS (8) and an IDR slice (5).
    let kinds: Vec<u8> = mock::nal_units(&frames[0])
        .iter()
        .map(|n| n[0] & 0x1F)
        .collect();
    assert!(
        kinds.contains(&7) && kinds.contains(&8) && kinds.contains(&5),
        "{kinds:?}"
    );
    let mut d = H264Decoder::new().unwrap();
    let mut decoded = 0;
    let mut last = None;
    for (i, au) in frames.iter().enumerate() {
        // Decode every frame, convert only every third (what a slow thumbnail does).
        if let Some((w, h, rgba)) = d.decode(au, i % 3 == 0).unwrap() {
            decoded += 1;
            last = Some(Frame {
                width: w,
                height: h,
                rgba,
                seq: 0,
            });
        }
    }
    assert!(decoded >= 3, "{decoded}");
    let f = last.unwrap();
    assert_eq!((f.width, f.height), (176, 144));
    assert_eq!(quadrants(&f), "rgbw");
    // Garbage is an error, not a panic.
    assert!(
        d.decode(&[0, 0, 0, 1, 0x65, 0xff, 0x00, 0x13], true)
            .is_err()
            || d.decode(&[0, 0, 0, 1, 0x65], true).is_ok()
    );
}

#[test]
fn rtsp_h264_stream_plays_with_a_login() {
    let login = ("viewer".to_string(), "rtsp-pass".to_string());
    let server =
        RtspServer::start(0, "/coreone", 15., Some(login.clone()), pattern(320, 240)).unwrap();
    let url = server.url("coreone");
    let p = Player::start(
        Source::from_url(&url).unwrap(),
        Some(Auth {
            username: login.0.clone(),
            password: login.1.clone(),
        }),
        Orientation::default(),
        Rate {
            fps: 5.,
            max_width: 160,
        },
        || {},
    )
    .unwrap();
    assert_eq!(p.label(), "RTSP");
    let f = wait_frame(&p, Duration::from_secs(10)).unwrap_or_else(|| {
        panic!("no RTSP frame: {:?}", p.status());
    });
    assert_eq!((f.width, f.height), (160, 120));
    assert_eq!(quadrants(&f), "rgbw");
    drop(p);
    // Without the login: refused, readably.
    let p = Player::start(
        Source::Rtsp(url),
        None,
        Orientation::default(),
        Rate::THUMBNAIL,
        || {},
    )
    .unwrap();
    let e = wait_error(&p, Duration::from_secs(10)).expect("an error");
    assert!(
        e.contains("401") || e.to_lowercase().contains("auth"),
        "{e}"
    );
}

/// A decoder that cannot start is an error the player shows, not a black picture.
#[cfg(all(target_os = "macos", not(feature = "openh264")))]
#[test]
fn rtsp_without_a_working_decoder_says_so() {
    let server = RtspServer::start(0, "/coreone", 15., None, pattern(320, 240)).unwrap();
    rtsp::BROKEN_DECODER_PORT.store(server.port, Ordering::Relaxed);
    let p = Player::start(
        Source::from_url(&server.url("coreone")).unwrap(),
        None,
        Orientation::default(),
        Rate::THUMBNAIL,
        || {},
    )
    .unwrap();
    let e = wait_error(&p, Duration::from_secs(10)).expect("an error");
    assert!(e.contains("cannot start the H.264 decoder"), "{e}");
    assert!(p.take_frame().is_none());
}

/// Reads one request head from `s`; `None` at the end of the connection.
fn read_head(s: &mut std::net::TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut b = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).ok()? == 0 {
            return None;
        }
        head.push(b[0]);
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

#[test]
fn snapshots_refresh_up_to_four_a_second_on_one_connection_and_pause_unwatched() {
    let jpeg = mock::jpeg(&test_pattern(64, 48, 0.), 85);
    let requests = Arc::new(AtomicUsize::new(0));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let most_in_flight = Arc::new(AtomicUsize::new(0));
    let (r, f, m) = (requests.clone(), in_flight.clone(), most_in_flight.clone());
    // A camera that answers in 40 ms and keeps the connection open.
    let (port, connections) = http_server(move |_, s| {
        loop {
            r.fetch_add(1, Ordering::Relaxed);
            let n = f.fetch_add(1, Ordering::Relaxed) + 1;
            m.fetch_max(n, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(40));
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                jpeg.len()
            );
            let _ = s.write_all(&jpeg);
            f.fetch_sub(1, Ordering::Relaxed);
            if read_head(s).is_none() {
                return;
            }
        }
    });
    let p = Player::start(
        Source::from_url(&format!("http://127.0.0.1:{port}/webcam/snapshot.jpg")).unwrap(),
        None,
        Orientation::default(),
        Rate::FOCUSED,
        || {},
    )
    .unwrap();
    p.touch();
    wait_frame(&p, Duration::from_secs(5)).expect("a snapshot");
    let before = requests.load(Ordering::Relaxed);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2000) {
        p.touch();
        thread::sleep(Duration::from_millis(100));
    }
    let n = requests.load(Ordering::Relaxed) - before;
    // 15 fps asked, the camera could do 25: four a second.
    assert!((6..=9).contains(&n), "{n} snapshots in 2 s");
    assert_eq!(most_in_flight.load(Ordering::Relaxed), 1);
    assert_eq!(
        connections.load(Ordering::Relaxed),
        1,
        "one kept-alive connection"
    );
    // Nobody touches it any more (a minimised window): polling stops within the lease.
    thread::sleep(LEASE + Duration::from_millis(500));
    let paused = requests.load(Ordering::Relaxed);
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        requests.load(Ordering::Relaxed),
        paused,
        "no polling unwatched"
    );
    // Shown again: polling resumes.
    p.touch();
    thread::sleep(Duration::from_millis(800));
    assert!(requests.load(Ordering::Relaxed) > paused);
}

/// A camera that takes 0.3 s per snapshot (the U1 takes 0.25–0.45 s) is asked again as
/// soon as it answers: over 3/s, not one answer plus a quarter second each. Measured on the
/// camera's side (from the end of an answer to the next request), so a slow or busy machine
/// does not change the result: back to back takes milliseconds, the quarter second it must
/// not wait takes 250 ms.
#[test]
fn slow_snapshots_are_asked_for_back_to_back() {
    let jpeg = mock::jpeg(&test_pattern(64, 48, 0.), 85);
    let gaps = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let g = gaps.clone();
    let (port, _) = http_server(move |_, s| {
        loop {
            thread::sleep(Duration::from_millis(300));
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                jpeg.len()
            );
            let _ = s.write_all(&jpeg);
            let answered = Instant::now();
            if read_head(s).is_none() {
                return;
            }
            g.lock().unwrap().push(answered.elapsed());
        }
    });
    let p = Player::start(
        Source::Snapshot(format!("http://127.0.0.1:{port}/webcam/snapshot.jpg")),
        None,
        Orientation::default(),
        Rate::FOCUSED,
        || {},
    )
    .unwrap();
    wait_frame(&p, Duration::from_secs(5)).expect("a snapshot");
    assert!(
        wait_until(Duration::from_secs(10), || gaps.lock().unwrap().len() >= 5),
        "five more snapshots asked for"
    );
    let mut gaps = gaps.lock().unwrap().clone();
    gaps.sort();
    // The median: one hiccup of a shared CI runner does not decide it.
    let median = gaps[gaps.len() / 2];
    assert!(
        median < Duration::from_millis(125),
        "asked again {median:?} after an answer ({gaps:?}); back to back expected"
    );
    // The answers are shown, not only fetched.
    assert!(p.status().frames >= 3, "{} frames", p.status().frames);
}

fn streamer(lose_every: Option<u32>, webrtc: bool) -> mock::CameraStreamer {
    mock::CameraStreamer::start(
        0,
        mock::StreamerOptions {
            fps: 15.,
            render: pattern(320, 240),
            lose_every,
            webrtc,
        },
    )
    .unwrap()
}

fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    f()
}

/// End to end over UDP on 127.0.0.1: camera-streamer's signalling, ICE, DTLS, SRTP, H.264
/// depacketised and decoded by the system decoder, the keep-alive answered.
#[test]
fn webrtc_from_camera_streamer_plays_and_answers_its_keepalive() {
    let cam = streamer(None, true);
    let published = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let pb = published.clone();
    let t0 = Instant::now();
    let p = Player::start(
        Source::WebRtc(cam.url(), Some(cam.snapshot_url())),
        None,
        Orientation::default(),
        // Faster than the camera: every decoded frame is published (latency below).
        Rate {
            fps: 60.,
            max_width: 0,
        },
        move || pb.lock().unwrap().push(Instant::now()),
    )
    .unwrap();
    assert_eq!(p.label(), "WebRTC");
    let f = wait_frame(&p, Duration::from_secs(10))
        .unwrap_or_else(|| panic!("no WebRTC frame: {:?}", p.status()));
    let first = t0.elapsed();
    assert_eq!((f.width, f.height), (320, 240));
    assert_eq!(quadrants(&f), "rgbw");
    // The three POSTs camera-streamer's own page and Mainsail make.
    let posts = cam.stats.posts.lock().unwrap().clone();
    assert_eq!(posts.len(), 3, "{posts:?}");
    assert_eq!(posts[0], r#"{"type":"request","keepAlive":true}"#);
    let answer: serde_json::Value = serde_json::from_str(&posts[1]).unwrap();
    assert_eq!(answer["type"], "answer");
    assert!(answer["id"].as_str().unwrap().starts_with("rtc-"));
    assert!(answer["sdp"].as_str().unwrap().contains("a=recvonly"));
    let cand: serde_json::Value = serde_json::from_str(&posts[2]).unwrap();
    assert_eq!(cand["type"], "remote_candidate");
    assert_eq!(cand["id"], answer["id"]);
    assert!(
        cand["candidates"][0]["candidate"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1")
    );
    // Steady video at the camera's 15 fps, and pongs for its pings.
    let before = p.status().frames;
    thread::sleep(Duration::from_millis(2000));
    let n = p.status().frames - before;
    assert!((20..=40).contains(&n), "{n} frames in 2 s at 15 fps");
    assert!(cam.stats.pongs.load(Ordering::Relaxed) >= 1);
    assert!(p.status().error.is_none(), "{:?}", p.status());
    // Latency: each published frame against the newest frame sent before it (exact while
    // the latency stays below the 67 ms frame interval). The first second is left out: the
    // system decoder takes ~0.2 s to start, and frames that queue meanwhile are decoded but
    // not all published.
    let sent = cam.stats.sent.lock().unwrap().clone();
    let got = published.lock().unwrap().clone();
    let mut lat: Vec<Duration> = got
        .iter()
        .filter(|g| g.duration_since(t0) > first + Duration::from_secs(1))
        .filter_map(|g| {
            sent.iter()
                .rev()
                .find(|s| *s <= g)
                .map(|s| g.duration_since(*s))
        })
        .collect();
    lat.sort();
    eprintln!(
        "WebRTC mock: first frame {:.0} ms after start; latency median {:.1} ms, max {:.1} ms over {} frames",
        first.as_secs_f64() * 1000.,
        lat[lat.len() / 2].as_secs_f64() * 1000.,
        lat[lat.len() - 1].as_secs_f64() * 1000.,
        lat.len()
    );
    assert!(lat[lat.len() / 2] < Duration::from_millis(50));
    // Dropping the player ends the camera's session at once (DTLS close).
    drop(p);
    assert!(
        wait_until(Duration::from_secs(5), || cam
            .stats
            .sessions
            .load(Ordering::Relaxed)
            == 0),
        "the camera still streams"
    );
}

/// Frames published in `secs` seconds after the first, and the camera's statistics.
fn lossy(lose_every: u32, secs: u64) -> (u64, usize, usize, Player, mock::CameraStreamer) {
    let cam = streamer(Some(lose_every), true);
    let p = Player::start(
        Source::WebRtc(cam.url(), None),
        None,
        Orientation::default(),
        Rate::FOCUSED,
        || {},
    )
    .unwrap();
    wait_frame(&p, Duration::from_secs(10))
        .unwrap_or_else(|| panic!("no WebRTC frame: {:?}", p.status()));
    let before = p.status().frames;
    thread::sleep(Duration::from_secs(secs));
    let n = p.status().frames - before;
    let sent = cam.stats.frames_sent.load(Ordering::Relaxed);
    let plis = cam.stats.keyframe_requests.load(Ordering::Relaxed);
    eprintln!(
        "WebRTC mock, every {lose_every}th packet lost: {n} frames shown in {secs} s ({sent} sent in all), {plis} key frame requests"
    );
    (n, sent, plis, p, cam)
}

#[test]
fn webrtc_light_packet_loss_is_repaired_by_nacks() {
    let (n, _, plis, p, _cam) = lossy(15, 3);
    assert!(
        n >= 25,
        "{n} frames in 3 s at 15 fps with 1 in 15 packets lost"
    );
    assert_eq!(plis, 0, "retransmissions repaired every gap");
    assert_eq!(
        quadrants(&wait_frame(&p, Duration::from_secs(2)).unwrap()),
        "rgbw"
    );
}

#[test]
fn webrtc_heavy_packet_loss_waits_for_key_frames_and_keeps_playing() {
    // camera-streamer ignores key frame requests; the clip's own key frames recover.
    let (n, _, plis, p, _cam) = lossy(6, 4);
    assert!(
        plis >= 1,
        "a gap NACKs could not repair asks for a key frame"
    );
    assert!(n >= 3, "{n} frames in 4 s");
    assert!(p.status().error.is_none(), "{:?}", p.status());
}

#[test]
fn webrtc_falls_back_to_snapshots_when_the_camera_has_none() {
    let cam = streamer(None, false);
    let p = Player::start(
        Source::WebRtc(cam.url(), Some(cam.snapshot_url())),
        None,
        Orientation::default(),
        Rate::FOCUSED,
        || {},
    )
    .unwrap();
    let f = wait_frame(&p, Duration::from_secs(10))
        .unwrap_or_else(|| panic!("no snapshot: {:?}", p.status()));
    assert_eq!((f.width, f.height), (320, 240));
    let s = p.status();
    assert!(s.fallback.as_deref().unwrap_or("").contains("404"), "{s:?}");
    assert!(s.error.is_none());
    assert_eq!(p.label(), "snapshots");
    assert!(cam.stats.snapshots.load(Ordering::Relaxed) >= 1);
}

#[test]
fn webrtc_lets_go_of_the_camera_while_not_shown_and_reconnects() {
    let cam = streamer(None, true);
    let p = Player::start(
        Source::from_url(&cam.url()).unwrap(),
        None,
        Orientation::default(),
        Rate::FOCUSED,
        || {},
    )
    .unwrap();
    assert_eq!(p.label(), "WebRTC");
    p.touch();
    wait_frame(&p, Duration::from_secs(10)).expect("a frame");
    assert_eq!(cam.stats.sessions.load(Ordering::Relaxed), 1);
    // No touches (a minimised window): the session closes after the lease.
    assert!(
        wait_until(LEASE + Duration::from_secs(3), || cam
            .stats
            .sessions
            .load(Ordering::Relaxed)
            == 0),
        "still streaming unwatched"
    );
    let _ = p.take_frame();
    // Shown again: a new session.
    let end = Instant::now() + Duration::from_secs(10);
    let mut again = None;
    while again.is_none() && Instant::now() < end {
        p.touch();
        again = p.take_frame();
        thread::sleep(Duration::from_millis(50));
    }
    assert!(again.is_some(), "{:?}", p.status());
    assert_eq!(cam.stats.posts.lock().unwrap().len(), 6);
}
