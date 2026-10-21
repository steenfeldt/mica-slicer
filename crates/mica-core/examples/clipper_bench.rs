// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Replays a `MICA_CLIPPER_DUMP` capture (format in `crates/clipper2/src/capture.rs`) through the
//! vendored `clipper2-rust`, timing only the clipper calls; `scripts/bench-clipper2.cpp` replays the
//! same file through C++ Clipper2. Nested records (an offset's own finishing union) are skipped,
//! since the offset record times them. `--hashes FILE` writes one result hash per record, so two
//! builds of the crate can be checked for bit-identical results record by record.
//! `CLIPPER_BENCH_PARSE_ONLY=1` reads the capture without clipping (process-cost baseline).
use clipper2_rust::{
    ClipType, Clipper64, ClipperOffset, EndType, FillRule, JoinType, Path64, Paths64, Point64,
    PolyTree64,
};
use std::io::{BufReader, Read, Write};
use std::time::Instant;

struct Reader<R: Read> {
    inner: R,
    ok: bool,
    /// FNV-1a over the bytes of the current record, for the duplicate report.
    digest: u64,
}

impl<R: Read> Reader<R> {
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0; N];
        if self.inner.read_exact(&mut b).is_err() {
            self.ok = false;
        }
        self.mix(&b);
        b
    }
    fn mix(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.digest = (self.digest ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn u8(&mut self) -> u8 {
        self.bytes::<1>()[0]
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }
    fn f64(&mut self) -> f64 {
        f64::from_le_bytes(self.bytes())
    }
    fn paths(&mut self) -> Paths64 {
        let n = self.u32();
        let mut out = Paths64::with_capacity(n as usize);
        let mut raw = Vec::new();
        for _ in 0..n {
            if !self.ok {
                break;
            }
            let m = self.u32() as usize;
            raw.resize(m * 24, 0);
            if self.inner.read_exact(&mut raw).is_err() {
                self.ok = false;
            }
            self.mix(&raw);
            let path: Path64 = raw
                .as_chunks::<24>()
                .0
                .iter()
                .map(|c| {
                    let v = |i: usize| i64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().unwrap());
                    Point64::new_z(v(0), v(1), v(2))
                })
                .collect();
            out.push(path);
        }
        out
    }
}

#[derive(Default)]
struct Sum {
    paths: u64,
    points: u64,
    hash: u64,
    hash_z: u64,
}

const FNV: u64 = 1_099_511_628_211;

impl Sum {
    fn new() -> Self {
        Self {
            hash: 1_469_598_103_934_665_603,
            hash_z: 1_469_598_103_934_665_603,
            ..Self::default()
        }
    }
    fn add(&mut self, paths: &Paths64) {
        for p in paths {
            self.paths += 1;
            for pt in p {
                self.points += 1;
                self.hash = (self.hash ^ pt.x as u64).wrapping_mul(FNV);
                self.hash = (self.hash ^ pt.y as u64).wrapping_mul(FNV);
                self.hash_z = (self.hash_z ^ pt.z as u64).wrapping_mul(FNV);
            }
        }
    }
}

fn tree_paths(tree: &PolyTree64, idx: usize, out: &mut Paths64) {
    for &child in tree.nodes[idx].children() {
        out.push(tree.nodes[child].polygon().clone());
        tree_paths(tree, child, out);
    }
}

fn clip_type(v: u8) -> ClipType {
    match v {
        1 => ClipType::Intersection,
        2 => ClipType::Union,
        3 => ClipType::Difference,
        4 => ClipType::Xor,
        _ => ClipType::NoClip,
    }
}

fn fill_rule(v: u8) -> FillRule {
    match v {
        0 => FillRule::EvenOdd,
        1 => FillRule::NonZero,
        2 => FillRule::Positive,
        _ => FillRule::Negative,
    }
}

fn join_type(v: u8) -> JoinType {
    match v {
        0 => JoinType::Square,
        1 => JoinType::Bevel,
        2 => JoinType::Round,
        _ => JoinType::Miter,
    }
}

