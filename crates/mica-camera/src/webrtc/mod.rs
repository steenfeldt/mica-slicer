// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! WebRTC from camera-streamer (the `webrtc-camerastreamer` service of Moonraker; the U1's
//! case camera), receive only. Written from camera-streamer's HTTP handler
//! (`output/webrtc/webrtc.cc`, `http_webrtc_offer`) and its page `html/webrtc.html`, and
//! checked against Mainsail's `WebrtcCameraStreamer.vue`; all three use the same signalling,
//! three JSON POSTs to the stream URL (`/webcam/webrtc`):
//!
//! 1. `{"type":"request","keepAlive":true}` → camera-streamer makes a peer connection, waits
//!    for its ICE candidates (up to 3 s) and answers `{"id":"rtc-…","type":"offer","sdp":…,
//!    "keepAlive":true,"iceServers":[…]}` (an H.264 video track, payload type 102, and a
//!    `keepalive` data channel).
//! 2. `{"type":"answer","id":…,"sdp":…}` with Mica's answer (its one LAN address in it).
//! 3. `{"type":"remote_candidate","id":…,"candidates":[{"candidate":…,"sdpMid":…}]}` with the
//!    same address again, as browsers trickle it (camera-streamer ignores duplicates).
//!
//! Then camera-streamer sends H.264 over RTP (SRTP, after ICE and DTLS on one UDP port) from
//! the next key frame, and `ping` on the data channel every second; Mica answers `pong`
//! (without one for 30 s camera-streamer closes the stream). str0m (sans-IO) does ICE, DTLS,
//! SRTP, NACKs and the H.264 depacketisation (single NAL units, STAP-A, FU-A) into Annex B
//! access units; the operating system decoder (`mica-videodec`) decodes them. After a gap
//! (lost packets) frames are skipped until the next key frame, and one is asked for (PLI).
//!
//! Only host candidates on this network are used: the camera's own address first, else its
//! other private (LAN) addresses; no STUN or TURN servers, nothing on the internet. The UDP
//! socket binds the local address that routes to the camera. In a loopback-only run (tests,
//! the monkey) only 127.0.0.1 candidates are accepted.

mod crypto;

pub(crate) use crypto::provider;

use crate::{Auth, Ctx, http};
use serde_json::Value;
use std::{
    io::{ErrorKind, Read},
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    time::{Duration, Instant},
};
use str0m::{
    Candidate, CandidateKind, Event, IceConnectionState, Input, Output, Rtc, RtcConfig,
    change::SdpOffer,
    channel::ChannelId,
    media::{KeyframeRequestKind, Mid},
    net::{Protocol, Receive},
};

/// ICE and DTLS must be done within this time.
const CONNECT: Duration = Duration::from_secs(10);
/// No video for this long ends the session (it is then retried with back-off).
const STALL: Duration = Duration::from_secs(10);
/// How long an incomplete frame waits for its missing packets.
const REORDER_WAIT: Duration = Duration::from_millis(250);
/// At most one key frame request this often.
const PLI_EVERY: Duration = Duration::from_secs(1);
/// Signalling answers are small JSON; more is refused.
const MAX_JSON: u64 = 256 * 1024;

