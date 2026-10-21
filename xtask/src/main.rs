// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Repository maintenance tasks. Run with `cargo xtask <task>`.
//!
//! `provenance` enforces the rules in PROVENANCE.md: every source file declares where its
//! logic comes from, every upstream source it names is pinned in upstream/UPSTREAM.lock,
//! and every ported or adapted file is listed in the PROVENANCE.md register.
//!
//! `gen-prusa-options` regenerates the PrusaSlicer option table (see gen_options.rs).
//! `gen-option-text` regenerates the GUI's setting labels and page layout (see gen_option_text.rs),
//! then docs/setting-names.md.
//! `gen-setting-names` regenerates docs/setting-names.md from the GUI's tables (the mica-gui test
//! `setting_names_doc_is_current` checks it).
//! `monkey` runs the GUI monkey test (see monkey.rs and docs/quality-safety.md).
//! `parity` checks that the GUI exports the CLI's G-code byte for byte (see parity.rs).
//! `gen-credits` regenerates the About window's library list and THIRD_PARTY_LICENSES.md (see credits.rs).
//! `bundle-profiles <out-dir>` writes the OrcaSlicer profiles a packaged Mica ships (see
//! bundle_profiles.rs).
//! `site-shots` regenerates the website's screenshots from site/screenshots.toml in the website
//! repository (`$MICA_WEBSITE_DIR`, default `../mica-website`; see site_shots.rs).

mod bundle_profiles;
mod corpus;
mod credits;
mod gen_option_text;
mod gen_options;
mod monkey;
mod parity;
mod reference_hashes;
mod site_shots;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const LICENSE_LINE: &str = "// SPDX-License-Identifier: AGPL-3.0-only";
/// Vendored third-party crates keep their own license: files under each directory must start with
/// that SPDX line instead, and are otherwise checked like every other ported file (sources pinned
/// in UPSTREAM.lock, `Changes:` line, register row).
const VENDORED: &[(&str, &str)] = &[
    (
        "crates/boostvoronoi/",
        "// SPDX-License-Identifier: BSL-1.0",
    ),
    ("crates/clipper2/", "// SPDX-License-Identifier: BSL-1.0"),
];
const SOURCE_DIRS: &[&str] = &["crates", "xtask"];
const SOURCE_EXTS: &[&str] = &["rs", "wgsl"];
const MIN_COMMIT_PREFIX: usize = 10;

/// The cargo profile the build-and-run tasks (corpus, monkey, parity) use: `MICA_PROFILE`, else
/// `release`. `fast` (no LTO, parallel codegen) builds in a fraction of the time and gives the
/// same G-code; `release` is what ships.
pub(crate) fn profile() -> String {
    std::env::var("MICA_PROFILE")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "release".into())
}

/// The `target/` subdirectory a profile builds into.
pub(crate) fn profile_dir(profile: &str) -> &str {
    if profile == "dev" { "debug" } else { profile }
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("provenance") => provenance(),
        Some("reference-hashes") => {
            reference_hashes::run(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        }
        Some("corpus") => corpus::run(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()),
        Some("monkey") => monkey::run(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()),
        Some("parity") => parity::run(
            Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
            std::env::args().nth(2).as_deref(),
        ),
        Some("gen-prusa-options") => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
            gen_options::run(root, std::env::args().nth(2).as_deref())
        }
        Some("gen-option-text") => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
            match gen_option_text::run(
                root,
                std::env::args().nth(2).as_deref(),
                std::env::args().nth(3).as_deref(),
            ) {
                ExitCode::SUCCESS => gen_option_text::setting_names(root),
                failed => failed,
            }
        }
        Some("bundle-profiles") => bundle_profiles::run(
            Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
            std::env::args().nth(2).as_deref(),
        ),
        Some("gen-credits") => {
            credits::run(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        }
        Some("site-shots") => site_shots::run(
            Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
            &std::env::args().skip(2).collect::<Vec<_>>(),
        ),
        Some("gen-setting-names") => {
            gen_option_text::setting_names(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        }
        _ => {
            eprintln!(
                "usage: cargo xtask provenance | reference-hashes [--output DIR] [--baseline DIR] | corpus [--quick] [--case NAME] [--printer core|xl|u1|mini|mk35] | monkey [--quick] [--seeds N] [--steps N] [--seed S] | parity [test name] | gen-prusa-options <defaults.ini> | gen-option-text [orca-dir] [prusaslicer-dir] | gen-setting-names | gen-credits | bundle-profiles <out-dir> | site-shots [--only NAME,…] [--dry-run] [--check]"
            );
            ExitCode::from(2)
        }
    }
}

#[derive(Debug, PartialEq)]
enum Kind {
    /// Written from scratch for Mica.
    Original,
    /// Upstream code translated into Mica (a derivative of that code).
    Ported,
    /// Upstream algorithm or behavior re-implemented without translating its code.
    Adapted,
}

