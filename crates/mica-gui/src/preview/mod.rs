// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

pub mod attrs;
pub mod color_print;
pub mod layer_gcode;
pub mod legend;
pub mod slider;
pub mod stats;
use crate::worker::TempJob;
pub use attrs::Attr;
use bytemuck::{Pod, Zeroable};
use mica_core::{
    config::Config, geometry::unscale, print::PathEmission, print_job::ScheduledLayer,
    settings::PrintSettings, toolpath::Role,
};
use mica_gcode::cooling::{self, CoolingConfig};
pub use stats::Stats;
use std::{
    fs::File,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    sync::Arc,
};

/// 32 bytes, directly consumed as an instanced GPU vertex. Width/height in micrometres.
/// meta: role (4), tool (4), fan percent (8), speed mm/s (16).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Segment {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub size: u32,
    pub meta: u32,
}
impl Segment {
    pub fn new(
        a: [f32; 3],
        b: [f32; 3],
        dimensions: [f64; 2],
        role: u32,
        tool: usize,
        speed: f64,
        fan: f64,
    ) -> Self {
        let [width, height] = dimensions;
        Self {
            a,
            b,
            size: ((width * 1000.).clamp(1., 65535.) as u32)
                | (((height * 1000.).clamp(1., 65535.) as u32) << 16),
            meta: role
                | ((tool.min(15) as u32) << 4)
                | ((fan.clamp(0., 100.) as u32) << 8)
                | ((speed.round().clamp(0., 65535.) as u32) << 16),
        }
    }
    pub fn role(&self) -> u32 {
        self.meta & 15
    }
    pub fn tool(&self) -> usize {
        ((self.meta >> 4) & 15) as usize
    }
    pub fn speed(&self) -> f32 {
        (self.meta >> 16) as f32
    }
}
pub const TRAVEL: u32 = 14;
pub const TOWER: u32 = 15;
pub const BATCH: usize = 65536;
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub start: u64,
    pub count: u64,
    pub z: f32,
    /// Estimated print time of the layer in seconds (after cooling slowdown).
    pub time_s: f32,
}
pub struct Preview {
    pub job: Arc<TempJob>,
    pub layers: Vec<Layer>,
    pub count: u64,
    pub stats: Stats,
    /// The exporter's print time (its time estimator over the real G-code), when known.
    pub total_time_s: Option<f64>,
    /// Colour segments of the plate's colour and tool changes, when it has any.
    pub color_print: Option<color_print::ColorPrint>,
    /// An opened G-code file's extrusions without a known feature type sit in a role slot the
    /// file does not use otherwise; this names that slot (`Unknown` / `Custom`).
    pub relabel: Option<(usize, &'static str)>,
}
impl Preview {
    /// The legend / tooltip name of a record role (`legend::NAMES`, or the relabelled slot).
    pub fn role_name(&self, role: usize) -> &'static str {
        match self.relabel {
            Some((slot, name)) if slot == role => name,
            _ => legend::NAMES[role.min(15)],
        }
    }
    /// Print time to show: the exporter's when known, else the sum of segment times.
    pub fn total_time(&self) -> f64 {
        self.total_time_s.unwrap_or_else(|| self.stats.total_time())
    }
    pub fn read_batches(
        &self,
        range: std::ops::Range<u64>,
        mut consume: impl FnMut(&[Segment]),
    ) -> io::Result<()> {
        let mut file = File::open(self.job.path.join("preview.bin"))?;
        file.seek(SeekFrom::Start(range.start * 32))?;
        let mut buffer = vec![Segment::zeroed(); BATCH];
        let mut remaining = range.end.saturating_sub(range.start);
        while remaining > 0 {
            let n = (remaining as usize).min(BATCH);
            file.read_exact(bytemuck::cast_slice_mut(&mut buffer[..n]))?;
            consume(&buffer[..n]);
            remaining -= n as u64;
        }
        Ok(())
    }
    /// Like `read_batches`, with each batch's per-record attributes (`attrs.bin`, same order).
    /// A preview without the attribute file reads zeros.
    pub fn read_batches_with_attrs(
        &self,
        range: std::ops::Range<u64>,
        mut consume: impl FnMut(&[Segment], &[Attr]),
    ) -> io::Result<()> {
        let mut attrs = File::open(self.job.path.join("attrs.bin")).ok();
        if let Some(f) = &mut attrs
            && f.seek(SeekFrom::Start(range.start * attrs::SIZE as u64))
                .is_err()
        {
            attrs = None;
        }
        let mut buffer = vec![Attr::zeroed(); BATCH];
        self.read_batches(range, |segments| {
            let n = segments.len();
            let ok = attrs.as_mut().is_some_and(|f| {
                f.read_exact(bytemuck::cast_slice_mut(&mut buffer[..n]))
                    .is_ok()
            });
            if !ok {
                buffer[..n].fill(Attr::zeroed());
            }
            consume(segments, &buffer[..n]);
        })
    }
    /// One record's attributes (hover card).
    pub fn attr_at(&self, index: u64) -> io::Result<Attr> {
        let mut file = File::open(self.job.path.join("attrs.bin"))?;
        file.seek(SeekFrom::Start(index * attrs::SIZE as u64))?;
        let mut a = Attr::zeroed();
        file.read_exact(bytemuck::bytes_of_mut(&mut a))?;
        Ok(a)
    }
    /// One record, for the tool-head marker at the move slider position.
    pub fn segment_at(&self, index: u64) -> io::Result<Segment> {
        if index >= self.count {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "out of range"));
        }
        let mut file = File::open(self.job.path.join("preview.bin"))?;
        file.seek(SeekFrom::Start(index * 32))?;
        let mut s = Segment::zeroed();
        file.read_exact(bytemuck::bytes_of_mut(&mut s))?;
        Ok(s)
    }
    /// Layer heights (print_z) in mm, as the engine has them: the preview stores `f32`, so
    /// they are rounded to 0.1 µm steps (`10.2`, not `10.199999809`) for custom G-code items.
    pub fn layer_zs(&self) -> Vec<f64> {
        self.layers
            .iter()
            .map(|l| (l.z as f64 * 1e4).round() / 1e4)
            .collect()
    }
    /// Layer index containing a record.
    pub fn layer_of(&self, index: u64) -> usize {
        match self.layers.binary_search_by(|l| l.start.cmp(&index)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        }
    }
}
pub struct Builder {
    file: BufWriter<File>,
    /// Per-record attributes, parallel to `file`.
    attrs: BufWriter<File>,
    pub layers: Vec<Layer>,
    pub count: u64,
    last: Option<[f32; 3]>,
    cooling: Vec<CoolingConfig>,
    stats: Stats,
    config: Config,
    machine: attrs::Machine,
    settings: Vec<PrintSettings>,
    /// Nozzle temperature a calibration control set per tool (it holds until the next one).
    temperature: Vec<Option<f64>>,
    /// Calibration temperatures of the next pushed layer's paths (`None` = unchanged).
    pub path_temperatures: Vec<Option<f64>>,
}
impl Builder {
    pub fn new(job: &TempJob, config: &Config, settings: &[PrintSettings]) -> io::Result<Self> {
        Ok(Self {
            file: BufWriter::new(File::create(job.path.join("preview.bin"))?),
            attrs: BufWriter::new(File::create(job.path.join("attrs.bin"))?),
            layers: vec![],
            count: 0,
            last: None,
            cooling: settings
                .iter()
                .map(|s| CoolingConfig::from_config(config, s))
                .collect(),
            stats: Stats::default(),
            config: config.clone(),
            machine: attrs::Machine::from_config(config),
            settings: settings.to_vec(),
            temperature: vec![None; settings.len().max(1)],
            path_temperatures: vec![],
        })
    }
    /// Nozzle temperature of `tool` on layer `layer` (a calibration override, else the profile's
    /// first-layer / normal temperature).
    fn temperature_of(&self, tool: usize, layer: usize) -> f64 {
        if let Some(Some(t)) = self.temperature.get(tool) {
            return *t;
        }
        let Some(s) = self
            .settings
            .get(tool.min(self.settings.len().saturating_sub(1)))
        else {
            return 0.;
        };
        if layer == 0 {
            s.first_layer_temperature
        } else {
            s.temperature
        }
    }
    /// Writes a layer's (or tower visit's) records with their attributes and statistics.
    /// `records` pairs each record with the index of its path (for calibration temperatures).
    fn write_records(
        &mut self,
        records: &[(Segment, usize)],
        layer_index: usize,
        stats_layer: usize,
    ) -> io::Result<()> {
        let overrides = std::mem::take(&mut self.path_temperatures);
        let mut temps = Vec::with_capacity(records.len());
        for (s, path) in records {
            let tool = s.tool();
            if let Some(Some(t)) = overrides.get(*path) {
                if tool >= self.temperature.len() {
                    self.temperature.resize(tool + 1, None);
                }
                self.temperature[tool] = Some(*t);
            }
            temps.push(self.temperature_of(tool, layer_index));
        }
        let segments: Vec<Segment> = records.iter().map(|(s, _)| *s).collect();
        let attrs = attrs::compute(
            &segments,
            &temps,
            &self.settings,
            &self.config,
            &self.machine,
            layer_index == 0,
        );
        for (segment, attr) in segments.iter().zip(&attrs) {
            self.file.write_all(bytemuck::bytes_of(segment))?;
            self.attrs.write_all(bytemuck::bytes_of(attr))?;
            self.count += 1;
            self.stats.add(segment, stats_layer);
            self.stats.add_attr(segment, attr);
        }
        Ok(())
    }
    /// Adds a layer with speeds and fans estimated the way the exporter computes them.
    pub fn push(&mut self, layer: &ScheduledLayer, settings: &[PrintSettings]) -> io::Result<()> {
        self.push_with(layer, settings, None, None, None)
    }
    /// Adds a layer using what the exporter actually emitted (final feedrates and fans per
    /// path stretch, and the layer's cooling-adjusted time). `spiral`: the exporter prints the
    /// layer as a spiral vase spiral (`Some(true)`: the last layer, with its finishing loop).
    pub fn push_emitted(
        &mut self,
        layer: &ScheduledLayer,
        settings: &[PrintSettings],
        emissions: &[PathEmission],
        layer_time_s: f64,
        spiral: Option<bool>,
    ) -> io::Result<()> {
        self.push_with(layer, settings, Some(emissions), Some(layer_time_s), spiral)
    }
    fn push_with(
        &mut self,
        layer: &ScheduledLayer,
        settings: &[PrintSettings],
        emissions: Option<&[PathEmission]>,
        layer_time_s: Option<f64>,
        spiral: Option<bool>,
    ) -> io::Result<()> {
        let start = self.count;
        let index = self.layers.len();
        let below = self.layers.last().map_or(0., |l| l.z);
        // One layer's records are buffered (bounded by the layer) so the acceleration planner
        // sees them in order before they are written.
        let mut records: Vec<(Segment, usize)> = Vec::new();
        let mut sink = |segment: Segment, path: usize| records.push((segment, path));
        match emissions {
            Some(e) => emitted_segments(layer, settings, e, &mut self.last, &mut sink),
            None => segments(layer, settings, &self.cooling, &mut self.last, &mut sink),
        }
        if let Some(last) = spiral {
            spiral_ramp(&mut records, below, layer.layer.print_z as f32, last);
        }
        self.write_records(&records, layer.layer.index, index)?;
        self.layers.push(Layer {
            start,
            count: self.count - start,
            z: layer.layer.print_z as f32,
            time_s: 0.,
        });
        let estimated = self.stats.finish_layer();
        if let Some(l) = self.layers.last_mut() {
            l.time_s += layer_time_s.map_or(estimated, |t| t as f32);
        }
        Ok(())
    }
    fn close_layer(&mut self) {
        let t = self.stats.finish_layer();
        if let Some(l) = self.layers.last_mut() {
            l.time_s += t;
        }
    }
    /// Adds records parsed from a G-code file (`gcode_doc`), with their attributes, as a new
    /// layer at `z`, or (`new_layer` false) appended to the last one. `time_s` is the records'
    /// planned time; the layer's time is their sum.
    pub fn push_parsed(
        &mut self,
        segments: &[Segment],
        attrs: &[Attr],
        z: f32,
        time_s: f32,
        new_layer: bool,
    ) -> io::Result<()> {
        if new_layer || self.layers.is_empty() {
            self.layers.push(Layer {
                start: self.count,
                count: 0,
                z,
                time_s: 0.,
            });
        }
        let index = self.layers.len() - 1;
        for (segment, attr) in segments.iter().zip(attrs) {
            self.file.write_all(bytemuck::bytes_of(segment))?;
            self.attrs.write_all(bytemuck::bytes_of(attr))?;
            self.count += 1;
            self.stats.add(segment, index);
            self.stats.add_attr(segment, attr);
        }
        // The nominal (feed-rate) time is not the layer's: the planned one is.
        self.stats.finish_layer();
        if let Some(l) = self.layers.last_mut() {
            l.count = self.count - l.start;
            l.time_s += time_s;
        }
        Ok(())
    }
    /// Include the engine's lazy tower geometry without retaining its paths.
    pub fn tower(
        &mut self,
        visits: impl IntoIterator<Item = mica_core::wipe_tower::TowerVisit>,
    ) -> io::Result<()> {
        let index = self.layers.len().saturating_sub(1);
        let mut records: Vec<(Segment, usize)> = Vec::new();
        for visit in visits {
            for (block, tool) in visit
                .before
                .iter()
                .map(|b| (b, visit.old_tool))
                .chain(std::iter::once((&visit.after, visit.tool)))
            {
                let mut a = [block.start[0] as f32, block.start[1] as f32, visit.z as f32];
                for m in &block.moves {
                    let b = [m.xy[0] as f32, m.xy[1] as f32, visit.z as f32];
                    if a != b {
                        let role = if m.volume > 0. { TOWER } else { TRAVEL };
                        let segment = Segment::new(
                            a,
                            b,
                            [m.width.max(0.08), visit.height],
                            role,
                            tool,
                            m.speed,
                            0.,
                        );
                        records.push((segment, usize::MAX));
                    }
                    a = b;
                }
            }
        }
        self.write_records(&records, index, index)?;
        if let Some(layer) = self.layers.last_mut() {
            layer.count = self.count - layer.start;
        }
        self.close_layer();
        Ok(())
    }
    #[cfg(test)]
    pub fn finish(self, job: Arc<TempJob>) -> io::Result<Preview> {
        self.finish_with(job, None)
    }
    #[cfg(test)]
    pub fn stats_mut(&mut self) -> &mut Stats {
        &mut self.stats
    }
    /// Like `finish`, recording the exporter's total print time.
    pub fn finish_with(
        mut self,
        job: Arc<TempJob>,
        total_time_s: Option<f64>,
    ) -> io::Result<Preview> {
        self.file.flush()?;
        self.attrs.flush()?;
        for l in &self.layers {
            if l.time_s > 0. {
                self.stats.layer_time.add(l.time_s);
            }
        }
        Ok(Preview {
            job,
            layers: self.layers,
            count: self.count,
            stats: self.stats,
            total_time_s,
            color_print: None,
            relabel: None,
        })
    }
}
/// A spiral vase layer as the exporter prints it (`mica_gcode::spiral_vase`): its extrusions
/// rise from the layer below (`below`) to the layer's own Z in proportion to the length printed
/// (the travel onto the loop is not printed), and the last layer prints its loop once more
/// at its own height. The XY smoothing towards the previous layer is not drawn.
fn spiral_ramp(records: &mut Vec<(Segment, usize)>, below: f32, z: f32, last: bool) {
    records.retain(|(s, _)| s.role() != TRAVEL);
    let length = |s: &Segment| (s.b[0] - s.a[0]).hypot(s.b[1] - s.a[1]);
    let total: f32 = records.iter().map(|(s, _)| length(s)).sum();
    if total <= 0. {
        return;
    }
    let finish: Vec<(Segment, usize)> = if last {
        records
            .iter()
            .map(|&(mut s, path)| {
                s.a[2] = z;
                s.b[2] = z;
                (s, path)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut done = 0.;
    for (s, _) in records.iter_mut() {
        s.a[2] = below + (z - below) * done / total;
        done += length(s);
        s.b[2] = below + (z - below) * done / total;
    }
    records.extend(finish);
}

/// Emits one record per path segment plus connecting travels, with the speed and fan the
/// exporter settled on for each stretch of a path.
fn emitted_segments(
    layer: &ScheduledLayer,
    settings: &[PrintSettings],
    emissions: &[PathEmission],
    last: &mut Option<[f32; 3]>,
    mut emit: impl FnMut(Segment, usize),
) {
    let tool_of = |i: usize| layer.assignments.get(i).map_or(0, |a| a.tool);
    let setting = |tool: usize| &settings[tool.min(settings.len() - 1)];
    for (i, path) in layer.layer.paths.iter().enumerate() {
        // Brick-layers walls print raised above the layer.
        let point = |p: mica_core::geometry::Point| {
            [
                unscale(p.x) as f32,
                unscale(p.y) as f32,
                (layer.layer.print_z + path.z_raise()) as f32,
            ]
        };
        let tool = tool_of(i);
        let s = setting(tool);
        let stretches: Vec<&PathEmission> = emissions.iter().filter(|e| e.path == i).collect();
        let fallback = nominal_speed(path.role, s, layer.layer.index == 0);
        let for_point = |k: usize| -> (f64, f64) {
            stretches
                .iter()
                .find(|e| k >= e.points.start && k < e.points.end.max(e.points.start + 1))
                .or_else(|| stretches.last())
                .map_or((fallback, 0.), |e| (e.speed_mm_s, e.fan_percent))
        };
        if let Some(&first) = path.points.first() {
            let b = point(first);
            if let Some(a) = *last
                && a != b
            {
                emit(
                    Segment::new(a, b, [0.08, 0.04], TRAVEL, tool, s.travel_speed, 0.),
                    i,
                );
            }
        }
        for (k, pair) in path.points.windows(2).enumerate() {
            let a = point(pair[0]);
            let b = point(pair[1]);
            if a != b {
                let (speed, fan) = for_point(k);
                emit(
                    Segment::new(
                        a,
                        b,
                        [path.flow.width, path.flow.height],
                        path.role as u32,
                        tool,
                        speed,
                        fan,
                    ),
                    i,
                );
            }
        }
        if let Some(&p) = path.points.last() {
            *last = Some(point(p));
        }
    }
}
/// Emits one record per path segment plus connecting travels. Speeds are the nominal /
/// volumetric-limited values run through the same layer-time slowdown as the G-code exporter,
/// so speed and fan colouring follow what the printer receives (travel routing excepted).
fn segments(
    layer: &ScheduledLayer,
    settings: &[PrintSettings],
    cooling: &[CoolingConfig],
    last: &mut Option<[f32; 3]>,
    mut emit: impl FnMut(Segment, usize),
) {
    let tool_of = |i: usize| layer.assignments.get(i).map_or(0, |a| a.tool);
    let setting = |tool: usize| &settings[tool.min(settings.len() - 1)];
    let first = layer.layer.index == 0;
    let mut moves: Vec<cooling::Move> = Vec::with_capacity(layer.layer.paths.len());
    let mut fixed = 0.;
    let mut previous = *last;
    for (i, path) in layer.layer.paths.iter().enumerate() {
        let s = setting(tool_of(i));
        let speed =
            nominal_speed(path.role, s, first).min(if s.filament_max_volumetric_speed > 0. {
                s.filament_max_volumetric_speed / path.mm3_per_mm()
            } else {
                f64::INFINITY
            });
        let length: f64 = path
            .points
            .windows(2)
            .map(|w| {
                let (dx, dy) = (unscale(w[1].x - w[0].x), unscale(w[1].y - w[0].y));
                (dx * dx + dy * dy).sqrt()
            })
            .sum();
        if let (Some(p), Some(q)) = (previous, path.points.first()) {
            let dx = unscale(q.x) - p[0] as f64;
            let dy = unscale(q.y) - p[1] as f64;
            fixed += (dx * dx + dy * dy).sqrt() / s.travel_speed.max(1.);
        }
        previous = path
            .points
            .last()
            .map(|p| [unscale(p.x) as f32, unscale(p.y) as f32, 0.]);
        moves.push(cooling::Move {
            length_mm: length,
            speed: speed.max(0.1),
            external: path.role == Role::ExternalPerimeter,
            adjustable: !path.role.is_bridge(),
            perimeter_index: path.perimeter_index,
            tool: tool_of(i).min(cooling.len().saturating_sub(1)),
            ..Default::default()
        });
    }
    let primary = cooling
        .get(tool_of(0).min(cooling.len().saturating_sub(1)))
        .copied();
    let layer_time = match primary {
        Some(_) if !moves.is_empty() => cooling::slow_down_multi(&mut moves, fixed, cooling),
        _ => fixed + moves.iter().map(|m| m.length_mm / m.speed).sum::<f64>(),
    };
    for (i, path) in layer.layer.paths.iter().enumerate() {
        let tool = tool_of(i);
        let s = setting(tool);
        let speed = moves[i].speed;
        let fan = match cooling.get(tool.min(cooling.len().saturating_sub(1))) {
            Some(cfg) => {
                let normal = cooling::fan_speed(layer.layer.index, layer_time, cfg);
                if path.role.is_bridge() && layer.layer.index >= cfg.disable_fan_first_layers {
                    normal.max(s.bridge_fan_speed)
                } else {
                    normal
                }
            }
            None => {
                if layer.layer.index < s.disable_fan_first_layers {
                    0.
                } else if path.role.is_bridge() {
                    s.bridge_fan_speed
                } else {
                    s.min_fan_speed
                }
            }
        };
        let point = |p: mica_core::geometry::Point| {
            [
                unscale(p.x) as f32,
                unscale(p.y) as f32,
                (layer.layer.print_z + path.z_raise()) as f32,
            ]
        };
        if let Some(&first) = path.points.first() {
            let b = point(first);
            if let Some(a) = *last
                && a != b
            {
                emit(
                    Segment::new(a, b, [0.08, 0.04], TRAVEL, tool, s.travel_speed, 0.),
                    i,
                );
            }
        }
        for pair in path.points.windows(2) {
            let a = point(pair[0]);
            let b = point(pair[1]);
            if a != b {
                emit(
                    Segment::new(
                        a,
                        b,
                        [path.flow.width, path.flow.height],
                        path.role as u32,
                        tool,
                        speed,
                        fan,
                    ),
                    i,
                );
            }
        }
        if let Some(&p) = path.points.last() {
            *last = Some(point(p));
        }
    }
}
fn nominal_speed(role: Role, s: &PrintSettings, first: bool) -> f64 {
    if first {
        return s.first_layer_speed;
    }
    match role {
        Role::ExternalPerimeter => s.external_perimeter_speed,
        Role::Perimeter | Role::Skirt | Role::Brim => s.perimeter_speed,
        Role::InternalInfill => s.infill_speed,
        Role::SolidInfill => s.solid_infill_speed,
        Role::TopSolidInfill | Role::Ironing => s.top_solid_infill_speed,
        Role::GapFill => s.gap_fill_speed,
        Role::SupportMaterial => s.support_material_speed,
        Role::SupportMaterialInterface => s.support_material_interface_speed,
        _ => s.bridge_speed,
    }
}
/// Preview colour schemes (PrusaSlicer / Orca `GCodeViewer` view types). The discriminant is
/// the shader's mode number (`segments.wgsl`), so existing values never change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    Feature = 0,
    Tool = 1,
    Speed = 2,
    Fan = 3,
    Width = 4,
    Height = 5,
    LayerTime = 6,
    ActualSpeed = 7,
    VolumetricFlow = 8,
    Temperature = 9,
    Acceleration = 10,
    LayerTimeLog = 11,
    ColorPrint = 12,
}
impl ColorMode {
    /// In menu order (upstream's order, acceleration after flow).
    pub const ALL: [Self; 13] = [
        Self::Feature,
        Self::Height,
        Self::Width,
        Self::Speed,
        Self::ActualSpeed,
        Self::Fan,
        Self::Temperature,
        Self::VolumetricFlow,
        Self::Acceleration,
        Self::LayerTime,
        Self::LayerTimeLog,
        Self::Tool,
        Self::ColorPrint,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Feature => "Feature",
            Self::Tool => "Filament",
            Self::Speed => "Speed",
            Self::Fan => "Fan",
            Self::Width => "Line width",
            Self::Height => "Layer height",
            Self::LayerTime => "Layer time",
            Self::ActualSpeed => "Actual speed",
            Self::VolumetricFlow => "Volumetric flow",
            Self::Temperature => "Temperature",
            Self::Acceleration => "Acceleration",
            Self::LayerTimeLog => "Layer time (log)",
            Self::ColorPrint => "Color print",
        }
    }
    /// Command-line / file name of the mode (`--color actual-speed`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Tool => "tool",
            Self::Speed => "speed",
            Self::Fan => "fan",
            Self::Width => "width",
            Self::Height => "height",
            Self::LayerTime => "layertime",
            Self::ActualSpeed => "actual-speed",
            Self::VolumetricFlow => "flow",
            Self::Temperature => "temperature",
            Self::Acceleration => "acceleration",
            Self::LayerTimeLog => "layertime-log",
            Self::ColorPrint => "color-print",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.replace('_', "-").to_lowercase();
        Self::ALL
            .into_iter()
            .find(|m| m.key() == name)
            .or(match name.as_str() {
                "filament" => Some(Self::Tool),
                "actual" => Some(Self::ActualSpeed),
                "volumetric" | "volumetric-flow" => Some(Self::VolumetricFlow),
                "temp" => Some(Self::Temperature),
                "accel" => Some(Self::Acceleration),
                "colour-print" | "colorprint" => Some(Self::ColorPrint),
                "layer-time" => Some(Self::LayerTime),
                "layer-time-log" => Some(Self::LayerTimeLog),
                _ => None,
            })
    }
    pub fn unit(self) -> &'static str {
        match self {
            Self::Speed | Self::ActualSpeed => "mm/s",
            Self::Fan => "%",
            Self::Width | Self::Height => "mm",
            Self::LayerTime | Self::LayerTimeLog => "s",
            Self::VolumetricFlow => "mm³/s",
            Self::Temperature => "°C",
            Self::Acceleration => "mm/s²",
            _ => "",
        }
    }
    /// A gradient scheme (legend with a range) rather than one of discrete rows.
    pub fn is_gradient(self) -> bool {
        !matches!(self, Self::Feature | Self::Tool | Self::ColorPrint)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub low: usize,
    pub high: usize,
    pub moves: u64,
    /// Visible feature roles, one bit per role (travel is bit 14).
    pub mask: u32,
    /// Visible tools, one bit per tool.
    pub tool_mask: u32,
    pub color: ColorMode,
    /// The layer slider's custom G-code menu and editors.
    pub layer_ui: layer_gcode::LayerUi,
}
impl Default for View {
    fn default() -> Self {
        Self {
            low: 0,
            high: usize::MAX,
            moves: u64::MAX,
            mask: ((1 << 16) - 1) & !(1 << TRAVEL),
            tool_mask: u32::MAX,
            color: ColorMode::Feature,
            layer_ui: Default::default(),
        }
    }
}
impl View {
    pub fn range(&self, p: &Preview) -> std::ops::Range<u64> {
        if p.layers.is_empty() {
            return 0..0;
        }
        let high = self.high.min(p.layers.len() - 1);
        let low = self.low.min(high);
        p.layers[low].start..p.layers[high].start + self.moves.min(p.layers[high].count)
    }
    /// Gradient `[min, max]` for the current colour mode from the preview statistics.
    /// A range narrower than the labels show is one value (`legend::single_value`): it is
    /// centred on a span of a full unit, so every line gets the scale's middle color.
    pub fn gradient_range(&self, p: &Preview) -> [f32; 2] {
        if let Some(v) = legend::single_value(self.color, p) {
            return [v - 0.5, v + 0.5];
        }
        let fallback = crate::render::default_range(self.color);
        match self.color {
            ColorMode::Speed => p.stats.speed.span(fallback),
            ColorMode::Fan => p.stats.fan.span(fallback),
            ColorMode::Width => p.stats.width.span(fallback),
            ColorMode::Height => p.stats.height.span(fallback),
            ColorMode::LayerTime | ColorMode::LayerTimeLog => p.stats.layer_time.span(fallback),
            ColorMode::ActualSpeed => p.stats.actual.span(fallback),
            ColorMode::VolumetricFlow => p.stats.flow.span(fallback),
            ColorMode::Temperature => p.stats.temp.span(fallback),
            ColorMode::Acceleration => p.stats.accel.span(fallback),
            _ => fallback,
        }
    }
    /// The range the shader maps onto the gradient: the logarithm of the layer-time range for
    /// the logarithmic view (layer times below 0.1 s are clamped), else `gradient_range`.
    pub fn shader_range(&self, p: &Preview) -> [f32; 2] {
        let [lo, hi] = self.gradient_range(p);
        if self.color == ColorMode::LayerTimeLog {
            [lo.max(0.1).ln(), hi.max(lo.max(0.1) * 1.01).ln()]
        } else {
            [lo, hi]
        }
    }
    pub fn travel_visible(&self) -> bool {
        self.mask & (1 << TRAVEL) != 0
    }
    /// Whether a record passes the feature and tool visibility masks.
    pub fn shows(&self, s: &Segment) -> bool {
        self.mask & (1 << s.role()) != 0 && self.tool_mask & (1 << s.tool()) != 0
    }
    /// Record index of the last visible move, if any.
    pub fn head_index(&self, p: &Preview) -> Option<u64> {
        let r = self.range(p);
        (r.end > r.start).then(|| r.end - 1)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use mica_core::{
        flow::Flow,
        geometry::Point,
        toolpath::{ExtrusionPath, LayerToolpaths},
    };
    fn job() -> (Arc<TempJob>, Config, Vec<PrintSettings>) {
        (
            Arc::new(TempJob::new().unwrap()),
            Config::defaults(),
            vec![PrintSettings::core_one_hf04_speed_pla()],
        )
    }
    fn layer(points: usize, index: usize) -> ScheduledLayer {
        let path = ExtrusionPath {
            role: Role::ExternalPerimeter,
            points: (0..points)
                .map(|i| Point::new(i as i64 * 1_000_000, 0))
                .collect(),
            flow: Flow::new(0.45, 0.2),
            seam: None,
            overhang: None,
            perimeter_index: None,
            brick: None,
        };
        ScheduledLayer {
            layer: LayerToolpaths {
                support_groups: Vec::new(),
                index,
                print_z: 0.2 * (index + 1) as f64,
                height: 0.2,
                paths: vec![path.clone(), path],
            },
            assignments: vec![
                mica_core::print_job::PathAssignment {
                    object: 0,
                    tool: 2,
                    region: None
                };
                2
            ],
        }
    }
    #[test]
    fn packed_segments_counts_and_attributes() {
        assert_eq!(std::mem::size_of::<Segment>(), 32);
        let mut result = Vec::new();
        let settings = [PrintSettings::core_one_hf04_speed_pla()];
        segments(&layer(3, 0), &settings, &[], &mut None, |s, _| {
            result.push(s)
        });
        assert_eq!(result.len(), 5);
        assert_eq!(result[2].role(), TRAVEL);
        assert_eq!(result[0].tool(), 2);
        assert_eq!(result[0].size & 65535, 450);
        assert_eq!(result[0].size >> 16, 200);
        assert_eq!(result[0].b, [1., 0., 0.2]);
        assert_eq!(result[0].speed(), settings[0].first_layer_speed as f32);
    }
    #[test]
    fn cooling_slowdown_is_reflected_in_speed_and_fan() {
        let (job, config, settings) = job();
        let mut b = Builder::new(&job, &config, &settings).unwrap();
        // Two tiny paths: a short layer well below slowdown_below_layer_time.
        b.push(&layer(2, 5), &settings).unwrap();
        let p = b.finish(job).unwrap();
        let s = p.segment_at(0).unwrap();
        let cfg = &settings[0];
        assert!(cfg.slowdown_below_layer_time > 0.);
        assert!(
            (s.speed() as f64) < cfg.external_perimeter_speed,
            "{} vs {}",
            s.speed(),
            cfg.external_perimeter_speed
        );
        // Two millimetres of path cannot reach the target time: speed clamps at the minimum.
        assert!((s.speed() as f64 - cfg.min_print_speed).abs() <= 1.);
        assert_eq!((s.meta >> 8) & 255, cfg.max_fan_speed as u32);
        assert!(p.layers[0].time_s > 0.);
        assert_eq!(p.stats.layer_time.min, p.layers[0].time_s);
    }
    /// A spiral vase layer rises from the layer below in proportion to the length printed,
    /// without its travel; the last one also prints a flat finishing loop.
    #[test]
    fn spiral_layers_rise_along_their_length() {
        let segment = |a: [f32; 2], b: [f32; 2], role| {
            (
                Segment::new(
                    [a[0], a[1], 1.],
                    [b[0], b[1], 1.],
                    [0.4, 0.2],
                    role,
                    0,
                    50.,
                    0.,
                ),
                0,
            )
        };
        let layer = vec![
            segment([0., 0.], [1., 0.], TRAVEL),
            segment([1., 0.], [4., 0.], Role::ExternalPerimeter as u32),
            segment([4., 0.], [4., 1.], Role::ExternalPerimeter as u32),
        ];
        let mut records = layer.clone();
        spiral_ramp(&mut records, 0.8, 1., false);
        let z: Vec<[f32; 2]> = records.iter().map(|(s, _)| [s.a[2], s.b[2]]).collect();
        assert_eq!(z.len(), 2);
        assert!((z[0][0] - 0.8).abs() < 1e-6 && (z[0][1] - 0.95).abs() < 1e-6);
        assert!((z[1][0] - 0.95).abs() < 1e-6 && (z[1][1] - 1.).abs() < 1e-6);
        let mut records = layer;
        spiral_ramp(&mut records, 0.8, 1., true);
        assert_eq!(records.len(), 4);
        assert!(
            records[2..]
                .iter()
                .all(|(s, _)| s.a[2] == 1. && s.b[2] == 1.)
        );
    }

    #[test]
    fn disk_batches_and_layer_range() {
        let (job, config, settings) = job();
        let mut b = Builder::new(&job, &config, &settings).unwrap();
        let l = ScheduledLayer::default();
        b.push(&l, &settings).unwrap();
        let p = b.finish(job).unwrap();
        assert_eq!(View::default().range(&p), 0..0);
        assert_eq!(View::default().head_index(&p), None);
        p.read_batches(0..0, |_| panic!("empty")).unwrap();
        assert!(p.segment_at(0).is_err());
    }
    #[test]
    fn disk_seek_and_bounded_uploads() {
        let (job, config, settings) = job();
        let mut b = Builder::new(&job, &config, &settings).unwrap();
        let segment = Segment::new([0.; 3], [1., 0., 0.], [0.4, 0.2], 2, 1, 100., 50.);
        for _ in 0..BATCH + 17 {
            b.file.write_all(bytemuck::bytes_of(&segment)).unwrap();
        }
        b.count = (BATCH + 17) as u64;
        b.layers = vec![
            Layer {
                start: 0,
                count: BATCH as u64,
                z: 0.2,
                time_s: 1.,
            },
            Layer {
                start: BATCH as u64,
                count: 17,
                z: 0.4,
                time_s: 2.,
            },
        ];
        let p = b.finish(job).unwrap();
        let mut sizes = vec![];
        p.read_batches(0..p.count, |batch| {
            sizes.push(batch.len());
            assert_eq!(batch[0].meta, segment.meta);
        })
        .unwrap();
        assert_eq!(sizes, vec![BATCH, 17]);
        let view = View {
            low: 1,
            high: 1,
            moves: 4,
            ..Default::default()
        };
        assert_eq!(view.range(&p), BATCH as u64..BATCH as u64 + 4);
        assert_eq!(view.head_index(&p), Some(BATCH as u64 + 3));
        assert_eq!(p.layer_of(BATCH as u64 + 3), 1);
        assert_eq!(p.layer_of(5), 0);
        assert_eq!(p.segment_at(BATCH as u64).unwrap().meta, segment.meta);
        let mut count = 0;
        p.read_batches(view.range(&p), |batch| count += batch.len())
            .unwrap();
        assert_eq!(count, 4);
        assert_eq!(
            View {
                color: ColorMode::Speed,
                ..Default::default()
            }
            .gradient_range(&p),
            [0., 300.]
        );
    }
}