/// The JSON bodies Mica POSTs to camera-streamer, in order.
pub mod signal {
    /// A JSON string literal.
    fn s(v: &str) -> String {
        serde_json::Value::from(v).to_string()
    }
    /// 1. Ask for an offer. No ICE servers: LAN only.
    pub fn request() -> String {
        r#"{"type":"request","keepAlive":true}"#.to_string()
    }
    /// 2. The answer to offer `id`.
    pub fn answer(id: &str, sdp: &str) -> String {
        format!(r#"{{"type":"answer","id":{},"sdp":{}}}"#, s(id), s(sdp))
    }
    /// 3. Mica's candidate for offer `id` (also in the answer's SDP).
    pub fn candidate(id: &str, candidate: &str, mid: &str) -> String {
        format!(
            r#"{{"type":"remote_candidate","id":{},"candidates":[{{"candidate":{},"sdpMid":{},"sdpMLineIndex":0}}]}}"#,
            s(id),
            s(candidate),
            s(mid)
        )
    }
}

/// POSTs `body` (JSON) and returns the JSON answer.
fn post(agent: &ureq::Agent, url: &str, auth: Option<&Auth>, body: &str) -> Result<Value, String> {
    let mut req = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    if let Some(a) = auth {
        req = req.header("Authorization", http::basic(a));
    }
    let mut response = http::checked(req.send(body)).map_err(|e| {
        if e.contains("HTTP 404") {
            "the camera has no WebRTC at this URL (HTTP 404)".to_string()
        } else {
            format!("WebRTC signalling: {e}")
        }
    })?;
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_JSON)
        .read_to_string(&mut text)
        .map_err(|_| "WebRTC signalling: the camera's answer was cut off".to_string())?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text)
        .map_err(|_| "WebRTC signalling: the camera's answer is not JSON".to_string())
}

/// The camera's address from its URL.
fn camera_ip(url: &str) -> Result<SocketAddr, String> {
    let u = url::Url::parse(url).map_err(|_| "invalid camera URL".to_string())?;
    let host = u.host_str().ok_or("the camera URL has no host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = u.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| "the camera's host name is unknown".to_string())?
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| "the camera's host name is unknown".to_string())
}

/// This computer's address on the way to `remote` (no packet is sent: connecting a UDP
/// socket only picks the route).
fn local_ip_towards(remote: SocketAddr) -> Result<IpAddr, String> {
    if remote.ip().is_loopback() {
        return Ok(remote.ip());
    }
    let any: SocketAddr = if remote.is_ipv4() {
        "0.0.0.0:0".parse().expect("valid")
    } else {
        "[::]:0".parse().expect("valid")
    };
    let probe = UdpSocket::bind(any).map_err(|e| format!("cannot open a UDP socket ({e})"))?;
    probe
        .connect(remote)
        .map_err(|_| "no network route to the camera".to_string())?;
    probe
        .local_addr()
        .map(|a| a.ip())
        .map_err(|e| format!("cannot open a UDP socket ({e})"))
}

/// A private, link-local or loopback address: on this network, not the internet.
fn on_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local() || v.is_loopback(),
        IpAddr::V6(v) => {
            v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 // unique local
        }
    }
}

/// The offer's SDP with only the candidates Mica may contact: UDP host candidates of the
/// camera's address family, the camera's own address if offered, else its LAN addresses
/// (loopback only in a loopback-only run). Fails when none is left.
pub(crate) fn usable_candidates(
    sdp: &str,
    camera: IpAddr,
    loopback_only: bool,
) -> Result<String, String> {
    let parsed: Vec<(usize, Option<Candidate>)> = sdp
        .lines()
        .enumerate()
        .filter_map(|(i, l)| {
            l.trim()
                .strip_prefix("a=")
                .filter(|a| a.starts_with("candidate:"))
                .map(|a| (i, Candidate::from_sdp_string(a).ok()))
        })
        .collect();
    let fits = |c: &Candidate| {
        let ip = c.addr().ip();
        c.kind() == CandidateKind::Host
            && c.proto() == Protocol::Udp
            && ip.is_ipv4() == camera.is_ipv4()
            && if loopback_only {
                ip.is_loopback()
            } else {
                on_lan(ip)
            }
    };
    let own = parsed.iter().any(|(_, c)| {
        c.as_ref()
            .is_some_and(|c| fits(c) && c.addr().ip() == camera)
    });
    let keep: Vec<usize> = parsed
        .iter()
        .filter(|(_, c)| {
            c.as_ref()
                .is_some_and(|c| fits(c) && (!own || c.addr().ip() == camera))
        })
        .map(|(i, _)| *i)
        .collect();
    if keep.is_empty() {
        return Err(if loopback_only {
            "the camera offered no WebRTC address on this computer".into()
        } else {
            "the camera offered no WebRTC address on this network".into()
        });
    }
    let drop: Vec<usize> = parsed
        .iter()
        .map(|(i, _)| *i)
        .filter(|i| !keep.contains(i))
        .collect();
    let mut out = String::with_capacity(sdp.len());
    for (i, l) in sdp.lines().enumerate() {
        if !drop.contains(&i) {
            out.push_str(l);
            out.push_str("\r\n");
        }
    }
    Ok(out)
}

