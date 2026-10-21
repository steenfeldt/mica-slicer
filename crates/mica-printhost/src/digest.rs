// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Minimal RFC 7616 MD5/MD5-sess client for older Buddy firmware. MD5 is used only
//! for wire compatibility, never for secret storage. Unsupported algorithms fail closed.
use crate::{Error, Result};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) fn authorization(
    challenge: &str,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
) -> Result<String> {
    let (scheme, rest) = challenge
        .split_once(' ')
        .ok_or_else(|| Error::protocol("invalid digest challenge"))?;
    if !scheme.eq_ignore_ascii_case("Digest") {
        return Err(Error::protocol("printer did not offer digest auth"));
    }
    let mut fields = BTreeMap::new();
    let mut part = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for ch in rest.chars().chain(std::iter::once(',')) {
        if escaped {
            part.push(ch);
            escaped = false;
        } else if ch == '\\' && quoted {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch == ',' && !quoted {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| Error::protocol("invalid digest fields"))?;
            fields.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
            part.clear();
        } else {
            part.push(ch);
        }
    }
    if quoted || escaped {
        return Err(Error::protocol("unterminated digest field"));
    }
    let get = |key: &str| {
        fields
            .get(key)
            .ok_or_else(|| Error::protocol("incomplete digest challenge"))
    };
    let realm = get("realm")?;
    let nonce = get("nonce")?;
    let algorithm = fields.get("algorithm").map_or("MD5", String::as_str);
    if !algorithm.eq_ignore_ascii_case("MD5") && !algorithm.eq_ignore_ascii_case("MD5-sess") {
        return Err(Error::protocol(
            "unsupported digest algorithm (expected MD5 or MD5-sess)",
        ));
    }
    let qop = fields.get("qop");
    if qop.is_some_and(|q| !q.split(',').any(|v| v.trim().eq_ignore_ascii_case("auth"))) {
        return Err(Error::protocol("digest auth requires qop=auth"));
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let cnonce = md5(format!(
        "{}:{}:{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
    .as_bytes());
    build(
        username,
        password,
        method,
        uri,
        realm,
        nonce,
        algorithm,
        qop.is_some(),
        fields.get("opaque"),
        &cnonce,
    )
}
#[allow(clippy::too_many_arguments)]
fn build(
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    realm: &str,
    nonce: &str,
    algorithm: &str,
    qop: bool,
    opaque: Option<&String>,
    cnonce: &str,
) -> Result<String> {
    let quote = |s: &str| -> Result<String> {
        if s.chars().any(char::is_control) {
            return Err(Error::protocol("invalid digest value"));
        }
        Ok(format!(
            "\"{}\"",
            s.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    };
    let mut ha1 = md5(format!("{username}:{realm}:{password}").as_bytes());
    if algorithm.eq_ignore_ascii_case("MD5-sess") {
        ha1 = md5(format!("{ha1}:{nonce}:{cnonce}").as_bytes());
    }
    let ha2 = md5(format!("{method}:{uri}").as_bytes());
    let response = md5(if qop {
        format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}")
    } else {
        format!("{ha1}:{nonce}:{ha2}")
    }
    .as_bytes());
    let mut out = format!(
        "Digest username={}, realm={}, nonce={}, uri={}, response={}, algorithm={algorithm}",
        quote(username)?,
        quote(realm)?,
        quote(nonce)?,
        quote(uri)?,
        quote(&response)?
    );
    if qop {
        out.push_str(&format!(
            ", qop=auth, nc=00000001, cnonce={}",
            quote(cnonce)?
        ));
    } else if algorithm.eq_ignore_ascii_case("MD5-sess") {
        out.push_str(&format!(", cnonce={}", quote(cnonce)?));
    }
    if let Some(o) = opaque {
        out.push_str(&format!(", opaque={}", quote(o)?));
    }
    Ok(out)
}
fn md5(input: &[u8]) -> String {
    const SHIFT: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut data = input.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend(bits.to_le_bytes());
    let mut state = [0x67452301_u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in data.as_chunks::<64>().0 {
        let mut words = [0_u32; 16];
        for (word, bytes) in words.iter_mut().zip(chunk.as_chunks::<4>().0) {
            *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let next = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(words[g])
                    .rotate_left(SHIFT[(i / 16) * 4 + i % 4]),
            );
            a = d;
            d = c;
            c = b;
            b = next;
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    state
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc_vectors() {
        assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5(&vec![b'a'; 1_000_000]),
            "7707d6ae4e027c70eea2a935c2296f21"
        );
        let out = build(
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "testrealm@host.com",
            "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            "MD5",
            true,
            None,
            "0a4f113b",
        )
        .unwrap();
        assert!(out.contains("6629fae49393a05397450978507c4ef1"));
        assert!(
            authorization(
                "Digest realm=\"x\", nonce=\"n\", algorithm=SHA-256",
                "u",
                "p",
                "GET",
                "/"
            )
            .is_err()
        );
    }
}