#[derive(Debug, PartialEq)]
struct Source {
    id: String,
    commit: String,
    path: String,
}

#[derive(Debug, PartialEq)]
struct Header {
    kind: Kind,
    sources: Vec<Source>,
    has_changes: bool,
}

/// One pinned upstream from upstream/UPSTREAM.lock.
struct Pin {
    id: String,
    commit: String,
}

fn provenance() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut problems = Vec::new();

    let pins = match fs::read_to_string(root.join("upstream/UPSTREAM.lock")) {
        Ok(text) => parse_lock(&text),
        Err(e) => {
            eprintln!("cannot read upstream/UPSTREAM.lock: {e}");
            return ExitCode::FAILURE;
        }
    };
    let register = fs::read_to_string(root.join("PROVENANCE.md")).unwrap_or_default();
    if register.is_empty() {
        problems.push("PROVENANCE.md is missing or empty".to_string());
    }
    // Merge conflict markers in the provenance records would silently corrupt them.
    for name in ["PROVENANCE.md", "THIRD_PARTY.md"] {
        let text = fs::read_to_string(root.join(name)).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            if ["<<<<<<< ", ">>>>>>> "].iter().any(|m| line.starts_with(m)) || line == "=======" {
                problems.push(format!("{name}:{}: merge conflict marker", n + 1));
            }
        }
    }

    let mut files = Vec::new();
    for dir in SOURCE_DIRS {
        collect(&root.join(dir), &mut files);
    }
    files.sort();

    let mut derived = 0;
    for file in &files {
        let rel = rel_path(&root, file);
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => {
                problems.push(format!("{rel}: unreadable: {e}"));
                continue;
            }
        };
        let license_line = VENDORED
            .iter()
            .find(|(dir, _)| rel.starts_with(dir))
            .map_or(LICENSE_LINE, |(_, line)| line);
        let header = match parse_header_with(&text, license_line) {
            Ok(header) => header,
            Err(e) => {
                problems.push(format!("{rel}: {e}"));
                continue;
            }
        };
        if header.kind == Kind::Original {
            continue;
        }
        derived += 1;
        if header.sources.is_empty() {
            problems.push(format!(
                "{rel}: {:?} file has no `// Source:` line",
                header.kind
            ));
        }
        if !header.has_changes {
            problems.push(format!(
                "{rel}: {:?} file has no `// Changes:` line",
                header.kind
            ));
        }
        for src in &header.sources {
            match pins.iter().find(|p| p.id == src.id) {
                None => problems.push(format!(
                    "{rel}: source id `{}` is not pinned in upstream/UPSTREAM.lock",
                    src.id
                )),
                Some(pin) if !pin.commit.starts_with(&src.commit) => problems.push(format!(
                    "{rel}: source `{}@{}` does not match the pinned commit {}",
                    src.id, src.commit, pin.commit
                )),
                Some(_) => {}
            }
            // Upstream checkouts are optional (CI fetches only sparse profile data,
            // scripts/fetch-upstream.sh --profiles); check paths in full checkouts.
            let checkout = root.join("upstream").join(&src.id);
            let sparse = checkout.join(".git/info/sparse-checkout").is_file();
            if checkout.is_dir() && !sparse && !checkout.join(&src.path).exists() {
                problems.push(format!(
                    "{rel}: source path `{}` does not exist in upstream/{}",
                    src.path, src.id
                ));
            }
        }
        if !register.contains(&format!("`{rel}`")) {
            problems.push(format!("{rel}: not listed in the PROVENANCE.md register"));
        }
    }

    for listed in register_paths(&register) {
        if !root.join(&listed).is_file() {
            problems.push(format!(
                "PROVENANCE.md lists `{listed}`, which does not exist"
            ));
        }
    }

    if problems.is_empty() {
        println!(
            "provenance ok: {} files checked, {derived} ported/adapted",
            files.len()
        );
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            eprintln!("error: {p}");
        }
        eprintln!(
            "provenance check failed with {} problem(s); see PROVENANCE.md",
            problems.len()
        );
        ExitCode::FAILURE
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCE_EXTS.contains(&e))
        {
            out.push(path);
        }
    }
}

fn rel_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn parse_lock(text: &str) -> Vec<Pin> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            (cols.len() >= 4).then(|| Pin {
                id: cols[0].to_string(),
                commit: cols[3].to_string(),
            })
        })
        .collect()
}

/// Parses the leading `//` comment block of a source file.
#[cfg(test)]
fn parse_header(text: &str) -> Result<Header, String> {
    parse_header_with(text, LICENSE_LINE)
}

