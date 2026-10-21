// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Scan performance on a synthetic library in a temporary folder:
//!
//! ```sh
//! cargo run --release --offline -p mica-library --example scan_bench -- [files] [threads] [downloads]
//! ```
//!
//! `downloads` adds that many realistic downloads (parts, a 3MF, G-code, pictures; about 20
//! files each) next to the `files` generated models, and times the project detection and
//! the grouped views.

use mica_library::testkit::{TempDir, synthetic_downloads, synthetic_library};
use mica_library::{Filter, GroupBy, Library, Options, Sort, group, query};
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(2000);
    let threads: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(2);
    let downloads: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(0);
    let lib = TempDir::new("bench");
    let data = TempDir::new("bench-data");
    let t = Instant::now();
    let mut stats = synthetic_library(lib.path(), n).expect("synthetic library");
    for i in 0..downloads {
        let d = synthetic_downloads(
            &lib.path()
                .join(format!("Set {:03}", i / 25))
                .join(format!("{i:04}")),
            0,
        )
        .expect("downloads");
        stats.files += d.files;
        stats.bytes += d.bytes;
    }
    println!(
        "synthetic tree: {} files, {:.1} MB, {} duplicate copies ({:.2} s to write)",
        stats.files,
        stats.bytes as f64 / 1e6,
        stats.duplicates,
        t.elapsed().as_secs_f64()
    );
    let options = Options {
        threads,
        watch: false,
        ..Default::default()
    };
    let l = Library::open(data.path(), options.clone()).expect("open");
    let t = Instant::now();
    l.add_folder(lib.path()).expect("add");
    assert!(l.wait_idle(Duration::from_secs(600)));
    let first = t.elapsed().as_secs_f64();
    let s = l.snapshot();
    let thumbs = s.models.iter().filter(|m| m.thumbnail.is_some()).count();
    println!(
        "first scan ({threads} threads): {first:.2} s = {:.0} files/s; {} files → {} models, {thumbs} thumbnails",
        s.files as f64 / first,
        s.files,
        s.models.len()
    );
    let t = Instant::now();
    l.rescan(None);
    assert!(l.wait_idle(Duration::from_secs(600)));
    println!(
        "rescan, nothing changed: {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let hay = query::haystacks(&s.models);
    let t = Instant::now();
    let f = Filter {
        text: "bracket 00".into(),
        ..Default::default()
    };
    let hits = query::apply(&s.models, &hay, &f, Sort::Modified, true, 0).len();
    println!(
        "search \"bracket 00\" + sort: {hits} hits in {:.2} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let projects = group::projects(&s);
    println!(
        "project detection: {} projects from {} files and {} pictures/documents in {:.1} ms",
        projects.list.len(),
        s.files,
        s.extras.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    for by in GroupBy::ALL {
        for text in ["", "dragon"] {
            let f = Filter {
                text: text.into(),
                ..Default::default()
            };
            let t = Instant::now();
            let v = group::view(&s.models, &hay, &projects, &f, by, Sort::Name, false, 0);
            println!(
                "view {:<8} search {text:?}: {} sections, {} projects, {} models in {:.1} ms",
                by.label(),
                v.sections.len(),
                v.projects,
                v.models,
                t.elapsed().as_secs_f64() * 1e3
            );
        }
    }
    drop(l);
    let t = Instant::now();
    let l = Library::open(data.path(), options).expect("reopen");
    assert!(l.wait_for(Duration::from_secs(60), |s| s.files == stats.files));
    println!(
        "reopen (index from disk, before the rescan): {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    assert!(l.wait_idle(Duration::from_secs(600)));
    println!(
        "reopen + incremental rescan: {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let size: u64 = std::fs::read_dir(data.path().join("thumbs"))
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    let index = std::fs::metadata(data.path().join("index.redb")).map_or(0, |m| m.len());
    println!(
        "data folder: index {:.1} MB, thumbnails {:.1} MB",
        index as f64 / 1e6,
        size as f64 / 1e6
    );
}
