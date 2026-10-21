// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! `cargo xtask site-shots [--only NAME,…] [--dry-run] [--check]`: regenerates the screenshots
//! of micaslicer.com from `site/screenshots.toml` in the website repository.
//!
//! The website lives in its own repository: `$MICA_WEBSITE_DIR`, by default `../mica-website`
//! next to this workspace. The manifest is read from there and the images are written there;
//! the screenshots themselves are rendered by this repository's `mica-gui`, and the manifest's
//! `model` paths are files in this repository.
//!
//! Every `[[shot]]` is one run of the GUI's offscreen screenshot tool (`mica-gui --screenshot`)
//! with a fresh temporary `MICA_CONFIG_DIR`, so Mica's own settings folder is never touched.
//! A shot can first start the loopback mock printers (`mock_printer`, for the Machine view)
//! and run Mica MCP tool calls in the window (`mcp`). The raw PNG (window size × scale) goes to
//! `target/site-shots/`; the web files go to the website's `site/img/`: an AVIF per `widths` entry and a PNG
//! fallback at `fallback_width`, plus `site/og-image.jpg` from the shot marked `og`.
//!
//! AVIF is encoded by `avifenc` when it is on the PATH, else by macOS's `sips`; without either
//! the task stops (the page's `<picture>` elements need the AVIF files). Resizing, the PNG
//! fallback and the JPEG come from the `image` crate.
//!
//! `--dry-run` prints each shot's exact command without running anything. `--check` builds
//! nothing either: it checks that every image the manifest makes exists and that `index.html`
//! references only those images (and all of them).

use image::{ImageEncoder, RgbaImage, imageops::FilterType};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, Stdio},
    time::{Duration, Instant},
};

/// Paths in the website repository.
const MANIFEST: &str = "site/screenshots.toml";
const IMG_DIR: &str = "site/img";
const OG_IMAGE: &str = "site/og-image.jpg";

/// The website repository: `$MICA_WEBSITE_DIR`, else `../mica-website` next to the workspace.
fn website_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = match std::env::var_os("MICA_WEBSITE_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => root.join("../mica-website"),
    };
    if !dir.join(MANIFEST).is_file() {
        return Err(format!(
            "the website repository is not at {} (no {MANIFEST} there); \
             set MICA_WEBSITE_DIR to your mica-website checkout",
            dir.display()
        ));
    }
    Ok(dir)
}
const OG_SIZE: (u32, u32) = (1200, 630);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    defaults: Defaults,
    shot: Vec<Shot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Defaults {
    /// Window size in points, `WxH`.
    size: String,
    /// Pixels per point.
    scale: f32,
    /// `dark` or `light`.
    theme: String,
    /// Widths of the AVIF files, in pixels.
    widths: Vec<u32>,
    /// Width of the PNG fallback.
    fallback_width: u32,
    /// AVIF quality, 0–100.
    avif_quality: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shot {
    name: String,
    /// What the image shows (docs/website.md and the page's caption).
    shows: String,
    /// The `alt` text the page uses.
    alt: String,
    /// Model files from the repository (empty: built-in models through `steps`).
    #[serde(default)]
    model: Vec<String>,
    /// Exact printer preset name (`--printer`); omitted: the default printer (CORE One).
    printer: Option<String>,
    /// Print profile (`--process`); omitted: the printer's default.
    process: Option<String>,
    /// Filament presets per tool, `T=NAME` (`--filament`); omitted: the printer's defaults.
    #[serde(default)]
    filaments: Vec<String>,
    /// Setting overrides, `key=value` (`--set`).
    #[serde(default)]
    set: Vec<String>,
    /// Mica MCP tool calls run in the window (`--mcp`), a JSON array.
    mcp: Option<String>,
    /// Scripted edits (commands, painting, text, plates, library, printers): GUI flags.
    #[serde(default)]
    steps: Vec<String>,
    /// Camera flags (`--view`, `--select`/`--focus`, `--zoom`, `--orbit`).
    #[serde(default)]
    camera: Vec<String>,
    /// Which view, tab or window shows: GUI flags.
    #[serde(default)]
    view: Vec<String>,
    /// Window size override, `WxH`.
    size: Option<String>,
    /// Arguments of the loopback mock printers started for this shot.
    #[serde(default)]
    mock_printer: Vec<String>,
    /// `127.0.0.1:PORT` the mock printers answer on once they are ready.
    mock_ready: Option<String>,
    /// Also make `site/og-image.jpg` (the link preview) from this shot.
    #[serde(default)]
    og: bool,
}

impl Shot {
    /// The screenshot tool's arguments after `--screenshot OUT`.
    fn args(&self, defaults: &Defaults, mcp_file: Option<&Path>) -> Vec<String> {
        let mut a: Vec<String> = self.model.clone();
        let size = self.size.as_deref().unwrap_or(&defaults.size);
        a.extend(["--size".into(), size.into()]);
        a.extend(["--scale".into(), defaults.scale.to_string()]);
        if defaults.theme == "light" {
            a.push("--light".into());
        }
        if let Some(p) = &self.printer {
            a.extend(["--printer".into(), p.clone()]);
        }
        if let Some(p) = &self.process {
            a.extend(["--process".into(), p.clone()]);
        }
        for f in &self.filaments {
            a.extend(["--filament".into(), f.clone()]);
        }
        for s in &self.set {
            a.extend(["--set".into(), s.clone()]);
        }
        if let Some(f) = mcp_file {
            a.extend(["--mcp".into(), f.display().to_string()]);
        }
        a.extend(self.steps.iter().cloned());
        a.extend(self.camera.iter().cloned());
        a.extend(self.view.iter().cloned());
        a
    }

    /// Every web file this shot makes, relative to `site/`.
    fn outputs(&self, d: &Defaults) -> Vec<String> {
        let mut v: Vec<String> = d
            .widths
            .iter()
            .map(|w| format!("img/{}-{w}.avif", self.name))
            .collect();
        v.push(format!("img/{}-{}.png", self.name, d.fallback_width));
        v
    }
}

pub fn run(root: &Path, args: &[String]) -> ExitCode {
    match try_run(root, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("site-shots: {e}");
            ExitCode::FAILURE
        }
    }
}

