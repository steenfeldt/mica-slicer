// SPDX-License-Identifier: BSL-1.0
// Provenance: original
//! Opt-in capture of clipper inputs for offline benchmarking (feature `capture`).
//!
//! With the feature enabled and `MICA_CLIPPER_DUMP=<file>` set, every boolean execution and every
//! offset execution appends one record to the file. Writes are serialised by a mutex; never enable
//! this during timing runs. Little-endian format, repeated records:
//!
//! - `u8 kind` (0 boolean, 1 offset), `u8 nested` (1 when issued inside an offset's own union)
//! - boolean: `u8 clip_type, u8 fill_rule, u8 tree, u8 preserve_collinear, u8 reverse`, then the
//!   subject, open-subject and clip path sets
//! - offset: `f64 delta, f64 miter_limit, f64 arc_tolerance, u8 preserve_collinear, u8 reverse,
//!   u8 tree, u32 groups`, each group `u8 join, u8 end` and a path set
//!
//! A path set is `u32 paths`, each `u32 points` followed by `(i64 x, i64 y, i64 z)` per point
//! (z is 0 without `using_z`).

use crate::core::{FillRule, Path64, Paths64};
use crate::engine::ClipType;
use crate::offset::{EndType, JoinType};
use std::cell::Cell;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

thread_local! {
    static NESTED: Cell<u32> = const { Cell::new(0) };
}

fn sink() -> Option<&'static Mutex<std::fs::File>> {
    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::env::var_os("MICA_CLIPPER_DUMP")?;
        std::fs::File::create(path).ok().map(Mutex::new)
    })
    .as_ref()
}

/// Runs `f` with the nested flag raised, so booleans issued by an offset are marked.
pub(crate) fn nested<R>(f: impl FnOnce() -> R) -> R {
    NESTED.with(|n| n.set(n.get() + 1));
    let r = f();
    NESTED.with(|n| n.set(n.get() - 1));
    r
}

fn put_paths(buf: &mut Vec<u8>, paths: &Paths64) {
    buf.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    for p in paths {
        put_path(buf, p);
    }
}

fn put_path(buf: &mut Vec<u8>, path: &Path64) {
    buf.extend_from_slice(&(path.len() as u32).to_le_bytes());
    for pt in path {
        buf.extend_from_slice(&pt.x.to_le_bytes());
        buf.extend_from_slice(&pt.y.to_le_bytes());
        #[cfg(feature = "using_z")]
        buf.extend_from_slice(&pt.z.to_le_bytes());
        #[cfg(not(feature = "using_z"))]
        buf.extend_from_slice(&0i64.to_le_bytes());
    }
}

fn emit(buf: Vec<u8>) {
    if let Some(sink) = sink() {
        report_repeat(&buf);
        let mut file = sink.lock().unwrap_or_else(|e| e.into_inner());
        let _ = file.write_all(&buf);
    }
}

/// With `MICA_CLIPPER_REPEATS=<file>`, every 64th exact repeat of an earlier record (ignoring the
/// nested flag) appends the caller's backtrace to that file, to find redundant clipping.
fn report_repeat(buf: &[u8]) {
    struct Repeats {
        seen: std::collections::HashSet<u64>,
        count: u64,
        file: std::fs::File,
    }
    static REPEATS: OnceLock<Option<Mutex<Repeats>>> = OnceLock::new();
    let Some(repeats) = REPEATS
        .get_or_init(|| {
            let path = std::env::var_os("MICA_CLIPPER_REPEATS")?;
            let file = std::fs::File::create(path).ok()?;
            Some(Mutex::new(Repeats {
                seen: Default::default(),
                count: 0,
                file,
            }))
        })
        .as_ref()
    else {
        return;
    };
    let mut digest = 0xcbf2_9ce4_8422_2325u64 ^ u64::from(buf[0]);
    for &b in &buf[2..] {
        digest = (digest ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    let mut r = repeats.lock().unwrap_or_else(|e| e.into_inner());
    if r.seen.insert(digest) {
        return;
    }
    r.count += 1;
    if r.count % 64 == 1 {
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = writeln!(
            r.file,
            "=== repeat kind {} bytes {}\n{bt}",
            buf[0],
            buf.len()
        );
    }
}

fn nested_flag() -> u8 {
    NESTED.with(|n| u8::from(n.get() > 0))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn boolean(
    ct: ClipType,
    fr: FillRule,
    tree: bool,
    preserve_collinear: bool,
    reverse: bool,
    subject: &Paths64,
    open: &Paths64,
    clip: &Paths64,
) {
    if sink().is_none() {
        return;
    }
    let mut buf = vec![0, nested_flag()];
    buf.extend_from_slice(&[
        ct as u8,
        fr as u8,
        u8::from(tree),
        u8::from(preserve_collinear),
        u8::from(reverse),
    ]);
    put_paths(&mut buf, subject);
    put_paths(&mut buf, open);
    put_paths(&mut buf, clip);
    emit(buf);
}

pub(crate) fn offset(
    delta: f64,
    miter_limit: f64,
    arc_tolerance: f64,
    preserve_collinear: bool,
    reverse: bool,
    tree: bool,
    groups: &[(JoinType, EndType, Paths64)],
) {
    if sink().is_none() {
        return;
    }
    let mut buf = vec![1, nested_flag()];
    buf.extend_from_slice(&delta.to_le_bytes());
    buf.extend_from_slice(&miter_limit.to_le_bytes());
    buf.extend_from_slice(&arc_tolerance.to_le_bytes());
    buf.extend_from_slice(&[
        u8::from(preserve_collinear),
        u8::from(reverse),
        u8::from(tree),
    ]);
    buf.extend_from_slice(&(groups.len() as u32).to_le_bytes());
    for (jt, et, paths) in groups {
        buf.extend_from_slice(&[*jt as u8, *et as u8]);
        put_paths(&mut buf, paths);
    }
    emit(buf);
}