/// Whether an Annex B access unit holds an IDR slice (a key frame).
fn has_idr(au: &[u8]) -> bool {
    mica_videodec::nal_units(au)
        .iter()
        .any(|n| n.first().is_some_and(|b| b & 0x1F == 5))
}

/// The session state that outlives one drain of str0m's output.
struct Session {
    socket: UdpSocket,
    local: SocketAddr,
    video: Option<Mid>,
    keepalive: Option<ChannelId>,
    connected: bool,
    /// Frames are skipped until the next key frame (start, or after lost packets).
    need_key: bool,
    last_pli: Option<Instant>,
    last_video: Instant,
    bad: u32,
}

pub fn run(ctx: &mut Ctx<'_>, url: &str, auth: Option<&Auth>) -> Result<(), String> {
    let camera = camera_ip(url)?;
    let local_ip = local_ip_towards(camera)?;
    let socket = UdpSocket::bind((local_ip, 0))
        .map_err(|e| format!("cannot open a UDP socket for WebRTC ({e})"))?;
    let local = socket
        .local_addr()
        .map_err(|e| format!("cannot open a UDP socket for WebRTC ({e})"))?;
    let agent = http::agent(false);

    // 1. Offer.
    let offer = post(&agent, url, auth, &signal::request())?;
    let (Some(id), Some(sdp)) = (offer["id"].as_str(), offer["sdp"].as_str()) else {
        return Err("the camera's WebRTC answer has no offer (is this camera-streamer?)".into());
    };
    if offer["type"].as_str() != Some("offer") {
        return Err("the camera's WebRTC answer has no offer (is this camera-streamer?)".into());
    }
    let sdp = usable_candidates(sdp, camera.ip(), crate::loopback_only())?;
    let offer = SdpOffer::from_sdp_string(&sdp)
        .map_err(|e| format!("the camera's WebRTC offer is unreadable ({e})"))?;

    let mut rtc = RtcConfig::new()
        .set_crypto_provider(provider())
        .clear_codecs()
        .enable_h264(true)
        // A packet a NACK did not bring back within this (a LAN round trip is a few ms)
        // drops its frame instead of holding the picture up to 2 s; the next key frame
        // recovers.
        .set_reordering_timeout_video(Some(REORDER_WAIT))
        .build(Instant::now());
    let candidate = Candidate::host(local, "udp")
        .map_err(|e| format!("cannot use this computer's address for WebRTC ({e})"))?;
    let candidate_line = candidate.to_sdp_string();
    rtc.add_local_candidate(candidate);
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| format!("the camera's WebRTC offer cannot be answered ({e})"))?;
    let answer = answer.to_sdp_string();
    if !answer.contains("H264") && !answer.contains("h264") {
        return Err("the camera's WebRTC stream is not H.264".into());
    }
    let mid = answer
        .lines()
        .find_map(|l| l.trim().strip_prefix("a=mid:"))
        .unwrap_or("0")
        .to_string();

    // 2. Answer, 3. candidate.
    post(&agent, url, auth, &signal::answer(id, &answer))?;
    post(
        &agent,
        url,
        auth,
        &signal::candidate(id, &candidate_line, &mid),
    )?;

    let mut decoder = crate::rtsp::H264Decoder::new()?;
    let mut s = Session {
        socket,
        local,
        video: None,
        keepalive: None,
        connected: false,
        need_key: true,
        last_pli: None,
        last_video: Instant::now(),
        bad: 0,
    };
    let started = Instant::now();
    let mut buf = vec![0u8; 2000];
    let result = loop {
        if ctx.stopped() || !ctx.watched() {
            break Ok(());
        }
        let timeout = match drain(&mut rtc, &mut s, ctx, &mut decoder) {
            Ok(t) => t,
            Err(e) => break Err(e),
        };
        if !s.connected && started.elapsed() > CONNECT {
            break Err(
                "the camera's WebRTC stream did not connect (UDP blocked between this \
                 computer and the camera?)"
                    .into(),
            );
        }
        if s.connected && s.last_video.elapsed() > STALL {
            break Err("no video from the camera for 10 s".into());
        }
        // Wake at least every 100 ms to notice a stop.
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .clamp(Duration::from_millis(1), Duration::from_millis(100));
        if s.socket.set_read_timeout(Some(wait)).is_err() {
            break Err("the WebRTC socket failed".into());
        }
        let input = match s.socket.recv_from(&mut buf) {
            Ok((n, source)) => match buf[..n].try_into() {
                Ok(contents) => Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: s.local,
                        contents,
                    },
                ),
                // Not STUN, DTLS or RTP: ignore it.
                Err(_) => continue,
            },
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Input::Timeout(Instant::now())
            }
            // An ICMP "port unreachable" from an earlier check; keep going.
            Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
            Err(e) => break Err(format!("the WebRTC socket failed ({e})")),
        };
        if let Err(e) = rtc.handle_input(input) {
            break Err(format!("WebRTC: {e}"));
        }
    };
    close(&mut rtc, &s.socket);
    result
}

