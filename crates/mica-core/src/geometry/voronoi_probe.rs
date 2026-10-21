// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Opt-in capture for identical-input Rust/Boost sweep benchmarks. Never enable during
//! slice timing: capture serializes file writes. Files contain little-endian u32 lengths
//! followed by that many (i32 ax, ay, bx, by) records, including repair attempts.

use super::voronoi::Line;
use std::{
    fs::File,
    io::Write,
    sync::{Mutex, OnceLock},
};

pub(super) fn record(lines: &[Line]) {
    static OUTPUT: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    let output = OUTPUT.get_or_init(|| {
        std::env::var_os("MICA_VORONOI_DUMP").and_then(|path| match File::create(path) {
            Ok(file) => Some(Mutex::new(file)),
            Err(e) => {
                eprintln!("warning: cannot create Voronoi benchmark capture: {e}");
                None
            }
        })
    });
    let Some(output) = output else {
        return;
    };
    let mut bytes = Vec::with_capacity(4 + 16 * lines.len());
    let Ok(count) = u32::try_from(lines.len()) else {
        return;
    };
    bytes.extend_from_slice(&count.to_le_bytes());
    for line in lines {
        for value in [line.a.x, line.a.y, line.b.x, line.b.y] {
            let Ok(value) = i32::try_from(value) else {
                eprintln!("warning: benchmark coordinate exceeds i32");
                return;
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    if let Err(e) = output
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .write_all(&bytes)
    {
        eprintln!("warning: cannot write Voronoi benchmark capture: {e}");
    }
}
