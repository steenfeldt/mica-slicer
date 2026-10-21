// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! RTSP with H.264: `retina` (pure Rust) speaks RTSP and depacketizes RTP over the TCP
//! connection (interleaved; no UDP ports to open), the operating system's H.264 decoder
//! decodes (`mica-videodec`: VideoToolbox, Media Foundation, GStreamer). Both run on the
//! player's thread, in a single-threaded Tokio runtime that lives as long as the stream.
//! Every access unit is decoded (H.264 frames depend on each other); only frames that are
//! due are converted to RGBA and published.

use crate::{Auth, Ctx};
use futures_util::StreamExt;
use retina::{
    client::{PlayOptions, Session, SessionOptions, SetupOptions, TcpTransportOptions, Transport},
    codec::{CodecItem, FrameFormat},
};
use std::time::{Duration, Instant};

/// No video for this long ends the session (it is then retried with back-off).
const STALL: Duration = Duration::from_secs(10);

/// An H.264 decoder for Annex B access units (start codes, parameter sets in band), backed
/// by the operating system's decoder.
pub struct H264Decoder {
    decoder: mica_videodec::Decoder,
}
impl H264Decoder {
    pub fn new() -> Result<Self, String> {
        let decoder = mica_videodec::Decoder::new(mica_videodec::Codec::H264, None)
            .map_err(|e| e.to_string())?;
        Ok(Self { decoder })
    }
    /// Decodes one access unit. `Ok(None)`: no picture yet (parameter sets only, or a frame
    /// the decoder holds back). With `want` false the picture is decoded but not converted.
    pub fn decode(&mut self, au: &[u8], want: bool) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        self.try_decode(au, want).map_err(|e| e.to_string())
    }
    pub(crate) fn try_decode(
        &mut self,
        au: &[u8],
        want: bool,
    ) -> Result<Option<(u32, u32, Vec<u8>)>, mica_videodec::Error> {
        if !want {
            return self.decoder.decode_only(au).map(|()| None);
        }
        Ok(self
            .decoder
            .decode(au)?
            .map(|f| (f.width, f.height, f.rgba)))
    }
}

/// Tests: RTSP sessions to this port get a decoder that cannot start (the error path).
#[cfg(test)]
pub(crate) static BROKEN_DECODER_PORT: std::sync::atomic::AtomicU16 =
    std::sync::atomic::AtomicU16::new(0);

fn decoder_for(_url: &url::Url) -> Result<H264Decoder, String> {
    #[cfg(test)]
    if _url.port() == Some(BROKEN_DECODER_PORT.load(std::sync::atomic::Ordering::Relaxed)) {
        // Parameter sets that are no parameter sets: the system decoder refuses to start.
        let bogus = [0, 0, 0, 1, 0x67, 0xff, 0x00, 0x13, 0, 0, 0, 1, 0x68, 0x00];
        return mica_videodec::Decoder::new(mica_videodec::Codec::H264, Some(&bogus))
            .map(|decoder| H264Decoder { decoder })
            .map_err(|e| e.to_string());
    }
    H264Decoder::new()
}

pub fn run(ctx: &mut Ctx<'_>, url: &str, auth: Option<&Auth>) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| "cannot start the RTSP runtime".to_string())?;
    rt.block_on(play(ctx, url, auth))
}

fn describe_error(e: &retina::Error) -> String {
    let s = e.to_string();
    if s.contains("401") || s.to_ascii_lowercase().contains("unauthorized") {
        "the camera refused the login (RTSP 401)".into()
    } else if s.contains("404") {
        "the camera has no stream at this RTSP path (404)".into()
    } else if s.to_ascii_lowercase().contains("connection refused")
        || s.to_ascii_lowercase().contains("unable to connect")
    {
        "the camera is unreachable".into()
    } else {
        // retina's messages name the URL we gave it (credential-free) and the RTSP detail.
        format!("RTSP: {}", s.lines().next().unwrap_or(""))
    }
}

async fn play(ctx: &mut Ctx<'_>, url: &str, auth: Option<&Auth>) -> Result<(), String> {
    let url = url::Url::parse(url).map_err(|_| "invalid RTSP URL".to_string())?;
    let mut decoder = decoder_for(&url)?;
    let creds = auth.map(|a| retina::client::Credentials {
        username: a.username.clone(),
        password: a.password.clone(),
    });
    let options = SessionOptions::default()
        .creds(creds)
        .user_agent("Mica Slicer".into());
    let mut session =
        tokio::time::timeout(Duration::from_secs(10), Session::describe(url, options))
            .await
            .map_err(|_| "the camera did not answer in time".to_string())?
            .map_err(|e| describe_error(&e))?;
    let video = session
        .streams()
        .iter()
        .position(|s| s.media() == "video" && s.encoding_name().eq_ignore_ascii_case("h264"))
        .ok_or(
            "the RTSP stream has no H.264 video (H.265 and MJPEG over RTSP are not supported)",
        )?;
    session
        .setup(
            video,
            SetupOptions::default()
                .transport(Transport::Tcp(TcpTransportOptions::default()))
                .frame_format(FrameFormat::SIMPLE),
        )
        .await
        .map_err(|e| describe_error(&e))?;
    let mut demuxed = session
        .play(PlayOptions::default())
        .await
        .map_err(|e| describe_error(&e))?
        .demuxed()
        .map_err(|e| describe_error(&e))?;
    let mut last_video = Instant::now();
    let mut bad = 0u32;
    loop {
        if ctx.stopped() {
            return Ok(());
        }
        let item = match tokio::time::timeout(Duration::from_millis(250), demuxed.next()).await {
            Err(_) => {
                if last_video.elapsed() > STALL {
                    return Err("no video from the camera for 10 s".into());
                }
                continue;
            }
            Ok(None) => return Err("the camera ended the stream".into()),
            Ok(Some(Err(e))) => return Err(describe_error(&e)),
            Ok(Some(Ok(item))) => item,
        };
        let CodecItem::VideoFrame(frame) = item else {
            continue;
        };
        last_video = Instant::now();
        match decoder.try_decode(frame.data(), ctx.due()) {
            Ok(Some((w, h, rgba))) => {
                bad = 0;
                ctx.publish_rgba(w, h, rgba)?;
            }
            Ok(None) => {}
            // No decoder for this stream on this computer: say so now.
            Err(e) if e.is_fatal() => return Err(e.to_string()),
            // A lost packet spoils frames until the next key frame; give up only when it
            // keeps happening.
            Err(e) => {
                bad += 1;
                if bad > 300 {
                    return Err(e.to_string());
                }
            }
        }
    }
}
