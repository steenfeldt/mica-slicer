// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/Utils/OctoPrint.cpp :: PrusaConnect::set_http_post_header_args, PrusaLink::test_with_method_check, PrusaLink::post_inner, PrusaLink::put_inner
// Source: prusaslicer@b028299c77 src/slic3r/Utils/OctoPrint.hpp :: PrusaConnect
// Changes: Streamed Rust transport; fixed cloud origin, injectable loopback mocks; English acceptance messages; no retries or status queries; reject queue on PUT rather than silently dropping it.

use crate::{
    http::{Http, remote_path, response_bytes},
    *,
};
use std::path::Path;

pub const CONNECT_URL: &str = "https://connect.prusa3d.com";

/// Per-printer API-key transport. This is independent of Prusa Account OAuth.
pub struct PrusaConnect {
    http: Http,
    key: String,
}
impl PrusaConnect {
    pub fn new(mut config: PrinterConfig, timeouts: Timeouts) -> Result<Self> {
        let key = match &config.credentials {
            Credentials::ApiKey(k) if !k.trim().is_empty() => k.clone(),
            _ => {
                return Err(Error::protocol(
                    "Prusa Connect requires its per-printer Connect API key",
                ));
            }
        };
        if config.address.trim().is_empty() {
            config.address = CONNECT_URL.into();
        }
        let http = Http::new(config, timeouts, false)?;
        // Never send a cloud key to a mistyped host. Loopback is the explicit mock seam.
        let uri: ureq::http::Uri = http
            .base
            .parse()
            .map_err(|_| Error::protocol("invalid Connect address"))?;
        let local = uri
            .host()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback())
            && uri.path() == "/";
        if http.base != CONNECT_URL && !local {
            return Err(Error::protocol(
                "Prusa Connect uses https://connect.prusa3d.com (only loopback mocks may override it)",
            ));
        }
        Ok(Self { http, key })
    }
    fn version(&self) -> Result<(serde_json::Value, bool)> {
        let v = self
            .http
            .call("GET", "/api/version", None)
            .map_err(connect_error)?;
        if v["api"].as_str().is_none()
            || !v["text"]
                .as_str()
                .is_some_and(|s| s.starts_with("PrusaLink") || s.starts_with("OctoPrint"))
        {
            return Err(Error::protocol(
                "mismatched print host or invalid Connect /api/version response",
            ));
        }
        let put = v["capabilities"]["upload-by-put"]
            .as_bool()
            .unwrap_or(false);
        Ok((v, put))
    }
    fn unsupported<T>() -> Result<T> {
        Err(Error::protocol(
            "The Connect API-key connection only sends files; open Prusa Connect for status and printer controls",
        ))
    }
}
impl PrinterHost for PrusaConnect {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            upload: true,
            queue: true,
            ..Capabilities::default()
        }
    }
    fn info(&self) -> Result<PrinterInfo> {
        let (v, _) = self.version()?;
        Ok(PrinterInfo {
            model: None,
            firmware: string(&v["server"]),
            state: PrinterState::Offline,
            tools: None,
            formats: vec![FileFormat::Gcode, FileFormat::Bgcode],
            bed_type: None,
        })
    }
    fn upload_with_progress(
        &self,
        path: &Path,
        remote_name: &str,
        start_after: bool,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<Job> {
        self.upload_action_with_progress(
            path,
            remote_name,
            if start_after {
                UploadAction::Print
            } else {
                UploadAction::UploadOnly
            },
            progress,
        )
        .map(|o| o.job)
    }
    fn upload_action_with_progress(
        &self,
        path: &Path,
        remote_name: &str,
        action: UploadAction,
        progress: &mut ProgressCallback<'_>,
    ) -> Result<UploadOutcome> {
        let encoded = remote_path(remote_name)?;
        if FileFormat::from_path(Path::new(remote_name)).is_none() {
            return Err(Error::protocol(
                "Prusa Connect requires a .gcode or .bgcode file",
            ));
        }
        let (_, put) = self.version()?;
        let response = if put {
            if action == UploadAction::Queue {
                return Err(Error::protocol(
                    "Connect advertised PUT uploads, which cannot add to queue; no file was sent",
                ));
            }
            let mut headers = vec![
                ("Content-Type", "text/x.gcode".into()),
                ("Overwrite", "?1".into()),
            ];
            if action == UploadAction::Print {
                headers.push(("Print-After-Upload", "?1".into()));
            }
            self.http
                .upload_response(
                    "PUT",
                    &format!("/api/v1/files/local/{encoded}"),
                    path,
                    headers,
                    (vec![], vec![]),
                    progress,
                )
                .map_err(connect_error)?
        } else {
            let boundary = crate::moonraker::boundary(path)?;
            let (parent, filename) = remote_name.rsplit_once('/').unwrap_or(("", remote_name));
            let mut prefix = String::new();
            let mut field = |name: &str, value: &str| {
                prefix.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"));
            };
            match action {
                UploadAction::Print => field("to_print", "True"),
                UploadAction::Queue => field("to_queue", "True"),
                UploadAction::UploadOnly => {}
            }
            field("path", parent);
            prefix.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"));
            self.http
                .upload_response(
                    "POST",
                    "/api/files/local",
                    path,
                    vec![
                        ("Accept-Language", "en".into()),
                        (
                            "Content-Type",
                            format!("multipart/form-data; boundary={boundary}"),
                        ),
                    ],
                    (
                        prefix.into_bytes(),
                        format!("\r\n--{boundary}--\r\n").into_bytes(),
                    ),
                    progress,
                )
                .map_err(connect_error)?
        };
        let warning = response.status().as_u16() == 202;
        let bytes = response_bytes(response)?;
        // Server text is untrusted: strip controls, bound display length, redact before truncation.
        let text = String::from_utf8_lossy(&bytes).replace(&self.key, "[redacted]");
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(2048)
            .collect();
        let text = text.trim();
        let message = if text.is_empty() {
            warning.then(|| "Connect accepted the file with a warning. Check the printer's queue in Prusa Connect; printing is not confirmed.".into())
        } else {
            Some(text.into())
        };
        Ok(UploadOutcome {
            job: Job {
                name: remote_name.into(),
                id: None,
                started: false,
            },
            message,
            warning,
        })
    }
    fn start(&self, _: &str) -> Result<()> {
        Self::unsupported()
    }
    fn pause(&self) -> Result<()> {
        Self::unsupported()
    }
    fn resume(&self) -> Result<()> {
        Self::unsupported()
    }
    fn cancel(&self) -> Result<()> {
        Self::unsupported()
    }
    fn status(&self) -> Result<PrinterStatus> {
        Self::unsupported()
    }
    fn preflight(&self, _: &JobRequirements) -> Vec<Issue> {
        vec![Issue { kind: IssueKind::Connection, tool: None,
            message: "Connect API-key uploads cannot check printer readiness, tools, materials or bed. Verify them in Prusa Connect before sending.".into() }]
    }
}

fn connect_error(mut error: Error) -> Error {
    error.message = match error.http_status {
        Some(401 | 403) => "Prusa Connect refused the per-printer API key".into(),
        Some(409) => "Prusa Connect reported a conflict; check the printer and queue before retrying".into(),
        Some(423) => "Prusa Connect reported a locked resource; check Connect before retrying".into(),
        Some(429) => "Prusa Connect rate limited the request; wait before retrying".into(),
        Some(code) => format!("Prusa Connect returned HTTP {code}"),
        None if error.kind == ErrorKind::Unreachable => "Prusa Connect unreachable or request timed out; if upload began, check Connect before retrying".into(),
        None => error.message,
    };
    error
}
