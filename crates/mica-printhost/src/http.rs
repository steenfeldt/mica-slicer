// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

use crate::{
    Credentials, Error, ErrorKind, PrinterConfig, ProgressCallback, Result, Timeouts,
    UploadProgress,
};
use serde_json::Value;
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    time::Duration,
};
use ureq::{
    Agent, Body, SendBody,
    http::{Request, Response, Uri},
};

pub(crate) struct Http {
    pub base: String,
    pub timeouts: Timeouts,
    credentials: Credentials,
    normal: Agent,
    upload: Agent,
}
impl Http {
    pub fn new(config: PrinterConfig, timeouts: Timeouts, digest: bool) -> Result<Self> {
        if !digest && matches!(config.credentials, Credentials::Digest { .. }) {
            return Err(Error::protocol(
                "Moonraker does not support PrusaLink digest credentials",
            ));
        }
        let address = config.address.trim().trim_end_matches('/');
        let base = if address.contains("://") {
            address.to_owned()
        } else {
            format!("http://{address}")
        };
        let uri: Uri = base
            .parse()
            .map_err(|_| Error::protocol("invalid printer address"))?;
        if !matches!(uri.scheme_str(), Some("http" | "https"))
            || uri.host().is_none()
            || uri.authority().is_some_and(|a| a.as_str().contains('@'))
            || uri.query().is_some()
            || base.contains('#')
        {
            return Err(Error::protocol(
                "address must be an HTTP(S) URL without credentials, query or fragment",
            ));
        }
        if [timeouts.connect, timeouts.request, timeouts.upload]
            .iter()
            .any(Duration::is_zero)
        {
            return Err(Error::protocol("timeouts must be positive"));
        }
        let agent = |timeout| {
            let config = Agent::config_builder()
                .timeout_connect(Some(timeouts.connect))
                .timeout_global(Some(timeout))
                .max_redirects(0)
                .http_status_as_error(false)
                .proxy(None)
                .build();
            #[cfg(test)]
            if let Some(agent) = crate::tests::memory_agent(&base, config.clone()) {
                return agent;
            }
            Agent::new_with_config(config)
        };
        let normal = agent(timeouts.request);
        let upload = agent(timeouts.upload);
        Ok(Self {
            base,
            timeouts,
            credentials: config.credentials,
            normal,
            upload,
        })
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        auth: Option<&str>,
    ) -> ureq::http::request::Builder {
        let mut req = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base));
        if let Credentials::ApiKey(key) = &self.credentials {
            req = req.header("X-Api-Key", key);
        }
        if let Some(auth) = auth {
            req = req.header("Authorization", auth);
        }
        for (name, value) in headers {
            req = req.header(*name, value);
        }
        req
    }
    fn checked(response: Response<Body>) -> Result<Response<Body>> {
        let code = response.status().as_u16();
        if (200..300).contains(&code) {
            return Ok(response);
        }
        Err(Error {
            kind: match code {
                401 | 403 => ErrorKind::Unauthorised,
                409 | 423 | 429 => ErrorKind::Busy,
                _ => ErrorKind::Protocol,
            },
            message: format!("printer returned HTTP {code}"),
            http_status: Some(code),
        })
    }
    fn auth(&self, response: &Response<Body>, method: &str, path: &str) -> Result<Option<String>> {
        if response.status().as_u16() != 401 {
            return Ok(None);
        }
        let Credentials::Digest { username, password } = &self.credentials else {
            return Ok(None);
        };
        let challenge = response
            .headers()
            .get("www-authenticate")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| Error::protocol("missing digest challenge"))?;
        // Digest URI includes any configured reverse-proxy prefix.
        let uri: Uri = format!("{}{path}", self.base)
            .parse()
            .map_err(|_| Error::protocol("invalid request URI"))?;
        crate::digest::authorization(
            challenge,
            username,
            password,
            method,
            uri.path_and_query().map_or("/", |p| p.as_str()),
        )
        .map(Some)
    }
    pub fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let headers = if body.is_some() {
            vec![("Content-Type", "application/json".into())]
        } else {
            vec![]
        };
        let data = body.map(|v| v.to_string()).unwrap_or_default();
        let send = |auth: Option<&str>| -> Result<Response<Body>> {
            let req = self
                .request(method, path, &headers, auth)
                .body(data.as_bytes())
                .map_err(|_| Error::protocol("invalid HTTP request"))?;
            self.normal.run(req).map_err(network_error)
        };
        let response = send(None)?;
        let response = if let Some(auth) = self.auth(&response, method, path)? {
            drop(response);
            send(Some(&auth))?
        } else {
            response
        };
        decode(Self::checked(response)?)
    }
    /// The API key, for transports that are not this agent (the WebSocket).
    pub fn api_key(&self) -> Option<&str> {
        match &self.credentials {
            Credentials::ApiKey(k) => Some(k),
            _ => None,
        }
    }
    /// Like `call`, but a refusal keeps the printer's own short reason (Moonraker's
    /// `error.message`, e.g. a Klipper macro error) in the message, sanitized and capped.
    /// For commands whose refusal the user must read; never used for secret-bearing paths.
    pub fn call_reporting(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let data = body.map(|v| v.to_string()).unwrap_or_default();
        let headers = vec![("Content-Type", "application/json".to_string())];
        let req = self
            .request(method, path, &headers, None)
            .body(data.as_bytes())
            .map_err(|_| Error::protocol("invalid HTTP request"))?;
        let mut response = self.normal.run(req).map_err(network_error)?;
        let code = response.status().as_u16();
        if (200..300).contains(&code) {
            return decode(response);
        }
        let mut text = Vec::new();
        let _ = response
            .body_mut()
            .as_reader()
            .take(65536)
            .read_to_end(&mut text);
        let reason = serde_json::from_slice::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(sanitize));
        let mut e = Self::checked(response)
            .err()
            .unwrap_or_else(|| Error::protocol(""));
        if let Some(r) = reason.filter(|r| !r.is_empty()) {
            e.message = format!("{}: {r}", e.message);
        }
        Err(e)
    }
    /// A GET whose answer is bytes (a camera picture), with the printer's credentials.
    /// `Ok(None)` for 204 / 304 (nothing new yet). At most `limit` bytes.
    pub fn get_bytes(&self, path: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let send = |auth: Option<&str>| -> Result<Response<Body>> {
            let req = self
                .request("GET", path, &[], auth)
                .body(())
                .map_err(|_| Error::protocol("invalid HTTP request"))?;
            self.normal.run(req).map_err(network_error)
        };
        let response = send(None)?;
        let response = if let Some(auth) = self.auth(&response, "GET", path)? {
            drop(response);
            send(Some(&auth))?
        } else {
            response
        };
        if matches!(response.status().as_u16(), 204 | 304) {
            return Ok(None);
        }
        let mut response = Self::checked(response)?;
        read_limited(&mut response, limit).map(Some)
    }
    pub fn upload(
        &self,
        method: &str,
        endpoint: &str,
        path: &Path,
        headers: Vec<(&str, String)>,
        framing: (Vec<u8>, Vec<u8>),
        progress: &mut ProgressCallback<'_>,
    ) -> Result<Value> {
        decode(self.upload_response(method, endpoint, path, headers, framing, progress)?)
    }
    /// Stream once and retain the status/body for hosts with text acceptance warnings.
    pub fn upload_response(
        &self,
        method: &str,
        endpoint: &str,
        path: &Path,
        headers: Vec<(&str, String)>,
        framing: (Vec<u8>, Vec<u8>),
        progress: &mut ProgressCallback<'_>,
    ) -> Result<Response<Body>> {
        let mut file = File::open(path).map_err(|_| Error::protocol("cannot open upload file"))?;
        let total = file
            .metadata()
            .map_err(|_| Error::protocol("cannot stat upload file"))?
            .len();
        if !file
            .metadata()
            .map_err(|_| Error::protocol("cannot stat upload file"))?
            .is_file()
        {
            return Err(Error::protocol("upload path must be a regular file"));
        }
        // Authenticate before streaming; no replay after a connection loss or ambiguous response.
        // The challenge uses GET on a read-only resource, never a probe PUT/POST.
        let auth = if matches!(self.credentials, Credentials::Digest { .. }) {
            let probe = self
                .request("GET", "/api/printer", &[], None)
                .body(())
                .map_err(|_| Error::protocol("invalid HTTP request"))?;
            let response = self.normal.run(probe).map_err(network_error)?;
            self.auth(&response, method, endpoint)?
        } else {
            None
        };
        progress(UploadProgress { sent: 0, total });
        let tracked = ProgressReader {
            reader: (&mut file).take(total),
            sent: 0,
            total,
            callback: progress,
        };
        let length = total
            .checked_add(framing.0.len() as u64)
            .and_then(|n| n.checked_add(framing.1.len() as u64))
            .ok_or_else(|| Error::protocol("upload too large"))?;
        let mut reader = io::Cursor::new(framing.0)
            .chain(tracked)
            .chain(io::Cursor::new(framing.1));
        let req = self
            .request(method, endpoint, &headers, auth.as_deref())
            .header("Content-Length", length)
            .body(SendBody::from_reader(&mut reader))
            .map_err(|_| Error::protocol("invalid upload headers"))?;
        Self::checked(self.upload.run(req).map_err(network_error)?)
    }
}
struct ProgressReader<'a, 'b, R> {
    reader: R,
    sent: u64,
    total: u64,
    callback: &'a mut ProgressCallback<'b>,
}
impl<R: Read> Read for ProgressReader<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.read(buf)?;
        if n == 0 && self.sent != self.total {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upload file shortened",
            ));
        }
        if n > 0 {
            self.sent += n as u64;
            (self.callback)(UploadProgress {
                sent: self.sent,
                total: self.total,
            });
        }
        Ok(n)
    }
}
pub(crate) fn response_bytes(mut response: Response<Body>) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|e| network_error(e.into()))?;
    if data.len() > 2 * 1024 * 1024 {
        return Err(Error::protocol("printer response exceeds 2 MiB"));
    }
    Ok(data)
}
fn decode(response: Response<Body>) -> Result<Value> {
    let data = response_bytes(response)?;
    if data.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&data).map_err(|_| Error::protocol("invalid JSON response"))
}
fn network_error(e: ureq::Error) -> Error {
    let kind = match e {
        ureq::Error::Timeout(_)
        | ureq::Error::Io(_)
        | ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::Tls(_) => ErrorKind::Unreachable,
        _ => ErrorKind::Protocol,
    };
    Error {
        kind,
        message: match kind {
            ErrorKind::Unreachable => "printer unreachable or request timed out",
            _ => "invalid HTTP exchange",
        }
        .into(),
        http_status: None,
    }
}
/// Fetches a camera snapshot (any small image) from an absolute HTTP(S) URL with a fresh,
/// credential-free agent: no redirects, no proxy, TLS verified, at most `limit` bytes. A
/// webcam service may need its own authentication; the printer's key is never sent to it.
pub fn fetch_snapshot(url: &str, timeout: Duration, limit: usize) -> Result<Vec<u8>> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::protocol("snapshot URL must be absolute HTTP(S)"));
    }
    let agent = Agent::new_with_config(
        Agent::config_builder()
            .timeout_connect(Some(timeout))
            .timeout_global(Some(timeout))
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .build(),
    );
    let mut response = Http::checked(agent.get(url).call().map_err(network_error)?)?;
    read_limited(&mut response, limit)
}
fn read_limited(response: &mut Response<Body>, limit: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| network_error(e.into()))?;
    if data.len() > limit {
        return Err(Error::protocol("snapshot exceeds the size limit"));
    }
    if data.is_empty() {
        return Err(Error::protocol("empty snapshot"));
    }
    Ok(data)
}

/// A printer-supplied reason made safe to show: one line, printable, at most 200 chars.
pub(crate) fn sanitize(text: &str) -> String {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 200 {
        one.chars().take(199).collect::<String>() + "…"
    } else {
        one
    }
}

pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
pub(crate) fn remote_path(name: &str) -> Result<String> {
    if name.is_empty()
        || name.starts_with('/')
        || name
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        || name
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == '"')
    {
        return Err(Error::protocol("invalid remote file name"));
    }
    Ok(name.split('/').map(encode).collect::<Vec<_>>().join("/"))
}
pub(crate) fn optional<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if matches!(e.http_status, Some(404 | 405 | 501)) => Ok(None),
        Err(e) => Err(e),
    }
}
