// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Plays one camera without the GUI and prints what arrives, once a second: for measuring a
//! player's frame rate and CPU (`ps` on this process) against the mock cameras.
//!
//! ```sh
//! cargo run --profile fast -p mica-camera --example camera_probe -- URL [SECONDS] [--snapshot URL] [--fps N]
//! ```
//!
//! The URL's kind is guessed as for a manual camera URL (`…/webrtc` → camera-streamer's
//! WebRTC, `….jpg` → snapshots, `rtsp://` → RTSP, else MJPEG). `--snapshot` is the WebRTC
//! fallback. Only the given URLs are contacted.
//!
//! With the features `mock,openh264`, `camera_probe --serve WxH PORT FPS` runs a mock
//! camera-streamer on 127.0.0.1:PORT instead, streaming the test picture at that size,
//! encoded live (to measure a full-HD stream from another process).

use mica_camera::{Orientation, Player, Rate, Source};
use std::time::{Duration, Instant};

#[cfg(feature = "mock")]
fn serve(mut args: impl Iterator<Item = String>) {
    let size = args.next().expect("WxH");
    let (w, h) = size.split_once('x').expect("WxH");
    let (w, h): (u32, u32) = (w.parse().expect("width"), h.parse().expect("height"));
    let port: u16 = args.next().expect("PORT").parse().expect("port");
    let fps: f32 = args.next().map_or(15., |f| f.parse().expect("fps"));
    let cam = mica_camera::mock::CameraStreamer::start(
        port,
        mica_camera::mock::StreamerOptions {
            fps,
            render: std::sync::Arc::new(move |t| mica_camera::mock::test_pattern(w, h, t)),
            lose_every: None,
            webrtc: true,
        },
    )
    .expect("bind");
    println!("serving {} ({w}x{h} at {fps} fps)", cam.url());
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn main() {
    #[cfg(feature = "mock")]
    if std::env::args().nth(1).as_deref() == Some("--serve") {
        return serve(std::env::args().skip(2));
    }
    let mut args = std::env::args().skip(1);
    let mut url = None;
    let mut seconds = 10u64;
    let mut snapshot = None;
    let mut fps = 15.;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--snapshot" => snapshot = args.next(),
            "--fps" => fps = args.next().and_then(|v| v.parse().ok()).expect("--fps N"),
            s if url.is_none() => url = Some(s.to_string()),
            s => seconds = s.parse().expect("seconds"),
        }
    }
    let url = url.expect("usage: camera_probe URL [SECONDS] [--snapshot URL] [--fps N]");
    let source = match Source::from_url(&url).unwrap_or_else(|e| panic!("{e}")) {
        Source::WebRtc(u, _) => Source::WebRtc(u, snapshot),
        s => s,
    };
    let player = Player::start(
        source,
        None,
        Orientation::default(),
        Rate { fps, max_width: 0 },
        || {},
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let start = Instant::now();
    let mut last = 0;
    let mut size = (0, 0);
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(1));
        player.touch();
        if let Some(f) = player.take_frame() {
            size = (f.width, f.height);
        }
        let s = player.status();
        println!(
            "{:>3} s  {:<9}  {:>3} frames/s  {}x{}  {}",
            start.elapsed().as_secs(),
            player.label(),
            s.frames - last,
            size.0,
            size.1,
            s.error.as_deref().or(s.fallback.as_deref()).unwrap_or("ok")
        );
        last = s.frames;
    }
}
