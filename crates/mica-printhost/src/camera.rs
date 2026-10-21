// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Camera discovery: what a printer says about its cameras, turned into absolute URLs. Only
//! discovery lives here; playing a stream is `mica-camera`'s job.
//!
//! * Moonraker: `GET /server/webcams/list` (Moonraker docs, `docs/external_api/webcams.md`;
//!   the service names are the table in `docs/configuration.md`, `[webcam]` section).
//! * PrusaLink: `GET /api/v1/cameras` and `GET /api/v1/cameras/<id>/snap` exist in Prusa-Link
//!   (the Raspberry Pi daemon, `prusa/link/web/cameras.py`) and need the printer's own API
//!   key or digest login. The Buddy firmware (MK4, XL, CORE One) routes only `storage`,
//!   `info`, `job`, `status`, `transfer` and `files` under `/api/v1/`
//!   (`lib/WUI/link_content/prusa_link_api_v1.cpp`), so it answers 404 and has no camera
//!   over PrusaLink; its cameras talk to Prusa Connect only.

use crate::string;
use serde_json::Value;

/// How a camera streams, from Moonraker's `service` (or guessed from a URL's scheme).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraService {
    /// `multipart/x-mixed-replace` JPEG over HTTP (`mjpegstreamer`, `uv4l-mjpeg`).
    Mjpeg,
    /// Repeated JPEG snapshots (`mjpegstreamer-adaptive`, PrusaLink).
    Snapshot,
    /// RTSP (an `ipstream` camera with an `rtsp://` URL).
    Rtsp,
    /// WebRTC (`webrtc-camerastreamer`, `-go2rtc`, `-mediamtx`, `-janus`): camera-streamer's
    /// is played (`mica-camera`); for the others the snapshot URL is used instead.
    WebRtc,
    /// HLS (`hlsstream`): not played; the snapshot URL is used instead.
    Hls,
    /// Anything else (`jmuxer-stream`, `iframe`, unknown names).
    Other,
}
impl CameraService {
    /// Moonraker's service name, with the stream URL deciding what an `ipstream` is.
    pub fn from_moonraker(service: &str, stream_url: Option<&str>) -> Self {
        let s = service.to_ascii_lowercase();
        match s.as_str() {
            "mjpegstreamer" | "uv4l-mjpeg" | "" => Self::Mjpeg,
            "mjpegstreamer-adaptive" => Self::Snapshot,
            "hlsstream" => Self::Hls,
            "ipstream" => match stream_url {
                Some(u) if is_rtsp(u) => Self::Rtsp,
                Some(_) => Self::Mjpeg,
                None => Self::Other,
            },
            _ if s.starts_with("webrtc") => Self::WebRtc,
            _ => Self::Other,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Mjpeg => "MJPEG",
            Self::Snapshot => "snapshots",
            Self::Rtsp => "RTSP",
            Self::WebRtc => "WebRTC",
            Self::Hls => "HLS",
            Self::Other => "other",
        }
    }
}

/// One camera a printer reports. URLs are absolute; neither carries credentials.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraInfo {
    pub name: String,
    pub service: CameraService,
    /// The service name as the printer reported it (`webrtc-camerastreamer`).
    pub service_name: String,
    pub stream_url: Option<String>,
    pub snapshot_url: Option<String>,
    /// A snapshot served by the printer's own API, which needs the printer's credentials
    /// (PrusaLink): fetch it with `PrinterHost::camera_snapshot`, never with a bare GET.
    pub printer_snapshot: Option<String>,
    /// Clockwise degrees (0, 90, 180, 270).
    pub rotation: u16,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    pub target_fps: Option<f32>,
}
impl CameraInfo {
    pub(crate) fn named(name: String, service: CameraService) -> Self {
        Self {
            name,
            service,
            service_name: String::new(),
            stream_url: None,
            snapshot_url: None,
            printer_snapshot: None,
            rotation: 0,
            flip_horizontal: false,
            flip_vertical: false,
            target_fps: None,
        }
    }
}

pub fn is_rtsp(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("rtsp://") || u.starts_with("rtsps://")
}

/// `scheme://authority` of an absolute URL.
fn origin(url: &str) -> &str {
    url.find("://")
        .map(|i| {
            let rest = &url[i + 3..];
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            &url[..i + 3 + end]
        })
        .unwrap_or(url)
}

/// Where a relative webcam URL lives. Mainsail and Fluidd resolve relative URLs against the
/// page they are served from: the web server on the printer's host (port 80), which proxies
/// `/webcam/`. Moonraker's own API port (7125 by default) does not serve them, so an address
/// on port 7125 resolves to the same host without the port; any other address (a reverse
/// proxy, the mock printers) keeps its origin. An assumption, made visible in the UI.
pub fn web_origin(base: &str) -> String {
    let o = origin(base);
    match o.rsplit_once(':') {
        Some((head, "7125")) if head.contains("://") => head.to_string(),
        _ => o.to_string(),
    }
}