/// Tells the camera we are gone (RTCP BYE, SCTP shutdown, DTLS close_notify), so it frees
/// the stream at once instead of after its ICE timeout.
fn close(rtc: &mut Rtc, socket: &UdpSocket) {
    if rtc.close().is_err() {
        return;
    }
    let end = Instant::now() + Duration::from_millis(200);
    while rtc.is_alive() && Instant::now() < end {
        match rtc.poll_output() {
            Ok(Output::Transmit(t)) => {
                let _ = socket.send_to(&t.contents, t.destination);
            }
            Ok(Output::Event(_)) => {}
            Ok(Output::Timeout(_)) | Err(_) => break,
        }
    }
    rtc.disconnect();
}

/// Drains str0m's output (sending, handling events) until it wants to wait; returns when.
fn drain(
    rtc: &mut Rtc,
    s: &mut Session,
    ctx: &mut Ctx<'_>,
    decoder: &mut crate::rtsp::H264Decoder,
) -> Result<Instant, String> {
    loop {
        let out = rtc.poll_output().map_err(|e| format!("WebRTC: {e}"))?;
        match out {
            Output::Timeout(t) => return Ok(t),
            Output::Transmit(t) => {
                let _ = s.socket.send_to(&t.contents, t.destination);
            }
            Output::Event(e) => match e {
                Event::Connected => {
                    s.connected = true;
                    s.last_video = Instant::now();
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    if rtc.is_alive() {
                        return Err("the camera's WebRTC connection dropped".into());
                    }
                }
                Event::MediaAdded(m) => s.video = Some(m.mid),
                Event::ChannelOpen(id, label) if label == "keepalive" => s.keepalive = Some(id),
                Event::ChannelData(d) if Some(d.id) == s.keepalive && d.data == b"ping" => {
                    if let Some(mut ch) = rtc.channel(d.id) {
                        let _ = ch.write(false, b"pong");
                    }
                }
                Event::MediaData(d) => {
                    s.last_video = Instant::now();
                    if !d.contiguous {
                        s.need_key = true;
                    }
                    if s.need_key {
                        if !has_idr(&d.data) {
                            request_keyframe(rtc, s, d.mid);
                            continue;
                        }
                        s.need_key = false;
                    }
                    match decoder.try_decode(&d.data, ctx.due()) {
                        Ok(Some((w, h, rgba))) => {
                            s.bad = 0;
                            ctx.publish_rgba(w, h, rgba)?;
                        }
                        Ok(None) => {}
                        Err(e) if e.is_fatal() => return Err(e.to_string()),
                        // Corrupt or missing data: wait for a key frame.
                        Err(e) => {
                            s.bad += 1;
                            if s.bad > 300 {
                                return Err(e.to_string());
                            }
                            s.need_key = true;
                            request_keyframe(rtc, s, d.mid);
                        }
                    }
                }
                _ => {}
            },
        }
    }
}