fn end_type(v: u8) -> EndType {
    match v {
        0 => EndType::Polygon,
        1 => EndType::Joined,
        2 => EndType::Butt,
        3 => EndType::Square,
        _ => EndType::Round,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: clipper_bench CAPTURE [--hashes FILE]");
    let mut hashes = match (args.next().as_deref(), args.next()) {
        (Some("--hashes"), Some(file)) => Some(std::io::BufWriter::new(
            std::fs::File::create(file).expect("hash file"),
        )),
        _ => None,
    };
    // Subtract a CLIPPER_BENCH_PARSE_ONLY=1 run's process cycles to compare clipping alone.
    let parse_only = std::env::var_os("CLIPPER_BENCH_PARSE_ONLY").is_some();
    let file = std::fs::File::open(path).expect("capture file");
    let mut r = Reader {
        inner: BufReader::with_capacity(1 << 20, file),
        ok: true,
        digest: 0,
    };
    let names = [
        "boolean intersection",
        "boolean union",
        "boolean difference",
        "offset square",
        "offset bevel",
        "offset round",
        "offset miter",
    ];
    let mut secs = [0.0f64; 7];
    let mut counts = [0u64; 7];
    let mut sum = Sum::new();
    // Exact repeats of an earlier record (same inputs and parameters): calls and seconds.
    let mut seen = std::collections::HashSet::new();
    let mut dup_secs = [0.0f64; 7];
    let mut dup_counts = [0u64; 7];
    loop {
        let kind = r.u8();
        if !r.ok {
            break;
        }
        r.digest = 0xcbf2_9ce4_8422_2325;
        r.mix(&[kind]);
        let nested = r.u8() != 0;
        let mut result = Paths64::new();
        let slot;
        let elapsed;
        if kind == 0 {
            let (ct, fr, tree, pc, rev) = (r.u8(), r.u8(), r.u8() != 0, r.u8() != 0, r.u8() != 0);
            let (subj, open, clip) = (r.paths(), r.paths(), r.paths());
            if !r.ok {
                break;
            }
            if nested || parse_only {
                continue;
            }
            slot = match ct {
                1 => 0,
                2 => 1,
                _ => 2,
            };
            let t0 = Instant::now();
            let mut c = Clipper64::new();
            c.set_preserve_collinear(pc);
            c.set_reverse_solution(rev);
            if !subj.is_empty() {
                c.add_subject(&subj);
            }
            if !open.is_empty() {
                c.add_open_subject(&open);
            }
            if !clip.is_empty() {
                c.add_clip(&clip);
            }
            let mut open_out = Paths64::new();
            if tree {
                let mut pt = PolyTree64::new();
                c.execute_tree(clip_type(ct), fill_rule(fr), &mut pt, &mut open_out);
                elapsed = t0.elapsed();
                tree_paths(&pt, 0, &mut result);
            } else {
                c.execute(
                    clip_type(ct),
                    fill_rule(fr),
                    &mut result,
                    Some(&mut open_out),
                );
                elapsed = t0.elapsed();
            }
            result.extend(open_out);
        } else {
            let (delta, miter, arc) = (r.f64(), r.f64(), r.f64());
            let (pc, rev, tree) = (r.u8() != 0, r.u8() != 0, r.u8() != 0);
            let groups = r.u32();
            let mut gs = Vec::new();
            for _ in 0..groups {
                let (jt, et) = (r.u8(), r.u8());
                gs.push((jt, et, r.paths()));
            }
            if !r.ok {
                break;
            }
            if nested || parse_only {
                continue;
            }
            slot = 3 + gs.first().map_or(0, |g| g.0 as usize).min(3);
            let t0 = Instant::now();
            let mut co = ClipperOffset::new(miter, arc, pc, rev);
            for (jt, et, paths) in &gs {
                co.add_paths(paths, join_type(*jt), end_type(*et));
            }
            if tree {
                let mut pt = PolyTree64::new();
                co.execute_tree(delta, &mut pt);
                elapsed = t0.elapsed();
                tree_paths(&pt, 0, &mut result);
            } else {
                co.execute(delta, &mut result);
                elapsed = t0.elapsed();
            }
        }
        secs[slot] += elapsed.as_secs_f64();
        if !seen.insert(r.digest) {
            dup_secs[slot] += elapsed.as_secs_f64();
            dup_counts[slot] += 1;
        }
        counts[slot] += 1;
        let mut one = Sum::new();
        one.add(&result);
        sum.add(&result);
        if let Some(h) = hashes.as_mut() {
            writeln!(h, "{:016x} {:016x}", one.hash, one.hash_z).unwrap();
        }
    }
    let mut total = 0.0;
    for i in 0..7 {
        if counts[i] > 0 {
            println!("{:<22} {:>9} calls {:>9.3} s", names[i], counts[i], secs[i]);
            total += secs[i];
        }
    }
    println!("{:<22} {:>15} {:>9.3} s", "total", "", total);
    for i in 0..7 {
        if dup_counts[i] > 0 {
            println!(
                "{:<22} {:>9} exact repeats {:>9.3} s",
                names[i], dup_counts[i], dup_secs[i]
            );
        }
    }
    println!(
        "output: {} paths, {} points, xy hash {:016x}, z hash {:016x}",
        sum.paths, sum.points, sum.hash, sum.hash_z
    );
}
