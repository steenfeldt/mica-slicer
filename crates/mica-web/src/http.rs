// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The smallest HTTP/1.1 server the dashboard needs: one thread per connection (a camera
//! stream holds its thread for as long as it is watched), one request per connection
//! (`Connection: close`), the request head parsed by `httparse`, the body read on demand.
//! Few clients ever connect (a phone and a laptop), so there is no async runtime.

use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

/// The request head may be at most this large.
const MAX_HEAD: usize = 16 * 1024;
/// Connections served at once (each camera viewer holds one).
const MAX_CONNECTIONS: usize = 64;
/// How long a silent client is waited for.
const IDLE: Duration = Duration::from_secs(60);

pub struct Request {
    pub method: String,
    /// The path without the query, percent-decoded.
    pub path: String,
    pub query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    /// Bytes of the body already read with the head.
    early: Vec<u8>,
    length: u64,
    pub stream: TcpStream,
}

impl Request {
    /// A request header (case-insensitive name).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
    /// The declared body length (`Content-Length`; chunked bodies are refused earlier).
    pub fn body_len(&self) -> u64 {
        self.length
    }
    /// The body as a reader of exactly `body_len` bytes.
    pub fn body(&mut self) -> impl Read + '_ {
        let early = std::mem::take(&mut self.early);
        let rest = self.length.saturating_sub(early.len() as u64);
        io::Cursor::new(early).chain((&self.stream).take(rest))
    }
    /// The whole body, refused above `limit` bytes.
    pub fn body_bytes(&mut self, limit: u64) -> Result<Vec<u8>, Response> {
        if self.length > limit {
            return Err(Response::error(413, "The request is too large."));
        }
        let mut out = Vec::with_capacity(self.length as usize);
        self.body()
            .read_to_end(&mut out)
            .map_err(|_| Response::error(400, "The request body could not be read."))?;
        if (out.len() as u64) < self.length {
            return Err(Response::error(400, "The request body was cut short."));
        }
        Ok(out)
    }
}

/// A complete response (streams write to `Request::stream` themselves and return `None`).
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(&'static str, String)>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
            headers: vec![],
        }
    }
    pub fn json(value: &serde_json::Value) -> Self {
        Self::new(200, "application/json", value.to_string())
    }
    /// `{"error": message}` with `status`.
    pub fn error(status: u16, message: &str) -> Self {
        Self::new(
            status,
            "application/json",
            serde_json::json!({ "error": message }).to_string(),
        )
    }
    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// Writes `r` (and closes the connection afterwards).
pub fn write(stream: &mut impl Write, r: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    if !r.headers.iter().any(|(n, _)| *n == "Cache-Control") {
        head.push_str("Cache-Control: no-store\r\n");
    }
    for (n, v) in &r.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

/// `%41` → `A`, `+` → space (in queries). Invalid escapes are kept as they are.
pub fn decode(s: &str, plus_is_space: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k, true), decode(v, true))
        })
        .collect()
}

/// Reads and parses one request head. `Err` carries the response to send instead.
fn read_request(stream: TcpStream) -> Result<Request, (TcpStream, Response)> {
    let mut reader = &stream;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err((
                stream,
                Response::error(431, "The request head is too large."),
            ));
        }
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => {
                return Err((stream, Response::error(400, "Incomplete request.")));
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let mut headers = [httparse::EMPTY_HEADER; 48];
    let mut req = httparse::Request::new(&mut headers);
    let parsed = req.parse(&buf[..head_end]);
    if !matches!(parsed, Ok(httparse::Status::Complete(_))) {
        return Err((stream, Response::error(400, "Malformed request.")));
    }
    let method = req.method.unwrap_or("").to_string();
    let target = req.path.unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|h| {
            (
                h.name.to_string(),
                String::from_utf8_lossy(h.value).trim().to_string(),
            )
        })
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    if find("Transfer-Encoding").is_some() {
        return Err((
            stream,
            Response::error(411, "Send the body with a Content-Length."),
        ));
    }
    let length = match find("Content-Length") {
        Some(v) => match v.parse::<u64>() {
            Ok(n) => n,
            Err(_) => return Err((stream, Response::error(400, "Bad Content-Length."))),
        },
        None => 0,
    };
    Ok(Request {
        method,
        path: decode(path, false),
        query: parse_query(query),
        headers,
        early: buf[head_end..].to_vec(),
        length,
        stream,
    })
}

/// The listening server; dropping it stops accepting (connections already open finish).
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// Listens on `addr` and serves every request with `handler` on its own thread.
    /// `handler` returns `None` when it wrote the response itself (a stream).
    pub fn start(
        addr: SocketAddr,
        handler: impl Fn(&mut Request) -> Option<Response> + Send + Sync + 'static,
    ) -> io::Result<Server> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let open = Arc::new(AtomicUsize::new(0));
        let thread = {
            let stop = stop.clone();
            thread::Builder::new()
                .name("mica-web-accept".into())
                .spawn(move || {
                    for conn in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(mut stream) = conn else { continue };
                        if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                            let _ = write(
                                &mut stream,
                                &Response::error(503, "Too many connections; try again."),
                            );
                            continue;
                        }
                        open.fetch_add(1, Ordering::SeqCst);
                        let handler = handler.clone();
                        let open2 = open.clone();
                        let spawned =
                            thread::Builder::new()
                                .name("mica-web-conn".into())
                                .spawn(move || {
                                    serve(stream, handler.as_ref());
                                    open2.fetch_sub(1, Ordering::SeqCst);
                                });
                        if spawned.is_err() {
                            open.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                })?
        };
        Ok(Server {
            addr,
            stop,
            thread: Some(thread),
        })
    }
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    /// Blocks until the server stops (never, for the binary).
    pub fn join(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(stream: TcpStream, handler: &(dyn Fn(&mut Request) -> Option<Response> + Send + Sync)) {
    let _ = stream.set_read_timeout(Some(IDLE));
    let _ = stream.set_write_timeout(Some(IDLE));
    let _ = stream.set_nodelay(true);
    match read_request(stream) {
        Ok(mut req) => {
            if let Some(r) = handler(&mut req) {
                let _ = write(&mut req.stream, &r);
            }
            let _ = req.stream.shutdown(std::net::Shutdown::Both);
        }
        Err((mut stream, r)) => {
            let _ = write(&mut stream, &r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_paths_and_queries() {
        assert_eq!(decode("a%20b+c", true), "a b c");
        assert_eq!(decode("a%20b+c", false), "a b+c");
        assert_eq!(decode("100%", false), "100%");
        assert_eq!(decode("%zz%4", false), "%zz%4");
        assert_eq!(decode("%C3%A9", false), "é");
        assert_eq!(
            parse_query("q=benchy+boat&gcode=1&&x"),
            vec![
                ("q".into(), "benchy boat".into()),
                ("gcode".into(), "1".into()),
                ("x".into(), String::new())
            ]
        );
    }
}
