// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Opt-in pipeline measurements. Durations are inclusive elapsed time per call;
//! parallel calls are summed (worker-seconds), not mistaken for wall time.
//! No clock reads or atomic updates occur unless `MICA_TIMINGS=1`.
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    Load,
    DecodePainting,
    MeshSlicing,
    PaintingProjection,
    Segmentation,
    Regions,
    RegionPreparation,
    Walls,
    Fills,
    Supports,
    Seams,
    Export,
    WallCleanup,
    WallGraph,
    WallTransitions,
    Caps,
    RegionWalls,
    SharedFill,
    Shells,
    WallClipping,
    SeamBoundaries,
    SeamVisibility,
    SupportContacts,
    SupportVolumes,
    SupportTips,
    SupportPathing,
    SupportSmooth,
    SupportDraw,
    SupportFinalize,
    GraphVoronoi,
    GraphTransfer,
    GraphCollapse,
    SupportBridges,
    VoronoiBuild,
    VoronoiCheck,
    VoronoiCacheHit,
    VoronoiSweep,
    VoronoiCopy,
    ExportOverhang,
    ExportTravel,
    ExportPlan,
    ExportEmit,
    Postprocess,
    ToolUsage,
    BrickPlan,
    BrickApply,
    TextureDisplace,
}
const NAMES: [&str; 47] = [
    "load model / 3MF",
    "decode painting",
    "mesh slicing",
    "painting projection",
    "MMU segmentation",
    "regions",
    "region preparation",
    "walls (workers)",
    "fills (workers)",
    "supports",
    "seams",
    "export",
    "wall cleanup (workers)",
    "wall graph (workers)",
    "wall transitions (workers)",
    "material caps",
    "regional walls",
    "shared fill",
    "shells",
    "wall clipping (workers)",
    "seam boundaries",
    "seam visibility",
    "support contacts",
    "support volumes",
    "support tips",
    "support pathing",
    "organic smoothing",
    "support drawing",
    "support finalize",
    "graph Voronoi",
    "graph transfer",
    "graph collapse",
    "support bridge masks",
    "Voronoi sweep + copy",
    "Voronoi validation",
    "Voronoi cache hit",
    "Voronoi sweep",
    "Voronoi copy",
    "export overhangs",
    "export travel setup",
    "export curves",
    "export emit + cooling",
    "postprocess",
    "tool-usage prepass",
    "brick layers plan",
    "brick layers walls (workers)",
    "texture displacement",
];
static NANOS: [AtomicU64; NAMES.len()] = [const { AtomicU64::new(0) }; NAMES.len()];
static CALLS: [AtomicU64; NAMES.len()] = [const { AtomicU64::new(0) }; NAMES.len()];
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MICA_TIMINGS").is_some_and(|v| v == "1"))
}
/// Reference instant for the "ended at" offsets of the printed spans (first report or span).
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

pub struct Span {
    stage: Stage,
    start: Option<Instant>,
}
impl Span {
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            start: enabled().then(|| {
                origin();
                Instant::now()
            }),
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let index = self.stage as usize;
            let elapsed = start.elapsed();
            NANOS[index].fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
            if matches!(
                self.stage,
                Stage::Load
                    | Stage::MeshSlicing
                    | Stage::PaintingProjection
                    | Stage::Segmentation
                    | Stage::Regions
                    | Stage::RegionPreparation
                    | Stage::Seams
                    | Stage::Supports
                    | Stage::SupportVolumes
                    | Stage::SupportTips
                    | Stage::SupportPathing
                    | Stage::SupportSmooth
                    | Stage::SupportDraw
                    | Stage::SupportFinalize
                    | Stage::Export
                    | Stage::Caps
                    | Stage::RegionWalls
                    | Stage::SharedFill
                    | Stage::Shells
                    | Stage::ToolUsage
            ) {
                eprintln!(
                    "MICA_TIMINGS finished {}: {:.3} s (ended at {:.3} s)",
                    NAMES[index],
                    elapsed.as_secs_f64(),
                    origin().elapsed().as_secs_f64()
                );
            }
            CALLS[index].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Print accumulated stage measurements at the end of a CLI/example run, including errors.
/// Nested rows overlap; wall/fill rows sum work on all Rayon workers.
pub struct Report(Option<Instant>);
impl Default for Report {
    fn default() -> Self {
        if enabled() {
            origin();
            eprintln!("MICA_TIMINGS pid {}", std::process::id());
        }
        Self(enabled().then(Instant::now))
    }
}
impl Drop for Report {
    fn drop(&mut self) {
        if let Some(start) = self.0 {
            eprintln!("MICA_TIMINGS inclusive elapsed seconds (parallel calls summed):");
            for (i, name) in NAMES.iter().enumerate() {
                eprintln!(
                    "  {name:24} {:10.3} s {:8} calls",
                    NANOS[i].load(Ordering::Relaxed) as f64 / 1e9,
                    CALLS[i].load(Ordering::Relaxed)
                );
            }
            eprintln!(
                "  total wall time          {:10.3} s",
                start.elapsed().as_secs_f64()
            );
        }
    }
}
