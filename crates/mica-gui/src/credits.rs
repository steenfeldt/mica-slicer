// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Who Mica owes: the data behind the About window's Credits tab and Open-source licenses
//! view, and the repository's `THIRD_PARTY_LICENSES.md` (the crate list as a file for GitHub,
//! written by `third_party_licenses_md`, which the test `third_party_licenses_are_current`
//! checks and `cargo xtask gen-credits` runs). The website only has a short Thanks section.
//!
//! Credits name the projects Mica is built on or inspired by, with a short list of its key
//! libraries; the full generated crate list is only on the licenses view and in that file.
//!
//! The rule: credit the originals, also when Mica only adopted an idea.
//! The hand-written entries follow PROVENANCE.md (every upstream of `upstream/UPSTREAM.lock`
//! is credited, see the tests), THIRD_PARTY.md and its "Ideas adopted without their code"
//! table; the textures come from the texture catalog; the Rust crates are generated from
//! Cargo's dependency data by `cargo xtask gen-credits` (`assets/credits/crates.tsv`).
//!
//! Hidden features stay hidden: an entry that belongs to one (Brick layers) is only listed
//! while the feature is turned on.

use mica_core::texture::catalog::{BUILTINS, Credit as TextureCredit};

/// The groups of the Credits tab, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Engines,
    Ideas,
    Textures,
    Models,
    Printers,
    Libraries,
}

impl Group {
    pub const ALL: [Self; 6] = [
        Self::Engines,
        Self::Ideas,
        Self::Textures,
        Self::Models,
        Self::Printers,
        Self::Libraries,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Engines => "Slicing engines",
            Self::Ideas => "Ideas and features",
            Self::Textures => "Textures",
            Self::Models => "Models",
            Self::Printers => "Camera and printers",
            Self::Libraries => "Open-source libraries",
        }
    }

    pub fn intro(self) -> &'static str {
        match self {
            Self::Engines => {
                "Mica's slicing rules are ported from these slicers, and their authors' \
                 notices are kept in every ported file."
            }
            Self::Ideas => {
                "Features that started elsewhere, whether Mica ported the code or only \
                 adopted the idea."
            }
            Self::Textures => "The height maps that come with Mica and the people who made them.",
            Self::Models => "The ready-made models in Add shape that Mica did not make itself.",
            Self::Printers => "Printer profiles, cameras and the printer software Mica talks to.",
            Self::Libraries => "The key libraries, fonts and formats inside Mica.",
        }
    }
}

/// One credited project, person or data source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub group: Group,
    pub name: String,
    pub by: String,
    /// SPDX expression (`AGPL-3.0`, `MIT OR Apache-2.0`); empty when none applies or none is
    /// stated.
    pub license: String,
    pub link: String,
    /// What Mica took from it.
    pub what: String,
    /// The `upstream/UPSTREAM.lock` id, for the pinned version line.
    pub upstream: Option<&'static str>,
}

impl Entry {
    /// `version_2.9.6 · b028299c77` for a pinned upstream.
    pub fn pin(&self) -> Option<String> {
        let id = self.upstream?;
        upstream_pins()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| format!("{} · {}", p.reference, &p.commit[..p.commit.len().min(10)]))
    }

    /// Whether `query` (lowercase) matches any of its text.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [&self.name, &self.by, &self.license, &self.what, &self.link]
                .iter()
                .any(|s| s.to_lowercase().contains(query))
    }
}

/// One Rust crate of the generated list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crate {
    pub name: &'static str,
    pub version: &'static str,
    pub license: &'static str,
    pub repository: &'static str,
    /// The copyright lines of its license files (may be empty: then "the <name> authors").
    pub copyright: &'static str,
}

impl Crate {
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [
                self.name,
                self.version,
                self.license,
                self.repository,
                self.copyright,
            ]
            .iter()
            .any(|s| s.to_lowercase().contains(query))
    }

    pub fn holders(&self) -> String {
        if self.copyright.is_empty() {
            format!("The {} authors", self.name)
        } else {
            self.copyright.to_string()
        }
    }
}

const CRATES_TSV: &str = include_str!("../assets/credits/crates.tsv");
const UPSTREAM_LOCK: &str = include_str!("../../../upstream/UPSTREAM.lock");
const AGPL: &str = include_str!("../../../LICENSE");

/// Every crate the shipped binaries link, on any platform (generated; see the module docs).
pub fn crates() -> Vec<Crate> {
    CRATES_TSV
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut c = l.split('\t');
            Some(Crate {
                name: c.next()?,
                version: c.next()?,
                license: c.next()?,
                repository: c.next().unwrap_or_default(),
                copyright: c.next().unwrap_or_default(),
            })
        })
        .collect()
}

