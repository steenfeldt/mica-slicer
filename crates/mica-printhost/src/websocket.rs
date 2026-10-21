// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! A minimal, blocking WebSocket client (RFC 6455) for one Moonraker JSON-RPC call over
//! `ws://<host>/websocket`. It exists because some Moonraker endpoints (the Snapmaker U1's
//! `server.files.start_local_print`) are registered for every transport except HTTP, so
//! `POST /server/jsonrpc` answers "Method not found for transport HTTP". Plain `ws://` only:
//! the printers are on the LAN and Mica's `https://` addresses fall back to other routes.

use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Why a call did not produce an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WsError {
    /// Nothing was sent: no WebSocket here (refused, not upgraded, `https`). Safe to use
    /// another route.
    Unavailable(String),
    /// The request went out but no clear answer came back; the printer may have acted on it.
    Ambiguous(String),
}

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_MESSAGE: usize = 2 * 1024 * 1024;

/// One JSON-RPC request over a fresh WebSocket; returns the whole reply object (with
/// `result` or `error`) whose `id` matches. Notifications in between are skipped.
pub(crate) fn call(
    base: &str,
    api_key: Option<&str>,
    method: &str,
    params: Value,
    connect: Duration,
    reply: Duration,
) -> Result<Value, WsError> {
    let (authority, prefix) = split_base(base)?;
    let address = authority
        .to_socket_addrs()
        .map_err(|_| WsError::Unavailable("cannot resolve the printer address".into()))?
        .next()
        .ok_or_else(|| WsError::Unavailable("cannot resolve the printer address".into()))?;
    let mut stream = TcpStream::connect_timeout(&address, connect)
        .map_err(|_| WsError::Unavailable("cannot open the printer's WebSocket".into()))?;
    let io = |_| WsError::Unavailable("the WebSocket handshake failed".into());
    stream.set_read_timeout(Some(connect)).map_err(io)?;
    stream.set_write_timeout(Some(connect)).map_err(io)?;
    let key = base64(&nonce16());
    let mut request = format!(
        "GET {prefix}/websocket HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
    );
    if let Some(k) = api_key {
        request.push_str(&format!("X-Api-Key: {k}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).map_err(io)?;
    let head = read_head(&mut stream)?;
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    if status.split(' ').nth(1) != Some("101") {
        let code = status.split(' ').nth(1).unwrap_or("?");
        return Err(WsError::Unavailable(format!(
            "the printer did not open a WebSocket (HTTP {code})"
        )));
    }
    let accept = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept"))
        .map(|(_, v)| v.trim().to_owned());
    if accept.as_deref() != Some(accept_key(&key).as_str()) {
        return Err(WsError::Unavailable(
            "the printer's WebSocket handshake did not verify".into(),
        ));
    }
    static ID: AtomicU64 = AtomicU64::new(1);
    let id = ID.fetch_add(1, Ordering::Relaxed) + 4000;
    let body = json!({"jsonrpc":"2.0","method":method,"params":params,"id":id}).to_string();
    let ambiguous = |_| WsError::Ambiguous("the printer's WebSocket closed mid-request".into());
    write_frame(&mut stream, 0x1, body.as_bytes()).map_err(ambiguous)?;
    let deadline = Instant::now() + reply;
    loop {
        let message = read_message(&mut stream, deadline)?;
        let Ok(v) = serde_json::from_slice::<Value>(&message) else {
            continue;
        };
        if v["id"].as_u64() == Some(id) {
            let _ = write_frame(&mut stream, 0x8, &1000u16.to_be_bytes());
            return Ok(v);
        }
    }
}

/// `http://host:port/prefix` → (`host:port`, `/prefix`). Only plain HTTP maps to `ws://`.
fn split_base(base: &str) -> Result<(String, String), WsError> {
    let Some(rest) = base.strip_prefix("http://") else {
        return Err(WsError::Unavailable(
            "Mica opens WebSockets only to plain http:// addresses".into(),
        ));
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = if authority.contains(':') && !authority.ends_with(']') {
        authority.to_owned()
    } else {
        format!("{authority}:80")
    };
    let prefix = path.trim_end_matches('/');
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("/{prefix}")
    };
    Ok((authority, prefix))
}

fn read_head(stream: &mut TcpStream) -> Result<String, WsError> {
    let mut data = Vec::new();
    let mut byte = [0];
    while !data.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => data.push(byte[0]),
            _ => {
                return Err(WsError::Unavailable(
                    "the WebSocket handshake got no answer".into(),
                ));
            }
        }
        if data.len() > 16384 {
            return Err(WsError::Unavailable(
                "the WebSocket handshake answer is too long".into(),
            ));
        }
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// Client frames are always masked (RFC 6455 §5.3).
fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut frame = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => frame.push(0x80 | n as u8),
        n if n <= 0xFFFF => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let mask: [u8; 4] = nonce16()[..4].try_into().unwrap_or([0x5a; 4]);
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    stream.write_all(&frame)
}

/// The next complete text or binary message; answers pings, fails on close or timeout.
fn read_message(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, WsError> {
    let mut message = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(WsError::Ambiguous(
                "the printer did not answer in time".into(),
            ));
        }
        let lost = |_| WsError::Ambiguous("the printer did not answer in time".into());
        stream.set_read_timeout(Some(remaining)).map_err(lost)?;
        let mut head = [0u8; 2];
        stream.read_exact(&mut head).map_err(lost)?;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0F;
        let masked = head[1] & 0x80 != 0;
        let mut len = (head[1] & 0x7F) as u64;
        if len == 126 {
            let mut b = [0u8; 2];
            stream.read_exact(&mut b).map_err(lost)?;
            len = u16::from_be_bytes(b) as u64;
        } else if len == 127 {
            let mut b = [0u8; 8];
            stream.read_exact(&mut b).map_err(lost)?;
            len = u64::from_be_bytes(b);
        }
        if len as usize > MAX_MESSAGE || message.len() + len as usize > MAX_MESSAGE {
            return Err(WsError::Ambiguous(
                "the printer's WebSocket message is too large".into(),
            ));
        }
        let mut mask = [0u8; 4];
        if masked {
            stream.read_exact(&mut mask).map_err(lost)?;
        }
        let mut payload = vec![0u8; len as usize];
        stream.read_exact(&mut payload).map_err(lost)?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        match opcode {
            0x9 => {
                let _ = write_frame(stream, 0xA, &payload);
            }
            0xA => {}
            0x8 => {
                return Err(WsError::Ambiguous(
                    "the printer closed the WebSocket before answering".into(),
                ));
            }
            0x0..=0x2 => {
                message.extend_from_slice(&payload);
                if fin {
                    return Ok(message);
                }
            }
            _ => {}
        }
    }
}

/// `Sec-WebSocket-Accept` for a key. Public for the mock printers (`examples/`).
#[doc(hidden)]
pub fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

fn nonce16() -> [u8; 16] {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{t}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let h = sha1(seed.as_bytes());
    h[..16].try_into().unwrap_or([0x42; 16])
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// SHA-1 (FIPS 180-4), only for the handshake check and nonces.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Server side of one frame, for the tests and mock printers: reads a masked client frame.
#[doc(hidden)]
pub fn read_client_text(stream: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head)?;
    let opcode = head[0] & 0x0F;
    let mut len = (head[1] & 0x7F) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        stream.read_exact(&mut b)?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        stream.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    let mut mask = [0u8; 4];
    if head[1] & 0x80 != 0 {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len.min(MAX_MESSAGE as u64) as usize];
    stream.read_exact(&mut payload)?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Ok((opcode == 0x1).then_some(payload))
}

/// Server side: an unmasked text frame.
#[doc(hidden)]
pub fn server_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x81];
    match payload.len() {
        n if n < 126 => frame.push(n as u8),
        n if n <= 0xFFFF => {
            frame.push(126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_base64_and_accept_key_match_rfc_6455() {
        // RFC 6455 §1.3's example handshake.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"a"), "YQ==");
    }

    #[test]
    fn bases_map_to_websocket_addresses() {
        assert_eq!(
            split_base("http://192.0.2.50:7125").unwrap(),
            ("192.0.2.50:7125".into(), String::new())
        );
        assert_eq!(
            split_base("http://u1.local/moonraker/").unwrap(),
            ("u1.local:80".into(), "/moonraker".into())
        );
        assert!(matches!(
            split_base("https://u1.local"),
            Err(WsError::Unavailable(_))
        ));
    }
}