/// [`parse_header`] requiring `license_line` first (vendored crates keep their own license).
fn parse_header_with(text: &str, license_line: &str) -> Result<Header, String> {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|l| l.starts_with("//") && !l.starts_with("//!") && !l.starts_with("///"))
        .collect();
    if lines.first() != Some(&license_line) {
        return Err(format!("first line must be `{license_line}`"));
    }
    let kind = match lines.iter().find_map(|l| l.strip_prefix("// Provenance:")) {
        Some(k) => match k.trim() {
            "original" => Kind::Original,
            "ported" => Kind::Ported,
            "adapted" => Kind::Adapted,
            other => {
                return Err(format!(
                    "unknown provenance `{other}` (use original, ported or adapted)"
                ));
            }
        },
        None => return Err("missing `// Provenance:` line".to_string()),
    };
    let mut sources = Vec::new();
    for line in &lines {
        if let Some(rest) = line.strip_prefix("// Source:") {
            sources.push(parse_source(rest.trim())?);
        }
    }
    if kind == Kind::Original && !sources.is_empty() {
        return Err(
            "`original` file has `// Source:` lines; mark it ported or adapted".to_string(),
        );
    }
    let has_changes = lines.iter().any(|l| l.starts_with("// Changes:"));
    Ok(Header {
        kind,
        sources,
        has_changes,
    })
}

/// Parses `<id>@<commit> <path> [:: <symbol>]`.
fn parse_source(s: &str) -> Result<Source, String> {
    let bad =
        || format!("malformed `// Source: {s}` (expected `<id>@<commit> <path> :: <symbol>`)");
    let (pin, rest) = s.split_once(' ').ok_or_else(bad)?;
    let (id, commit) = pin.split_once('@').ok_or_else(bad)?;
    let path = rest.split("::").next().unwrap_or("").trim();
    if id.is_empty() || path.is_empty() {
        return Err(bad());
    }
    if commit.len() < MIN_COMMIT_PREFIX || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "source commit `{commit}` must be at least {MIN_COMMIT_PREFIX} hex characters"
        ));
    }
    Ok(Source {
        id: id.to_string(),
        commit: commit.to_string(),
        path: path.to_string(),
    })
}

/// Repository paths listed in the first column of the PROVENANCE.md register table.
fn register_paths(register: &str) -> Vec<String> {
    let Some(start) = register.find("## Register") else {
        return Vec::new();
    };
    register[start..]
        .lines()
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter_map(|l| {
            let cell = l.trim().strip_prefix('|')?.split('|').next()?.trim();
            let path = cell.strip_prefix('`')?.strip_suffix('`')?;
            SOURCE_DIRS
                .iter()
                .any(|d| path.starts_with(&format!("{d}/")))
                .then(|| path.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_header() {
        let h = parse_header(&format!(
            "{LICENSE_LINE}\n// Provenance: original\n\nfn main() {{}}"
        ))
        .unwrap();
        assert_eq!(h.kind, Kind::Original);
        assert!(h.sources.is_empty());
    }

    #[test]
    fn ported_header_with_sources() {
        let text = format!(
            "{LICENSE_LINE}\n// Provenance: ported\n\
             // Source: orca@8500fcdcca src/libslic3r/PerimeterGenerator.cpp :: process_classic\n\
             // Source: prusaslicer@b028299c77 src/libslic3r/Layer.cpp\n\
             // Changes: Rust port.\n//! docs"
        );
        let h = parse_header(&text).unwrap();
        assert_eq!(h.kind, Kind::Ported);
        assert!(h.has_changes);
        assert_eq!(h.sources.len(), 2);
        assert_eq!(h.sources[0].id, "orca");
        assert_eq!(h.sources[0].path, "src/libslic3r/PerimeterGenerator.cpp");
        assert_eq!(h.sources[1].commit, "b028299c77");
    }

    #[test]
    fn rejects_missing_license_and_bad_sources() {
        assert!(parse_header("// Provenance: original\n").is_err());
        assert!(parse_header(&format!("{LICENSE_LINE}\n// Provenance: borrowed\n")).is_err());
        let short = format!("{LICENSE_LINE}\n// Provenance: ported\n// Source: orca@85 a.cpp\n");
        assert!(parse_header(&short).is_err());
        let orig =
            format!("{LICENSE_LINE}\n// Provenance: original\n// Source: orca@8500fcdcca a.cpp\n");
        assert!(parse_header(&orig).is_err());
    }

    #[test]
    fn register_paths_reads_first_column_only() {
        let md = "# P\n## Register\n| File | Source |\n|---|---|\n\
                  | `crates/mica-core/src/a.rs` | `crates/not/this.rs` |\n## Next\n| `crates/x.rs` |";
        assert_eq!(
            register_paths(md),
            vec!["crates/mica-core/src/a.rs".to_string()]
        );
    }
}