fn try_run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut only: Option<BTreeSet<String>> = None;
    let (mut dry_run, mut check) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--only" => {
                let v = it.next().ok_or("--only needs NAME[,NAME…]")?;
                only = Some(v.split(',').map(|s| s.trim().to_string()).collect());
            }
            "--dry-run" => dry_run = true,
            "--check" => check = true,
            other => {
                return Err(format!(
                    "unknown option {other} (usage: cargo xtask site-shots [--only NAME,…] [--dry-run] [--check])"
                ));
            }
        }
    }
    let web = website_dir(root)?;
    let manifest = load(root, &web)?;
    if check {
        return check_site(&web, &manifest);
    }
    let shots: Vec<&Shot> = manifest
        .shot
        .iter()
        .filter(|s| only.as_ref().is_none_or(|o| o.contains(&s.name)))
        .collect();
    if let Some(o) = &only {
        for name in o {
            if !manifest.shot.iter().any(|s| &s.name == name) {
                return Err(format!("no shot named {name} in {MANIFEST}"));
            }
        }
    }
    let profile = crate::profile();
    let bin_dir = root.join("target").join(crate::profile_dir(&profile));
    let gui = bin_dir.join(format!("mica-gui{}", std::env::consts::EXE_SUFFIX));
    let mock = bin_dir
        .join("examples")
        .join(format!("mock_printer{}", std::env::consts::EXE_SUFFIX));
    let raw_dir = root.join("target/site-shots");
    if dry_run {
        for s in &shots {
            println!("# {}: {}", s.name, s.shows);
            let mcp = s
                .mcp
                .as_ref()
                .map(|_| raw_dir.join(format!("{}.mcp.json", s.name)));
            if !s.mock_printer.is_empty() {
                println!("{} {} &", mock.display(), shell_join(&s.mock_printer));
            }
            println!(
                "MICA_CONFIG_DIR=$(mktemp -d) {} --screenshot {} {}",
                gui.display(),
                raw_dir.join(format!("{}.png", s.name)).display(),
                shell_join(&s.args(&manifest.defaults, mcp.as_deref()))
            );
        }
        return Ok(());
    }
    let encoder = avif_encoder()?;
    let build = Command::new("cargo")
        .args(["build", "--offline", "--profile", &profile])
        .args(["-p", "mica-gui", "-p", "mica-printhost"])
        .args(["--bin", "mica-gui", "--example", "mock_printer"])
        .current_dir(root)
        .status();
    if !build.is_ok_and(|s| s.success()) {
        return Err("could not build mica-gui and the mock printers".into());
    }
    fs::create_dir_all(&raw_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(web.join(IMG_DIR)).map_err(|e| e.to_string())?;
    let work = std::env::temp_dir().join(format!("mica-site-shots-{}", std::process::id()));
    let result = (|| {
        let mut table = Vec::new();
        for s in &shots {
            let raw = raw_dir.join(format!("{}.png", s.name));
            shoot(root, &gui, &mock, &work, &manifest.defaults, s, &raw)?;
            let sizes = web_files(&web, &manifest.defaults, s, &raw, &encoder, &work)?;
            table.push((s.name.clone(), sizes));
        }
        println!("\n{:<22} {:>10}  files", "shot", "bytes");
        let mut total = 0;
        for (name, sizes) in &table {
            for (file, bytes) in sizes {
                total += bytes;
                println!(
                    "{name:<22} {bytes:>10}  {}",
                    web.join("site").join(file).display()
                );
            }
        }
        println!("{:<22} {total:>10}", "total");
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

/// Reads the manifest from the website repository `web`; models are files under `root`.
fn load(root: &Path, web: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(web.join(MANIFEST)).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let m: Manifest = toml::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let mut names = BTreeSet::new();
    for s in &m.shot {
        if !names.insert(&s.name) {
            return Err(format!("{MANIFEST}: shot {} appears twice", s.name));
        }
        if s.name.is_empty()
            || !s
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(format!(
                "{MANIFEST}: shot name {:?} must be lowercase-with-dashes",
                s.name
            ));
        }
        for f in &s.model {
            if Path::new(f).is_absolute() || f.contains("..") || !root.join(f).is_file() {
                return Err(format!(
                    "{MANIFEST}: shot {}: model {f} is not a file in the repository",
                    s.name
                ));
            }
        }
        if !s.mock_printer.is_empty() && s.mock_ready.is_none() {
            return Err(format!(
                "{MANIFEST}: shot {}: mock_printer needs mock_ready",
                s.name
            ));
        }
    }
    if m.shot.iter().filter(|s| s.og).count() > 1 {
        return Err(format!("{MANIFEST}: only one shot can be the og image"));
    }
    if !matches!(m.defaults.theme.as_str(), "dark" | "light") {
        return Err(format!("{MANIFEST}: theme is dark or light"));
    }
    Ok(m)
}

/// One screenshot run, with its mock printers and a throwaway settings folder.
fn shoot(
    root: &Path,
    gui: &Path,
    mock: &Path,
    work: &Path,
    d: &Defaults,
    s: &Shot,
    raw: &Path,
) -> Result<(), String> {
    let dir = work.join(&s.name);
    let config = dir.join("config");
    fs::create_dir_all(&config).map_err(|e| e.to_string())?;
    let mcp_file = match &s.mcp {
        Some(calls) => {
            let f = dir.join("mcp.json");
            fs::write(&f, calls).map_err(|e| e.to_string())?;
            Some(f)
        }
        None => None,
    };
    let mut printers = match &s.mock_ready {
        Some(addr) => Some(MockPrinters::start(mock, &s.mock_printer, addr)?),
        None => None,
    };
    let args = s.args(d, mcp_file.as_deref());
    println!(
        "== {}: mica-gui --screenshot {} {}",
        s.name,
        raw.display(),
        shell_join(&args)
    );
    let _ = fs::remove_file(raw);
    let status = Command::new(gui)
        .arg("--screenshot")
        .arg(raw)
        .args(&args)
        .env("MICA_CONFIG_DIR", &config)
        .current_dir(root)
        .status();
    if let Some(p) = printers.as_mut() {
        p.stop();
    }
    let _ = fs::remove_dir_all(&dir);
    match status {
        Ok(st) if st.success() && raw.is_file() => Ok(()),
        Ok(st) => Err(format!("shot {} failed ({st})", s.name)),
        Err(e) => Err(format!("could not run {}: {e}", gui.display())),
    }
}

/// The loopback mock printers of one shot; stopped on drop as well.
struct MockPrinters(Option<Child>);

impl MockPrinters {
    fn start(exe: &Path, args: &[String], ready: &str) -> Result<Self, String> {
        if !ready.starts_with("127.0.0.1:") {
            return Err(format!(
                "mock_ready {ready}: the mock printers only listen on 127.0.0.1"
            ));
        }
        if TcpStream::connect(ready).is_ok() {
            return Err(format!("{ready} is already in use (another mock printer?)"));
        }
        let child = Command::new(exe)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", exe.display()))?;
        let mut me = Self(Some(child));
        let deadline = Instant::now() + Duration::from_secs(15);
        while TcpStream::connect(ready).is_err() {
            if Instant::now() > deadline {
                me.stop();
                return Err(format!("the mock printers did not answer on {ready}"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(me)
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for MockPrinters {
    fn drop(&mut self) {
        self.stop();
    }
}

enum Avif {
    Avifenc,
    Sips,
}

fn avif_encoder() -> Result<Avif, String> {
    let works = |cmd: &str, arg: &str| {
        Command::new(cmd)
            .arg(arg)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    };
    if works("avifenc", "--version") {
        Ok(Avif::Avifenc)
    } else if cfg!(target_os = "macos") && works("sips", "--help") {
        Ok(Avif::Sips)
    } else {
        Err("no AVIF encoder: install libavif (avifenc), or run on macOS (sips)".into())
    }
}

fn encode_avif(enc: &Avif, png: &Path, out: &Path, quality: u32) -> Result<(), String> {
    let q = quality.to_string();
    let mut cmd = match enc {
        Avif::Avifenc => {
            let mut c = Command::new("avifenc");
            c.args(["-q", &q, "-s", "4"]).arg(png).arg(out);
            c
        }
        Avif::Sips => {
            let mut c = Command::new("sips");
            c.args(["-s", "format", "avif", "-s", "formatOptions", &q])
                .arg(png)
                .arg("--out")
                .arg(out);
            c
        }
    };
    let ok = cmd
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok && out.is_file() {
        Ok(())
    } else {
        Err(format!("AVIF encoding of {} failed", png.display()))
    }
}

fn resized(img: &RgbaImage, width: u32) -> RgbaImage {
    if width >= img.width() {
        return img.clone();
    }
    let height = (img.height() as f64 * width as f64 / img.width() as f64).round() as u32;
    image::imageops::resize(img, width, height, FilterType::Lanczos3)
}

fn write_png(img: &RgbaImage, path: &Path) -> Result<(), String> {
    use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
    let file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Screenshots are opaque: RGB is a quarter smaller than RGBA.
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        CompressionType::Best,
        PngFilter::Adaptive,
    )
    .write_image(
        &rgb,
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// The web files of one shot, written into the website repository `web`; returns (path under
/// site/, bytes).
fn web_files(
    web: &Path,
    d: &Defaults,
    s: &Shot,
    raw: &Path,
    enc: &Avif,
    work: &Path,
) -> Result<Vec<(String, u64)>, String> {
    let img = image::open(raw)
        .map_err(|e| format!("{}: {e}", raw.display()))?
        .to_rgba8();
    let site = web.join("site");
    let tmp = work.join(format!("{}-resized.png", s.name));
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let size = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    for &w in &d.widths {
        let file = format!("img/{}-{w}.avif", s.name);
        let path = site.join(&file);
        let r = resized(&img, w);
        // A fast PNG is enough as the encoder's input.
        r.save(&tmp).map_err(|e| e.to_string())?;
        encode_avif(enc, &tmp, &path, d.avif_quality)?;
        out.push((file, size(&path)));
    }
    let _ = fs::remove_file(&tmp);
    let file = format!("img/{}-{}.png", s.name, d.fallback_width);
    let path = site.join(&file);
    write_png(&resized(&img, d.fallback_width), &path)?;
    out.push((file, size(&path)));
    if s.og {
        let (w, h) = OG_SIZE;
        let r = resized(&img, w);
        let top = r.height().saturating_sub(h) / 2;
        let crop = image::imageops::crop_imm(&r, 0, top, w, h.min(r.height())).to_image();
        let rgb = image::DynamicImage::ImageRgba8(crop).to_rgb8();
        let path = web.join(OG_IMAGE);
        let file = fs::File::create(&path).map_err(|e| e.to_string())?;
        image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 85)
            .write_image(
                &rgb,
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())?;
        out.push(("og-image.jpg".into(), size(&path)));
    }
    Ok(out)
}

/// `--check`: the manifest's files exist and the page uses exactly those images.
fn check_site(web: &Path, m: &Manifest) -> Result<(), String> {
    let site = web.join("site");
    let html = fs::read_to_string(site.join("index.html")).map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    let mut expected = BTreeSet::new();
    for s in &m.shot {
        for f in s.outputs(&m.defaults) {
            if !site.join(&f).is_file() {
                problems.push(format!(
                    "site/{f} is missing (cargo xtask site-shots --only {})",
                    s.name
                ));
            }
            if !html.contains(&f) {
                problems.push(format!("index.html does not use site/{f}"));
            }
            expected.insert(f);
        }
        if !html.contains(&format!("alt=\"{}\"", s.alt)) {
            problems.push(format!(
                "index.html does not use the alt text of {}",
                s.name
            ));
        }
    }
    if m.shot.iter().any(|s| s.og) && !site.join("og-image.jpg").is_file() {
        problems.push("site/og-image.jpg is missing".into());
    }
    // Every img/… the page names comes from the manifest.
    for (i, _) in html.match_indices("img/") {
        let rest = &html[i..];
        let end = rest.find(['"', '\'', ' ', ',', ')']).unwrap_or(rest.len());
        let f = &rest[..end];
        if !expected.contains(f) {
            problems.push(format!(
                "index.html names {f}, which no shot in {MANIFEST} makes"
            ));
        }
    }
    if let Ok(dir) = fs::read_dir(site.join("img")) {
        for e in dir.flatten() {
            let f = format!("img/{}", e.file_name().to_string_lossy());
            if !expected.contains(&f) {
                problems.push(format!(
                    "site/{f} is not in {MANIFEST} (remove it or add its shot)"
                ));
            }
        }
    }
    if problems.is_empty() {
        println!(
            "site-shots: {} shots, every image present and used",
            m.shot.len()
        );
        Ok(())
    } else {
        for p in &problems {
            eprintln!("  {p}");
        }
        Err(format!("{} problem(s)", problems.len()))
    }
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if !a.is_empty()
                && a.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,@+%".contains(&b))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_site_manifest_parses_and_names_repository_models() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        // The manifest lives in the website repository; without a checkout there is nothing to
        // check here (`cargo xtask site-shots --check` reports the missing folder instead).
        let Ok(web) = website_dir(root) else {
            eprintln!("skipped: no website repository (set MICA_WEBSITE_DIR)");
            return;
        };
        let m = load(root, &web).unwrap();
        assert!(m.shot.len() >= 10);
        assert_eq!(m.shot.iter().filter(|s| s.og).count(), 1);
        for s in &m.shot {
            // Mock printers and the MCP calls stay on this machine.
            for a in s.args(&m.defaults, None).iter().chain(&s.mock_printer) {
                assert!(
                    !a.contains("192.168.") && !a.contains("10.0."),
                    "{}: {a}",
                    s.name
                );
            }
            if let Some(calls) = &s.mcp {
                let v: serde_json::Value = serde_json::from_str(calls).unwrap();
                assert!(v.as_array().is_some_and(|a| !a.is_empty()), "{}", s.name);
            }
        }
    }

    #[test]
    fn shell_join_quotes_what_the_shell_would_split() {
        let a = [
            "--printer".to_string(),
            "Snapmaker U1 (0.4 nozzle)".into(),
            "it's".into(),
        ];
        assert_eq!(
            shell_join(&a),
            r"--printer 'Snapmaker U1 (0.4 nozzle)' 'it'\''s'"
        );
    }
}