/// One row of `upstream/UPSTREAM.lock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub id: &'static str,
    pub repo: &'static str,
    pub reference: &'static str,
    pub commit: &'static str,
}

pub fn upstream_pins() -> Vec<Pin> {
    UPSTREAM_LOCK
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut c = l.split_whitespace();
            Some(Pin {
                id: c.next()?,
                repo: c.next()?,
                reference: c.next()?,
                commit: c.next()?,
            })
        })
        .collect()
}

/// The full text of a license Mica can show, by SPDX id.
pub fn license_text(id: &str) -> Option<&'static str> {
    Some(match id {
        "AGPL-3.0" | "AGPL-3.0-only" | "AGPL-3.0-or-later" => AGPL,
        "0BSD" => include_str!("../assets/credits/licenses/0BSD.txt"),
        "Apache-2.0" => include_str!("../assets/credits/licenses/Apache-2.0.txt"),
        "BSD-1-Clause" => include_str!("../assets/credits/licenses/BSD-1-Clause.txt"),
        "BSD-2-Clause" => include_str!("../assets/credits/licenses/BSD-2-Clause.txt"),
        "BSD-3-Clause" => include_str!("../assets/credits/licenses/BSD-3-Clause.txt"),
        "BSL-1.0" => include_str!("../assets/credits/licenses/BSL-1.0.txt"),
        "CC0-1.0" => include_str!("../assets/credits/licenses/CC0-1.0.txt"),
        "CDLA-Permissive-2.0" => include_str!("../assets/credits/licenses/CDLA-Permissive-2.0.txt"),
        "ISC" => include_str!("../assets/credits/licenses/ISC.txt"),
        "MIT" => include_str!("../assets/credits/licenses/MIT.txt"),
        "OFL-1.1" => include_str!("../assets/credits/licenses/OFL-1.1.txt"),
        "Ubuntu-font-1.0" => include_str!("../assets/credits/licenses/Ubuntu-font-1.0.txt"),
        "Unicode-3.0" => include_str!("../assets/credits/licenses/Unicode-3.0.txt"),
        "Unlicense" => include_str!("../assets/credits/licenses/Unlicense.txt"),
        "Zlib" => include_str!("../assets/credits/licenses/Zlib.txt"),
        _ => return None,
    })
}

/// Where a license's text lives on the web.
pub fn license_url(id: &str) -> String {
    let base = id.split(" WITH ").next().unwrap_or(id);
    match base {
        "AGPL-3.0" | "AGPL-3.0-only" | "AGPL-3.0-or-later" => {
            "https://www.gnu.org/licenses/agpl-3.0.html".into()
        }
        "GPL-3.0" | "GPL-3.0-only" | "GPL-3.0-or-later" => {
            "https://www.gnu.org/licenses/gpl-3.0.html".into()
        }
        "CC-BY-4.0" => "https://creativecommons.org/licenses/by/4.0/".into(),
        "CC0-1.0" => "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        _ => format!("https://spdx.org/licenses/{base}.html"),
    }
}

