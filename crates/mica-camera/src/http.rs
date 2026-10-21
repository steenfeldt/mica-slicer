// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! HTTP for cameras: one GET with an optional Basic login, no redirects, no proxy, TLS
//! verified; repeated snapshots over one kept-alive connection, one request at a time.

use crate::{Auth, Ctx, decode::MAX_ENCODED};
use std::{io::Read, time::Duration};
use ureq::{Agent, Body, http::Response};

pub const CONNECT: Duration = Duration::from_secs(5);
pub const RESPONSE: Duration = Duration::from_secs(10);

/// An HTTP client for one camera. `stream`: no overall time limit (an MJPEG body never
/// ends); otherwise 10 s per request. Keeps the connection open between requests.
pub fn agent(stream: bool) -> Agent {
    let config = Agent::config_builder()
        .timeout_connect(Some(CONNECT))
        .timeout_recv_response(Some(RESPONSE))
        .timeout_global((!stream).then_some(RESPONSE))
        .max_redirects(0)
        .http_status_as_error(false)
        .proxy(None)
        .user_agent("Mica Slicer")
        .build();
    Agent::new_with_config(config)
}

/// The `Authorization` header value for a Basic login.
pub fn basic(a: &Auth) -> String {
    format!(
        "Basic {}",
        base64(format!("{}:{}", a.username, a.password).as_bytes())
    )
}

/// One GET on a fresh connection (see [`agent`] for `stream`).
pub fn get(url: &str, auth: Option<&Auth>, stream: bool) -> Result<Response<Body>, String> {
    get_with(&agent(stream), url, auth)
}

/// One GET through `agent` (reusing its open connection).
pub fn get_with(agent: &Agent, url: &str, auth: Option<&Auth>) -> Result<Response<Body>, String> {
    let mut req = agent.get(url);
    if let Some(a) = auth {
        req = req.header("Authorization", basic(a));
    }
    checked(req.call())
}

/// A response with a 2xx status, or a sentence for the user.
pub fn checked(result: Result<Response<Body>, ureq::Error>) -> Result<Response<Body>, String> {
    let response = result.map_err(|e| match e {
        ureq::Error::Timeout(_) => "the camera did not answer in time".to_string(),
        ureq::Error::HostNotFound => "the camera's host name is unknown".to_string(),
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
            "the camera is unreachable".to_string()
        }
        ureq::Error::Tls(_) => "the camera's TLS certificate was refused".to_string(),
        _ => "invalid HTTP exchange with the camera".to_string(),
    })?;
    match response.status().as_u16() {
        200..=299 => Ok(response),
        401 | 403 => Err(format!(
            "the camera refused the login (HTTP {})",
            response.status().as_u16()
        )),
        code => Err(format!("the camera answered HTTP {code}")),
    }
}

/// Repeated snapshots at the player's rate (at most [`crate::SNAPSHOT_MAX_FPS`]): the next
/// request goes out when the last picture arrived and is due, so a slow camera sets the pace
/// and never has two requests to answer. Pauses while nobody looks ([`Ctx::wait_watched`]).
pub fn snapshots(ctx: &mut Ctx<'_>, url: &str, auth: Option<&Auth>) -> Result<(), String> {
    let agent = agent(false);
    loop {
        if !ctx.next_request() {
            return Ok(());
        }
        let mut response = get_with(&agent, url, auth)?;
        let mut data = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_ENCODED as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| "the camera picture was cut off".to_string())?;
        if data.is_empty() {
            return Err("the camera sent an empty picture".into());
        }
        ctx.publish_encoded(&data)?;
    }
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