/// An absolute, credential-free camera URL, or `None` when it cannot be made one.
pub(crate) fn resolve(base: &str, url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let absolute = ["http://", "https://", "rtsp://", "rtsps://"]
        .iter()
        .any(|s| lower.starts_with(s));
    let resolved = if absolute {
        u.to_string()
    } else if u.starts_with("//") {
        return None;
    } else if u.starts_with('/') {
        format!("{}{u}", web_origin(base))
    } else {
        format!("{}/{u}", web_origin(base))
    };
    // Credentials in a URL would be logged and shown; such a camera needs the manual field.
    let authority = resolved
        .split_once("://")
        .map(|(_, r)| r.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    (!authority.is_empty() && !authority.contains('@')).then_some(resolved)
}

/// `/server/webcams/list` result → cameras, disabled ones left out.
pub(crate) fn parse_moonraker(base: &str, result: &Value) -> Vec<CameraInfo> {
    let Some(list) = result["webcams"].as_array() else {
        return vec![];
    };
    list.iter()
        .filter(|w| w["enabled"].as_bool() != Some(false))
        .enumerate()
        .map(|(i, w)| {
            let stream = string(&w["stream_url"]).and_then(|u| resolve(base, &u));
            let service_name = string(&w["service"]).unwrap_or_else(|| "mjpegstreamer".into());
            let service = CameraService::from_moonraker(&service_name, stream.as_deref());
            let mut c = CameraInfo::named(
                string(&w["name"]).unwrap_or_else(|| format!("Camera {}", i + 1)),
                service,
            );
            c.service_name = service_name;
            c.stream_url = stream;
            c.snapshot_url = string(&w["snapshot_url"]).and_then(|u| resolve(base, &u));
            c.rotation = match w["rotation"].as_u64() {
                Some(r @ (90 | 180 | 270)) => r as u16,
                _ => 0,
            };
            c.flip_horizontal = w["flip_horizontal"].as_bool().unwrap_or(false);
            c.flip_vertical = w["flip_vertical"].as_bool().unwrap_or(false);
            c.target_fps = w["target_fps"]
                .as_f64()
                .filter(|f| f.is_finite() && *f > 0.)
                .map(|f| f as f32);
            c
        })
        .filter(|c| c.stream_url.is_some() || c.snapshot_url.is_some())
        .collect()
}

/// Prusa-Link's `GET /api/v1/cameras` → the connected cameras, in the printer's order.
pub(crate) fn parse_prusalink(v: &Value) -> Vec<CameraInfo> {
    let Some(list) = v["camera_list"].as_array() else {
        return vec![];
    };
    list.iter()
        .filter(|c| c["connected"].as_bool() == Some(true))
        .filter_map(|c| {
            let id = string(&c["camera_id"])?;
            // Ids are opaque; anything that is not a plain path segment is refused.
            if !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return None;
            }
            let name = string(&c["config"]["name"]).unwrap_or_else(|| id.clone());
            let mut cam = CameraInfo::named(name, CameraService::Snapshot);
            cam.service_name = "prusalink".into();
            cam.printer_snapshot = Some(format!("/api/v1/cameras/{id}/snap"));
            Some(cam)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn moonraker_cameras_resolve_against_the_web_server() {
        let v = json!({"webcams":[
            {"name":"Bed","service":"mjpegstreamer","enabled":true,"stream_url":"/webcam/?action=stream","snapshot_url":"/webcam/?action=snapshot","rotation":90,"flip_vertical":true,"target_fps":15},
            {"name":"Off","enabled":false,"stream_url":"/x"},
            {"name":"Door","service":"webrtc-camerastreamer","stream_url":"http://cam.lan/webrtc","snapshot_url":"http://cam.lan/snapshot"},
            {"name":"IP","service":"ipstream","stream_url":"rtsp://198.51.100.3:554/live"},
            {"name":"Secret","service":"mjpegstreamer","stream_url":"http://admin:pw@cam.lan/stream"},
        ]});
        let c = parse_moonraker("http://u1.local:7125", &v);
        assert_eq!(
            c.len(),
            3,
            "disabled and credential-bearing cameras are left out"
        );
        assert_eq!(c[0].service, CameraService::Mjpeg);
        assert_eq!(
            c[0].stream_url.as_deref(),
            Some("http://u1.local/webcam/?action=stream")
        );
        assert_eq!(c[0].rotation, 90);
        assert!(c[0].flip_vertical);
        assert_eq!(c[0].target_fps, Some(15.));
        assert_eq!(c[1].service, CameraService::WebRtc);
        assert_eq!(
            c[1].snapshot_url.as_deref(),
            Some("http://cam.lan/snapshot")
        );
        assert_eq!(c[2].service, CameraService::Rtsp);
        // Another port (a proxy, the mock) keeps the origin.
        let c = parse_moonraker("http://127.0.0.1:18082/", &v);
        assert_eq!(
            c[0].snapshot_url.as_deref(),
            Some("http://127.0.0.1:18082/webcam/?action=snapshot")
        );
        assert_eq!(web_origin("https://u1.local:7125/api"), "https://u1.local");
    }

    #[test]
    fn prusalink_lists_connected_cameras_with_printer_snapshots() {
        let v = json!({"camera_list":[
            {"camera_id":"abc-1","config":{"name":"Nozzle cam"},"connected":true},
            {"camera_id":"gone","config":{"name":"Old"},"connected":false},
            {"camera_id":"../etc","connected":true},
        ]});
        let c = parse_prusalink(&v);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "Nozzle cam");
        assert_eq!(
            c[0].printer_snapshot.as_deref(),
            Some("/api/v1/cameras/abc-1/snap")
        );
        assert_eq!(c[0].snapshot_url, None);
    }
}