/// The SPDX ids of an expression, in order (`MIT OR Apache-2.0` → `MIT`, `Apache-2.0`;
/// `MIT/Apache-2.0` as well; a `WITH` exception stays with its license).
pub fn license_ids(expr: &str) -> Vec<String> {
    let flat = expr.replace(['(', ')'], " ").replace('/', " OR ");
    let mut out: Vec<String> = vec![];
    let mut words = flat.split_whitespace().peekable();
    while let Some(w) = words.next() {
        if w == "OR" || w == "AND" {
            continue;
        }
        let id = if words.peek() == Some(&"WITH") {
            words.next();
            format!("{w} WITH {}", words.next().unwrap_or_default())
        } else {
            w.to_string()
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn entry(
    group: Group,
    name: &str,
    by: &str,
    license: &str,
    link: &str,
    what: &str,
    upstream: Option<&'static str>,
) -> Entry {
    Entry {
        group,
        name: name.into(),
        by: by.into(),
        license: license.into(),
        link: link.into(),
        what: what.into(),
        upstream,
    }
}

/// A library entry whose license comes from the generated crate list.
fn library(name: &str, krate: &str, by: &str, link: &str, what: &str) -> Entry {
    let license = crates()
        .into_iter()
        .find(|c| c.name == krate)
        .map(|c| c.license.to_string())
        .unwrap_or_default();
    entry(Group::Libraries, name, by, &license, link, what, None)
}

/// Everything the Credits tab lists, in group order. `brick_layers`: the hidden Brick layers
/// feature is on (its credits are listed only then).
pub fn entries(brick_layers: bool) -> Vec<Entry> {
    use Group::*;
    let mut out = vec![
        // Slicing engines.
        entry(
            Engines,
            "PrusaSlicer",
            "Prusa Research and contributors",
            "AGPL-3.0",
            "https://github.com/prusa3d/PrusaSlicer",
            "Mica's main source. Its structure and most of its slicing rules are ported from \
             PrusaSlicer: walls, Arachne, infill, supports, seams, cooling, the G-code writer and \
             3MF projects. CORE One and XL output matches PrusaSlicer.",
            Some("prusaslicer"),
        ),
        entry(
            Engines,
            "OrcaSlicer",
            "SoftFever and contributors",
            "AGPL-3.0",
            "https://github.com/OrcaSlicer/OrcaSlicer",
            "Features on top of PrusaSlicer's structure, Klipper output, setting names and the \
             slicing rules for the Snapmaker U1, whose output matches OrcaSlicer.",
            Some("orca"),
        ),
        entry(
            Engines,
            "Snapmaker Orca",
            "Snapmaker",
            "AGPL-3.0",
            "https://github.com/Snapmaker/OrcaSlicer",
            "Snapmaker's OrcaSlicer: the U1's G-code details and the color matching that maps \
             filaments to the U1's toolheads.",
            Some("snapmaker-orca"),
        ),
        entry(
            Engines,
            "Bambu Studio",
            "Bambu Lab and contributors",
            "AGPL-3.0",
            "https://github.com/bambulab/BambuStudio",
            "OrcaSlicer grew from Bambu Studio, so much of the OrcaSlicer code Mica ports \
             began there.",
            None,
        ),
        entry(
            Engines,
            "Slic3r",
            "Alessandro Ranellucci and contributors",
            "AGPL-3.0",
            "https://github.com/slic3r/Slic3r",
            "Where PrusaSlicer, Bambu Studio and OrcaSlicer began. The Slic3r authors' notices \
             are kept in Mica's ported files.",
            None,
        ),
        entry(
            Engines,
            "CuraEngine",
            "UltiMaker and contributors",
            "AGPL-3.0",
            "https://github.com/Ultimaker/CuraEngine",
            "Arachne variable-width walls, Lightning infill and the tree supports (Thomas \
             Rahm's organic supports) came to PrusaSlicer from CuraEngine.",
            None,
        ),
        entry(
            Engines,
            "libbgcode",
            "Prusa Research",
            "AGPL-3.0",
            "https://github.com/prusa3d/libbgcode",
            "Prusa's binary G-code format (.bgcode), ported to Rust.",
            Some("libbgcode"),
        ),
        // Ideas and features.
        entry(
            Ideas,
            "BumpMesh",
            "Stefan Hermann (CNC Kitchen) and contributors",
            "AGPL-3.0",
            "https://github.com/CNCKitchen/stlTexturizer",
            "Surface textures: Mica's projections, texture sampling, boundary falloff and \
             crease handling follow BumpMesh, and its height maps come from BumpMesh's \
             collection.",
            Some("bumpmesh"),
        ),
    ];
    if brick_layers {
        out.push(entry(
            Ideas,
            "Brick Layers",
            "GeekDetour (Everson Siqueira)",
            "GPL-3.0-or-later",
            "https://github.com/GeekDetour/BrickLayers",
            "The idea behind Brick layers: every other inner wall half a layer higher, so the \
             walls interlock like bricks. Re-implemented in Mica's engine; no code was taken.",
            Some("bricklayers"),
        ));
        out.push(entry(
            Ideas,
            "Brick layers strength tests",
            "Stefan Hermann (CNC Kitchen)",
            "",
            "https://www.cnckitchen.com",
            "The strength tests that showed how much brick layers help.",
            None,
        ));
    }
    out.extend([
        entry(
            Ideas,
            "Prusa ColorMix",
            "Prusa Research",
            "AGPL-3.0",
            "https://github.com/prusa3d/PrusaSlicer",
            "Mixed colors from two or three filaments, layer by layer: PrusaSlicer's virtual \
             extruders (Full Spectrum) and their project format.",
            None,
        ),
        entry(
            Ideas,
            "prusa-fdm-mixer",
            "Ondrej Bartas (Prusa Research) and contributors",
            "MIT",
            "https://github.com/prusa3d/prusa-fdm-mixer",
            "The color model that predicts how a mix will look; Mica's mixed-color swatches use \
             a Rust port of it.",
            None,
        ),
        entry(
            Ideas,
            "Snapmaker mixed filaments",
            "Snapmaker",
            "AGPL-3.0",
            "https://github.com/Snapmaker/OrcaSlicer",
            "The ideas behind the Outer wall first and Gradient patterns of mixed colors \
             (per-wall pattern groups and gradients).",
            None,
        ),
        entry(
            Ideas,
            "Small Area Flow Compensation",
            "Alexander Þór",
            "GPL-3.0",
            "https://github.com/Alexander-T-Moss/Small-Area-Flow-Comp",
            "Where OrcaSlicer's small area flow compensation and its default curve, which Mica \
             ports, come from.",
            None,
        ),
        entry(
            Ideas,
            "ArcWelder",
            "Brad Hochgesang (FormerLurker)",
            "AGPL-3.0",
            "https://github.com/FormerLurker/ArcWelderLib",
            "Arc fitting (G2/G3 moves), ported through PrusaSlicer.",
            None,
        ),
        entry(
            Ideas,
            "Infill patterns",
            "Kevin Ryde, David Eccles (gringer) and Bambu Lab",
            "",
            "https://metacpan.org/pod/Math::PlanePath::HilbertCurve",
            "The Hilbert curve table (Kevin Ryde's Math::PlanePath), the honeycomb geometry \
             (David Eccles) and CrossHatch infill (Bambu Lab and David Eccles), ported through \
             PrusaSlicer and OrcaSlicer.",
            None,
        ),
        entry(
            Ideas,
            "libc++ sort order",
            "The LLVM Project",
            "Apache-2.0 WITH LLVM-exception",
            "https://github.com/llvm/llvm-project",
            "Mica sorts equal elements in the same order as libc++'s std::sort, so its output \
             matches PrusaSlicer's builds.",
            None,
        ),
    ]);
    out.extend(textures());
    out.push(entry(
        Models,
        "3DBenchy",
        "Creative Tools (NTI Group); designed by Daniel Norée and Paulo Kiefe",
        "CC0-1.0",
        "https://www.3dbenchy.com",
        "#3DBenchy, the test boat in Add shape: the official single-part STL, unmodified. \
         Public domain since 2025 (CC0 1.0).",
        None,
    ));
    out.extend([
        // Camera and printers.
        entry(
            Printers,
            "camera-streamer",
            "Kamil Trzciński (ayufan)",
            "GPL-3.0",
            "https://github.com/ayufan/camera-streamer",
            "Mica plays camera-streamer's WebRTC video (the U1's case camera) by speaking its \
             signaling protocol; no code was taken.",
            None,
        ),
        entry(
            Printers,
            "Moonraker",
            "Eric Callahan (Arksine) and contributors",
            "GPL-3.0",
            "https://github.com/Arksine/moonraker",
            "The Klipper API Mica sends prints to and reads printer status from, written from \
             OrcaSlicer's Moonraker client.",
            None,
        ),
        entry(
            Printers,
            "Klipper",
            "Kevin O'Connor and contributors",
            "GPL-3.0",
            "https://www.klipper3d.org",
            "The firmware whose G-code and macros Mica writes for the Snapmaker U1.",
            None,
        ),
        entry(
            Printers,
            "PrusaLink and Prusa Connect",
            "Prusa Research",
            "",
            "https://github.com/prusa3d/Prusa-Link",
            "Sending prints to Prusa printers, written from PrusaSlicer's PrusaLink client.",
            None,
        ),
        entry(
            Printers,
            "Prusa printer profiles",
            "Prusa Research",
            "",
            "https://github.com/prusa3d/PrusaSlicer-settings-prusa-fff",
            "Printer, print and filament profiles for Prusa printers, downloaded from Prusa's \
             profile server once (not shipped with Mica), with their bed models and printer \
             pictures; they are trademarks of Prusa Research.",
            Some("prusa-profiles"),
        ),
        entry(
            Printers,
            "OrcaSlicer vendor profiles",
            "OrcaSlicer contributors and the printer vendors",
            "AGPL-3.0",
            "https://github.com/OrcaSlicer/OrcaSlicer/tree/main/resources/profiles",
            "Printer, print and filament profiles, bed models and pictures for Snapmaker, \
             RatRig, Bambu Lab, Creality, Voron, Elegoo, Anycubic, QIDI and Sovol (plus OrcaSlicer's own \
             Prusa profiles), shipped with Mica from OrcaSlicer 2.4.2, with OrcaSlicer's shared filament library. \
             Names and logos are trademarks of their owners.",
            Some("orca"),
        ),
        entry(
            Printers,
            "Snapmaker U1 profiles",
            "Snapmaker and OrcaSlicer contributors",
            "AGPL-3.0",
            "https://github.com/OrcaSlicer/OrcaSlicer/tree/main/resources/profiles/Snapmaker",
            "The U1's printer, print and filament profiles, bed model and picture, read from \
             OrcaSlicer's resources. The logos are trademarks of Snapmaker.",
            None,
        ),
        entry(
            Printers,
            "GStreamer",
            "The GStreamer team",
            "LGPL-2.1-or-later",
            "https://gstreamer.freedesktop.org",
            "Decodes camera video on Linux. On macOS and Windows the system's decoder \
             (VideoToolbox, Media Foundation) does.",
            None,
        ),
        // Open-source libraries.
        library(
            "egui and eframe",
            "egui",
            "Emil Ernerfeldt and contributors",
            "https://github.com/emilk/egui",
            "The whole user interface.",
        ),
        library(
            "wgpu",
            "wgpu",
            "The gfx-rs developers",
            "https://github.com/gfx-rs/wgpu",
            "The 3D view and the G-code preview on Metal, Vulkan and DirectX 12.",
        ),
        library(
            "winit",
            "winit",
            "The winit contributors",
            "https://github.com/rust-windowing/winit",
            "Windows and input on every platform.",
        ),
        entry(
            Libraries,
            "Clipper2",
            "Angus Johnson",
            "BSL-1.0",
            "https://github.com/AngusJohnson/Clipper2",
            "Polygon clipping and offsetting, the base of every layer Mica slices.",
            Some("clipper2"),
        ),
        entry(
            Libraries,
            "clipper2-rust",
            "Lars Brubaker",
            "BSL-1.0",
            "https://github.com/larsbrubaker/clipper2-rust",
            "The Rust port of Clipper2 that Mica ships as a tuned fork.",
            Some("clipper2-rust"),
        ),
        entry(
            Libraries,
            "boostvoronoi",
            "eadf, after Boost.Polygon by Andrii Sydorchuk",
            "BSL-1.0",
            "https://codeberg.org/eadf/boostvoronoi_rs",
            "Voronoi diagrams for the medial axis, Arachne walls and painting; Mica ships a \
             tuned fork.",
            Some("boostvoronoi"),
        ),
        library(
            "str0m",
            "str0m",
            "Martin Algesten and contributors",
            "https://github.com/algesten/str0m",
            "WebRTC for printer cameras.",
        ),
        library(
            "Retina",
            "retina",
            "Scott Lamb and contributors",
            "https://github.com/scottlamb/retina",
            "RTSP printer cameras.",
        ),
        library(
            "redb",
            "redb",
            "Christopher Berner",
            "https://github.com/cberner/redb",
            "The model library's index.",
        ),
        library(
            "resvg",
            "resvg",
            "The resvg authors (Linebender)",
            "https://github.com/linebender/resvg",
            "SVG icons, bed textures and SVG embossing.",
        ),
        library(
            "Skrifa and HarfRust",
            "skrifa",
            "The Fontations and HarfRust authors",
            "https://github.com/googlefonts/fontations",
            "Font outlines and text shaping for the Text tool.",
        ),
        library(
            "Rayon",
            "rayon",
            "Josh Stone, Niko Matsakis and contributors",
            "https://github.com/rayon-rs/rayon",
            "Slicing layers in parallel.",
        ),
        library(
            "Serde",
            "serde",
            "Erick Tryzelaar, David Tolnay and contributors",
            "https://github.com/serde-rs/serde",
            "Settings, profiles and projects.",
        ),
        library(
            "Tokio",
            "tokio",
            "The Tokio contributors",
            "https://github.com/tokio-rs/tokio",
            "Networking for cameras and Mica MCP.",
        ),
        library(
            "rmcp",
            "rmcp",
            "The Model Context Protocol Rust SDK contributors",
            "https://github.com/modelcontextprotocol/rust-sdk",
            "Mica MCP.",
        ),
        library(
            "rustls and ring",
            "rustls",
            "The rustls and ring authors",
            "https://github.com/rustls/rustls",
            "Secure connections to printers and the update server.",
        ),
        entry(
            Libraries,
            "IBM Plex",
            "IBM",
            "OFL-1.1",
            "https://github.com/IBM/plex",
            "Mica's typeface, and the Text tool's default font.",
            None,
        ),
        entry(
            Libraries,
            "Ubuntu and Hack fonts",
            "Canonical; Chris Simpkins and the DejaVu and Bitstream Vera authors",
            "Ubuntu-font-1.0 AND MIT",
            "https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts",
            "egui's built-in fonts.",
            None,
        ),
        entry(
            Libraries,
            "heatshrink",
            "Scott Vokes (Atomic Object)",
            "ISC",
            "https://github.com/atomicobject/heatshrink",
            "Compression in binary G-code.",
            None,
        ),
        entry(
            Libraries,
            "MeatPack",
            "Scott Mudge",
            "BSD-3-Clause",
            "https://github.com/scottmudge/OctoPrint-MeatPack",
            "G-code packing in binary G-code, through libbgcode.",
            None,
        ),
        entry(
            Libraries,
            "QOI",
            "Dominic Szablewski",
            "MIT",
            "https://qoiformat.org",
            "Thumbnail images in binary G-code.",
            None,
        ),
    ]);
    out
}

/// One entry per texture source, each naming the bundled maps it made.
fn textures() -> Vec<Entry> {
    let names = |pick: &dyn Fn(&TextureCredit) -> bool| -> Vec<&str> {
        BUILTINS
            .iter()
            .filter(|b| pick(&b.credit))
            .map(|b| b.name)
            .collect()
    };
    let hero = names(&|c| *c == TextureCredit::HeroPatterns);
    let mut out = vec![entry(
        Group::Textures,
        "Hero Patterns",
        "Steve Schoger",
        "CC-BY-4.0",
        "https://heropatterns.com",
        &format!(
            "“Hero Patterns” by Steve Schoger (heropatterns.com), licensed under CC BY 4.0. \
             Made into height maps by CNC Kitchen for BumpMesh and resized for Mica. Used for \
             {} textures: {}.",
            hero.len(),
            hero.join(", ")
        ),
        None,
    )];
    for (host, name, by, link) in [
        (
            "ambientcg.com",
            "ambientCG",
            "Lennart Demes",
            "https://ambientcg.com",
        ),
        (
            "polyhaven.com",
            "Poly Haven",
            "The Poly Haven team",
            "https://polyhaven.com",
        ),
    ] {
        let used: Vec<String> = BUILTINS
            .iter()
            .filter_map(|b| match b.credit {
                TextureCredit::Cc0(src) if src.contains(host) => Some(format!(
                    "{} ({})",
                    b.name,
                    src.rsplit(['=', '/']).next().unwrap_or(src)
                )),
                _ => None,
            })
            .collect();
        if used.is_empty() {
            continue;
        }
        out.push(entry(
            Group::Textures,
            name,
            by,
            "CC0-1.0",
            link,
            &format!(
                "Public domain. Through BumpMesh, resized for Mica: {}.",
                used.join(", ")
            ),
            None,
        ));
    }
    out
}

#[cfg(test)]
/// `THIRD_PARTY_LICENSES.md` at the repository root: every Rust crate in Mica, with its
/// version, license, repository and copyright holders (the app's Open-source licenses view as
/// a file for GitHub).
pub fn third_party_licenses_md() -> String {
    // Markdown table cells: no pipes or line breaks, and `<email>` shown as text, not HTML.
    let cell = |s: &str| {
        s.replace('|', "\\|")
            .replace('\n', " ")
            .replace('*', "\\*")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let list = crates();
    let mut out = format!(
        "# Third-party licenses\n\n\
         <!-- Generated by `cargo xtask gen-credits` from crates/mica-gui/assets/credits/crates.tsv. Do not edit. -->\n\n\
         The {} Rust crates Mica Slicer's app and command line use on any platform, generated \
         from Cargo's dependency data, with each crate's license and copyright notice. The full \
         license texts are in [`crates/mica-gui/assets/credits/licenses/`](crates/mica-gui/assets/credits/licenses/). \
         The projects, ideas and assets Mica is built on are listed in [`THIRD_PARTY.md`](THIRD_PARTY.md) \
         and [`PROVENANCE.md`](PROVENANCE.md). The same lists are in the app under About Mica \
         Slicer › Credits and Open-source licenses.\n\n\
         | Crate | Version | License | Copyright |\n|---|---|---|---|\n",
        list.len()
    );
    for c in &list {
        let name = if c.repository.is_empty() {
            format!("`{}`", c.name)
        } else {
            format!("[`{}`]({})", c.name, c.repository)
        };
        out += &format!(
            "| {} | {} | {} | {} |\n",
            name,
            cell(c.version),
            cell(c.license),
            cell(&c.holders())
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIRD_PARTY_LICENSES: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../THIRD_PARTY_LICENSES.md");

    #[test]
    fn every_pinned_upstream_is_credited() {
        let pins = upstream_pins();
        assert!(pins.len() >= 10, "{pins:?}");
        let all = entries(true);
        for p in &pins {
            assert!(
                all.iter().any(|e| e.upstream == Some(p.id)),
                "upstream `{}` of UPSTREAM.lock has no credit",
                p.id
            );
        }
        // And every upstream of PROVENANCE.md's table.
        let provenance = include_str!("../../../PROVENANCE.md");
        let table = provenance
            .split("## Upstreams")
            .nth(1)
            .and_then(|s| s.split("\n\n").nth(1))
            .unwrap();
        for row in table.lines().skip(2) {
            let id = row.split('`').nth(1).unwrap();
            assert!(
                all.iter().any(|e| e.upstream == Some(id)),
                "PROVENANCE upstream `{id}` has no credit"
            );
        }
        // Their pins are shown.
        let prusa = all.iter().find(|e| e.name == "PrusaSlicer").unwrap();
        assert_eq!(prusa.pin().unwrap(), "version_2.9.6 · b028299c77");
    }

    #[test]
    fn every_built_in_model_not_made_by_mica_is_credited() {
        use mica_core::builtin_models::BuiltinModel;
        let all = entries(false);
        for m in BuiltinModel::ALL {
            if m.credit().is_some() {
                let e = all
                    .iter()
                    .find(|e| e.group == Group::Models && e.name == m.name())
                    .unwrap_or_else(|| panic!("{} has no credit entry", m.name()));
                assert_eq!(e.license, "CC0-1.0");
            }
        }
    }

    #[test]
    fn every_texture_and_source_is_credited() {
        let all = entries(false);
        let textures: Vec<&Entry> = all.iter().filter(|e| e.group == Group::Textures).collect();
        for b in BUILTINS {
            let (source, license) = match b.credit {
                TextureCredit::HeroPatterns => ("Hero Patterns", "CC-BY-4.0"),
                TextureCredit::Cc0(src) if src.contains("ambientcg.com") => {
                    ("ambientCG", "CC0-1.0")
                }
                TextureCredit::Cc0(src) if src.contains("polyhaven.com") => {
                    ("Poly Haven", "CC0-1.0")
                }
                TextureCredit::Cc0(src) => panic!("texture source {src} has no credit entry"),
            };
            let e = textures
                .iter()
                .find(|e| e.name == source)
                .unwrap_or_else(|| panic!("{source} missing"));
            assert_eq!(e.license, license);
            assert!(
                e.what.contains(b.name),
                "{} not named under {source}",
                b.name
            );
        }
        // CC BY 4.0 asks for the author, the license and the changes made.
        let hero = textures.iter().find(|e| e.name == "Hero Patterns").unwrap();
        assert!(hero.what.contains("Steve Schoger") && hero.what.contains("CC BY 4.0"));
        assert!(hero.what.contains("Made into height maps"));
        // BumpMesh, where the maps and the feature come from.
        assert!(all.iter().any(|e| e.upstream == Some("bumpmesh")
            && e.by.contains("Stefan Hermann")
            && e.by.contains("CNC Kitchen")));
    }

    #[test]
    fn required_credits_are_listed() {
        let all = entries(false);
        for name in [
            "PrusaSlicer",
            "OrcaSlicer",
            "Snapmaker Orca",
            "Slic3r",
            "BumpMesh",
            "Prusa ColorMix",
            "prusa-fdm-mixer",
            "Snapmaker mixed filaments",
            "camera-streamer",
            "Hero Patterns",
            "ambientCG",
            "Poly Haven",
            "egui and eframe",
            "wgpu",
            "str0m",
            "Clipper2",
            "clipper2-rust",
            "boostvoronoi",
            "redb",
        ] {
            assert!(all.iter().any(|e| e.name == name), "{name} missing");
        }
        let mixer = all.iter().find(|e| e.name == "prusa-fdm-mixer").unwrap();
        assert_eq!(mixer.license, "MIT");
        // Library entries take their license from the generated list.
        for e in all.iter().filter(|e| e.group == Group::Libraries) {
            assert!(!e.license.is_empty(), "{} has no license", e.name);
        }
    }

    #[test]
    fn brick_layers_only_while_unlocked() {
        let hidden = entries(false);
        assert!(
            !hidden.iter().any(|e| {
                let t = format!("{} {} {} {}", e.name, e.by, e.what, e.link).to_lowercase();
                t.contains("brick") || t.contains("geekdetour")
            }),
            "the hidden Brick layers feature is advertised"
        );
        let shown = entries(true);
        let brick = shown.iter().find(|e| e.name == "Brick Layers").unwrap();
        assert_eq!(brick.group, Group::Ideas);
        assert_eq!(brick.license, "GPL-3.0-or-later");
        assert!(brick.by.contains("GeekDetour"));
        assert!(
            shown
                .iter()
                .any(|e| e.by.contains("CNC Kitchen") && e.what.contains("strength"))
        );
        // The generated repository file never lists it.
        let file = third_party_licenses_md().to_lowercase();
        assert!(!file.contains("brick") && !file.contains("geekdetour"));
    }

    #[test]
    fn crate_list_matches_the_lockfile() {
        let lock = include_str!("../../../Cargo.lock");
        let list = crates();
        assert!(list.len() > 100, "{} crates", list.len());
        for c in &list {
            let needle = format!("name = \"{}\"\nversion = \"{}\"\n", c.name, c.version);
            assert!(
                lock.contains(&needle),
                "{} {} is not in Cargo.lock: run `cargo xtask gen-credits`",
                c.name,
                c.version
            );
            assert!(!c.license.is_empty(), "{} has no license", c.name);
        }
        for name in ["egui", "eframe", "wgpu", "str0m", "redb", "rayon", "serde"] {
            assert!(list.iter().any(|c| c.name == name), "{name} missing");
        }
        // Every direct dependency of the app (from the registry) is listed.
        let manifest = include_str!("../Cargo.toml");
        let mut in_deps = false;
        for line in manifest.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_deps = line.ends_with("dependencies]")
                    && !line.contains("dev-dependencies")
                    && !line.contains("build-dependencies");
                continue;
            }
            let Some((name, rest)) = line.split_once(" = ") else {
                continue;
            };
            let name = name.trim();
            if !in_deps
                || name.starts_with('#')
                || name.contains('.')
                || rest.contains("path =")
                || rest.contains("workspace")
            {
                continue;
            }
            assert!(
                list.iter().any(|c| c.name == name),
                "dependency {name} is not in the crate list: run `cargo xtask gen-credits`"
            );
        }
    }

    #[test]
    fn license_texts_are_bundled() {
        // Every license of the crate list has its full text in the app, except alternatives
        // Mica does not take.
        for c in crates() {
            for id in license_ids(c.license) {
                if [
                    "GPL-2.0-only",
                    "LGPL-2.1-or-later",
                    "Apache-2.0 WITH LLVM-exception",
                ]
                .contains(&id.as_str())
                {
                    continue;
                }
                assert!(license_text(&id).is_some(), "{}: no text for {id}", c.name);
            }
        }
        assert!(
            license_text("AGPL-3.0")
                .unwrap()
                .contains("GNU AFFERO GENERAL PUBLIC LICENSE")
        );
        assert!(
            license_text("Apache-2.0")
                .unwrap()
                .contains("Apache License")
        );
        assert_eq!(
            license_ids("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"),
            ["Apache-2.0 WITH LLVM-exception", "Apache-2.0", "MIT"]
        );
        assert_eq!(license_ids("MIT/Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert!(license_url("CC-BY-4.0").contains("creativecommons.org/licenses/by/4.0"));
    }

    #[test]
    fn every_entry_names_its_authors_and_a_link() {
        for e in entries(true) {
            assert!(!e.name.trim().is_empty(), "{e:?}");
            assert!(!e.by.trim().is_empty(), "{}", e.name);
            assert!(e.link.starts_with("http"), "{}: {}", e.name, e.link);
        }
    }

    /// `THIRD_PARTY_LICENSES.md` is generated from the crate list; `cargo xtask gen-credits`
    /// rewrites it.
    #[test]
    fn third_party_licenses_are_current() {
        let file = third_party_licenses_md();
        if std::env::var_os("MICA_WRITE_CREDITS").is_some() {
            std::fs::write(THIRD_PARTY_LICENSES, &file).unwrap();
            return;
        }
        let on_disk = std::fs::read_to_string(THIRD_PARTY_LICENSES).unwrap_or_default();
        assert!(
            on_disk == file,
            "THIRD_PARTY_LICENSES.md is stale: run `cargo xtask gen-credits`"
        );
        // One row per crate, with its version and copyright.
        let list = crates();
        let rows = file
            .lines()
            .filter(|l| l.starts_with("| `") || l.starts_with("| [`"));
        assert_eq!(rows.count(), list.len());
        for c in &list {
            assert!(
                file.contains(&format!("`{}`", c.name))
                    && file.contains(&format!(" {} |", c.version)),
                "{} missing",
                c.name
            );
        }
        assert!(file.contains("[`egui`](https://github.com/emilk/egui)"));
        assert!(file.contains("Lokathor"));
    }
}
