// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! MJPEG over HTTP: a `multipart/x-mixed-replace` body whose parts are JPEG pictures
//! (mjpg-streamer, ustreamer, camera-streamer, most IP cameras). Parts with a
//! `Content-Length` are read exactly; parts without one end at the next boundary. The
//! boundary comes from the `Content-Type` header, or from the first boundary line when the
//! header has none (seen in the wild). Every part is read, only parts that are due are
//! decoded.

use crate::{Auth, Ctx, decode::MAX_ENCODED, http};
use std::io::{self, Read};

/// Reads the parts of a multipart body one by one, in bounded memory.
pub struct Multipart<R> {
    reader: R,
    buf: Vec<u8>,
    /// Without its leading dashes.
    boundary: Option<Vec<u8>>,
    limit: usize,
}
impl<R: Read> Multipart<R> {
    /// `content_type` is the response's `Content-Type` header, if any.
    pub fn new(reader: R, content_type: Option<&str>, limit: usize) -> Self {
        Self {
            reader,
            buf: Vec::with_capacity(64 * 1024),
            boundary: content_type.and_then(boundary),
            limit,
        }
    }
    fn fill(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 32 * 1024];
        let n = loop {
            match self.reader.read(&mut chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        };
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n > 0)
    }
    fn too_big() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "camera frame exceeds the size limit",
        )
    }
    /// The next part's body; `None` at the end of the stream.
    pub fn next_part(&mut self) -> io::Result<Option<Vec<u8>>> {
        // 1. The boundary line (learnt from the first `--` line when the header had none).
        loop {
            if self.boundary.is_none()
                && let Some(end) = find(&self.buf, b"\n")
            {
                let line = trim(&self.buf[..end]);
                if let Some(b) = line.strip_prefix(b"--") {
                    let b = b.strip_suffix(b"--").unwrap_or(b);
                    if !b.is_empty() {
                        self.boundary = Some(b.to_vec());
                    }
                }
                if self.boundary.is_none() {
                    self.buf.drain(..=end);
                    continue;
                }
            }
            if let Some(b) = &self.boundary
                && let Some(p) = find(&self.buf, b)
            {
                self.buf.drain(..p + b.len());
                break;
            }
            // Keep a tail in case the boundary straddles two reads.
            let keep = self.boundary.as_ref().map_or(0, |b| b.len());
            if self.buf.len() > keep + 4096 {
                let cut = self.buf.len() - keep;
                self.buf.drain(..cut);
            }
            if !self.fill()? {
                return Ok(None);
            }
        }
        // 2. The rest of the boundary line and the part's headers.
        let (headers, skip) = loop {
            if let Some(p) = find(&self.buf, b"\r\n\r\n") {
                break (self.buf[..p].to_vec(), p + 4);
            }
            if let Some(p) = find(&self.buf, b"\n\n") {
                break (self.buf[..p].to_vec(), p + 2);
            }
            if self.buf.len() > 16 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "camera part headers are too long",
                ));
            }
            if !self.fill()? {
                return Ok(None);
            }
        };
        // `--boundary--` closes the stream.
        if headers.starts_with(b"--") {
            return Ok(None);
        }
        self.buf.drain(..skip);
        let length = String::from_utf8_lossy(&headers)
            .lines()
            .skip(1)
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            });
        // 3. The body.
        match length {
            Some(n) => {
                if n > self.limit {
                    return Err(Self::too_big());
                }
                while self.buf.len() < n {
                    if !self.fill()? {
                        return Ok(None);
                    }
                }
                Ok(Some(self.buf.drain(..n).collect()))
            }
            None => {
                let b = self.boundary.clone().unwrap_or_default();
                let mut from = 0;
                loop {
                    if let Some(p) = find(&self.buf[from..], &b) {
                        let p = from + p;
                        let mut part = self.buf[..p].to_vec();
                        // The boundary stays for step 1; drop the `\r\n--` before it.
                        while part.last() == Some(&b'-') {
                            part.pop();
                        }
                        while matches!(part.last(), Some(b'\r' | b'\n')) {
                            part.pop();
                        }
                        self.buf.drain(..p);
                        return Ok(Some(part));
                    }
                    if self.buf.len() > self.limit {
                        return Err(Self::too_big());
                    }
                    from = self.buf.len().saturating_sub(b.len());
                    if !self.fill()? {
                        return Ok(None);
                    }
                }
            }
        }
    }
}

fn trim(line: &[u8]) -> &[u8] {
    let mut l = line;
    while let [rest @ .., b'\r' | b'\n' | b' ' | b'\t'] = l {
        l = rest;
    }
    while let [b' ' | b'\t', rest @ ..] = l {
        l = rest;
    }
    l
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The `boundary` parameter of a `multipart/…` content type, without quotes or leading
/// dashes.
fn boundary(content_type: &str) -> Option<Vec<u8>> {
    content_type.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        if !k.trim().eq_ignore_ascii_case("boundary") {
            return None;
        }
        let v = v.trim().trim_matches('"').trim_start_matches('-');
        (!v.is_empty()).then(|| v.as_bytes().to_vec())
    })
}

pub fn run(ctx: &mut Ctx<'_>, url: &str, auth: Option<&Auth>) -> Result<(), String> {
    let mut response = http::get(url, auth, true)?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(ct) = &content_type {
        let ct = ct.to_ascii_lowercase();
        if ct.starts_with("image/") {
            return Err(
                "this URL is a single picture, not a stream; Mica treats it as snapshots \
                 when its name contains \"snapshot\" or ends in .jpg"
                    .into(),
            );
        }
        if !ct.starts_with("multipart/") {
            return Err(format!(
                "the camera sent {}, not an MJPEG stream",
                ct.split(';').next().unwrap_or("")
            ));
        }
    }
    let reader = response.body_mut().as_reader();
    let mut parts = Multipart::new(reader, content_type.as_deref(), MAX_ENCODED);
    loop {
        if ctx.stopped() {
            return Ok(());
        }
        let part = parts
            .next_part()
            .map_err(|_| "the camera stream broke off".to_string())?;
        let Some(part) = part else {
            return Err("the camera stream ended".into());
        };
        if ctx.due() && !part.is_empty() {
            ctx.publish_encoded(&part)?;
        }
    }
}