/// Asks the camera for a key frame (PLI), at most once a second.
fn request_keyframe(rtc: &mut Rtc, s: &mut Session, mid: Mid) {
    if s.last_pli.is_some_and(|t| t.elapsed() < PLI_EVERY) {
        return;
    }
    s.last_pli = Some(Instant::now());
    if let Some(mut w) = rtc.writer(s.video.unwrap_or(mid))
        && w.is_request_keyframe_possible(KeyframeRequestKind::Pli)
    {
        let _ = w.request_keyframe(None, KeyframeRequestKind::Pli);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OFFER: &str = "v=0\r\no=rtc 1 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n\
        a=group:BUNDLE video\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\n\
        a=mid:video\r\n\
        a=candidate:1 1 UDP 2122317823 192.168.0.10 40000 typ host\r\n\
        a=candidate:2 1 UDP 2122317567 172.16.0.1 40000 typ host\r\n\
        a=candidate:3 1 UDP 2122317311 fe80::1 40000 typ host\r\n\
        a=candidate:4 1 UDP 1686052607 203.0.113.2 40000 typ srflx raddr 192.168.0.10 rport 40000\r\n\
        a=candidate:5 1 UDP 2122316799 8.8.8.8 40000 typ host\r\n\
        a=candidate:6 1 UDP 2122316543 printer.local 40000 typ host\r\n\
        a=rtpmap:102 H264/90000\r\n";

    fn candidates(sdp: &str) -> Vec<String> {
        sdp.lines()
            .filter(|l| l.starts_with("a=candidate"))
            .map(|l| l.split(' ').nth(4).unwrap().to_string())
            .collect()
    }

    #[test]
    fn only_the_cameras_own_lan_address_is_contacted() {
        let ip: IpAddr = "192.168.0.10".parse().unwrap();
        let sdp = usable_candidates(OFFER, ip, false).unwrap();
        assert_eq!(candidates(&sdp), ["192.168.0.10"]);
        assert!(sdp.contains("a=rtpmap:102 H264/90000"));
        // Reached through another address (a proxy): its LAN addresses, never the internet.
        let other: IpAddr = "10.0.0.1".parse().unwrap();
        let sdp = usable_candidates(OFFER, other, false).unwrap();
        assert_eq!(candidates(&sdp), ["192.168.0.10", "172.16.0.1"]);
        // Loopback-only runs refuse all of them.
        assert!(usable_candidates(OFFER, ip, true).is_err());
    }

    #[test]
    fn the_signalling_bodies_are_camera_streamers() {
        assert_eq!(signal::request(), r#"{"type":"request","keepAlive":true}"#);
        let a: Value = serde_json::from_str(&signal::answer("rtc-abc", "v=0\r\n")).unwrap();
        assert_eq!(a, json!({"type":"answer","id":"rtc-abc","sdp":"v=0\r\n"}));
        let c: Value = serde_json::from_str(&signal::candidate(
            "rtc-abc",
            "candidate:1 1 udp 1 1.2.3.4 5 typ host",
            "video",
        ))
        .unwrap();
        assert_eq!(c["type"], "remote_candidate");
        assert_eq!(c["candidates"][0]["sdpMid"], "video");
    }
}
