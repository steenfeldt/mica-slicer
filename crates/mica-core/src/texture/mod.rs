// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Surface textures: which height map, how it is laid onto the part and how deep it goes. The
//! settings (`mica_texture_skin*`), texture sources and projections are independent of walls,
//! so the wall texture ([`crate::texture_skin`]), the Prepare preview and a later mesh
//! displacement for tops share them. Inspired by CNC Kitchen's BumpMesh.
pub mod catalog;
pub mod displace;
pub mod heightmap;
pub mod procedural;
pub mod projection;

use heightmap::HeightMap;
use projection::{Axis, Mapping, Projection};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Which outer walls get the texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    None,
    /// Outer walls of the part's outside only.
    External,
    /// Every outer wall, holes included.
    All,
}

/// Which way the texture moves the surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// White stands out by the depth, black stays on the surface (the part grows).
    #[default]
    Outward,
    /// Black sinks in by the depth, white stays on the surface (the part keeps its size).
    Inward,
    /// Half out, half in, around the surface.
    Both,
}
impl Direction {
    pub fn key(self) -> &'static str {
        match self {
            Self::Outward => "outward",
            Self::Inward => "inward",
            Self::Both => "both",
        }
    }
    /// Displacement (mm, positive outward) for grey `g` (0–1) and depth `depth`.
    pub fn offset(self, g: f64, depth: f64) -> f64 {
        match self {
            Self::Outward => g * depth,
            Self::Inward => (g - 1.) * depth,
            Self::Both => (g - 0.5) * depth,
        }
    }
    /// How far the texture reaches outward and inward, for depth `depth`.
    pub fn reach(self, depth: f64) -> (f64, f64) {
        match self {
            Self::Outward => (depth, 0.),
            Self::Inward => (0., depth),
            Self::Both => (depth / 2., depth / 2.),
        }
    }
}

/// Shape of the ramp from a painted area's edge into it (BumpMesh's boundary falloff curves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Curve {
    Linear,
    SCurve,
    #[default]
    Ease,
}
impl Curve {
    // Ported from bumpmesh@a6d6d6e6df js/displacement.js :: _buildFalloffField (ramp shapes).
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0., 1.);
        match self {
            Self::Linear => t,
            Self::SCurve => t * t * (3. - 2. * t),
            Self::Ease => t * t,
        }
    }
}

/// Which surfaces get the texture: the walls only (moved while slicing, fast), or every
/// surface (the mesh is displaced before slicing, [`displace`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Surfaces {
    #[default]
    Walls,
    WallsAndTops,
}

/// Where the height map comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A bundled image (`builtin:<id>`, [`catalog::BUILTINS`]).
    Builtin(String),
    /// One of Mica's generated patterns (`procedural:<id>`, [`procedural::ALL`]).
    Procedural(String),
    /// An image stored in the project, by the SHA-256 of its normalised PNG (`sha256:<hex>`).
    Custom(String),
}
impl Source {
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || format!("unknown texture “{s}”");
        let (kind, id) = s.split_once(':').ok_or_else(bad)?;
        match kind {
            "builtin" if catalog::builtin(id).is_some() => Ok(Self::Builtin(id.into())),
            "procedural" if procedural::by_id(id).is_some() => Ok(Self::Procedural(id.into())),
            "sha256" if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
                Ok(Self::Custom(id.to_ascii_lowercase()))
            }
            _ => Err(bad()),
        }
    }
    pub fn key(&self) -> String {
        match self {
            Self::Builtin(id) => format!("builtin:{id}"),
            Self::Procedural(id) => format!("procedural:{id}"),
            Self::Custom(hash) => format!("sha256:{hash}"),
        }
    }
    /// The name shown in the GUI.
    pub fn name(&self) -> String {
        match self {
            Self::Builtin(id) => catalog::builtin(id).map_or(id.clone(), |b| b.name.into()),
            Self::Procedural(id) => procedural::by_id(id).map_or(id.clone(), |p| p.name.into()),
            Self::Custom(hash) => format!("Custom image {}", &hash[..8.min(hash.len())]),
        }
    }
}

/// Every `mica_texture_skin*` option, by its suffix after `mica_texture_skin`.
pub const KEYS: &[&str] = &[
    "mica_texture_skin",
    "mica_texture_skin_texture",
    "mica_texture_skin_projection",
    "mica_texture_skin_scale",
    "mica_texture_skin_scale_v",
    "mica_texture_skin_offset_u",
    "mica_texture_skin_offset_v",
    "mica_texture_skin_rotation",
    "mica_texture_skin_amplitude",
    "mica_texture_skin_direction",
    "mica_texture_skin_invert",
    "mica_texture_skin_smoothing",
    "mica_texture_skin_blend",
    "mica_texture_skin_cylinder_axis",
    "mica_texture_skin_cap_angle",
    "mica_texture_skin_top_angle",
    "mica_texture_skin_bottom_angle",
    "mica_texture_skin_overhang_clamp",
    "mica_texture_skin_first_layer",
    "mica_texture_skin_point_dist",
    "mica_texture_skin_pattern_ratio",
    "mica_texture_skin_pattern_softness",
    "mica_texture_skin_falloff",
    "mica_texture_skin_falloff_curve",
    "mica_texture_skin_surfaces",
    "mica_texture_paint_slots",
];
/// The deepest texture Mica accepts, mm.
pub const MAX_DEPTH: f64 = 2.;

/// Resolved texture settings of one region (or one painted slot).
#[derive(Clone, Debug, PartialEq)]
pub struct TextureSettings {
    pub mode: Mode,
    pub source: Source,
    pub mapping: Mapping,
    /// Depth, mm.
    pub amplitude: f64,
    pub direction: Direction,
    pub invert: bool,
    /// Blur, px of a 512 px map.
    pub smoothing: f64,
    /// No texture on surfaces flatter than these angles (degrees above horizontal), facing up
    /// and facing down; 0 turns the limit off.
    pub top_angle: f64,
    pub bottom_angle: f64,
    /// Less outward texture on overhangs, so it adds no overhang of its own.
    pub overhang_clamp: bool,
    pub first_layer: bool,
    /// Distance between the textured wall's points, mm.
    pub point_distance: f64,
    pub pattern: procedural::Params,
    /// Width of the soft border of painted areas, mm (0: hard edge).
    pub falloff: f64,
    pub falloff_curve: Curve,
    pub surfaces: Surfaces,
    /// Per painted slot (state 1..): option overrides on top of these settings.
    pub slots: BTreeMap<u8, BTreeMap<String, String>>,
    raw: BTreeMap<String, String>,
}

impl Default for TextureSettings {
    fn default() -> Self {
        Self::from_lookup(|_| None).expect("texture defaults are valid")
    }
}

/// The paint slots option (`1:texture=builtin:bark,scale=6;2:…`) as overrides per slot.
pub fn parse_slots(s: &str) -> Result<BTreeMap<u8, BTreeMap<String, String>>, String> {
    let mut out = BTreeMap::new();
    for slot in s.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (n, body) = slot.split_once(':').unwrap_or((slot, ""));
        let n: u8 = n
            .trim()
            .parse()
            .ok()
            .filter(|n| (1..=15).contains(n))
            .ok_or_else(|| format!("texture paint slot “{n}” is not 1 to 15"))?;
        let mut map = BTreeMap::new();
        for kv in body.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| format!("texture paint slot {n}: expected key=value, got “{kv}”"))?;
            let key = format!("mica_texture_skin_{}", k.trim());
            if !KEYS.contains(&key.as_str()) || key == "mica_texture_paint_slots" {
                return Err(format!("texture paint slot {n}: unknown option “{k}”"));
            }
            map.insert(key, v.trim().to_string());
        }
        out.insert(n, map);
    }
    Ok(out)
}

/// The slot list in its option form (`1:texture=builtin:bark,scale=6;2:…`).
pub fn format_slots(slots: &BTreeMap<u8, BTreeMap<String, String>>) -> String {
    slots
        .iter()
        .map(|(n, map)| {
            let body = map
                .iter()
                .map(|(k, v)| format!("{}={v}", k.strip_prefix("mica_texture_skin_").unwrap_or(k)))
                .collect::<Vec<_>>()
                .join(",");
            format!("{n}:{body}")
        })
        .collect::<Vec<_>>()
        .join(";")
}

impl TextureSettings {
    pub fn from_config(c: &crate::config::Config) -> Result<Self, crate::resolve::ResolveError> {
        Self::from_lookup(|k| c.raw(k)).map_err(crate::resolve::ResolveError)
    }

    /// Reads every texture option through `get` (raw text form; `None` for the default).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        // Values equal to the option's default are left out, so equal settings compare equal
        // however they were written.
        let defs = crate::config::option_defs();
        let raw: BTreeMap<String, String> = KEYS
            .iter()
            .filter_map(|k| get(k).map(|v| (k.to_string(), v)))
            .filter(|(k, v)| defs.get(k.as_str()).is_none_or(|d| d.default != v.as_str()))
            .collect();
        let s = Self::from_raw(raw)?;
        for n in s.slots.keys() {
            s.slot(*n)?;
        }
        Ok(s)
    }

    fn from_raw(raw: BTreeMap<String, String>) -> Result<Self, String> {
        let text = |k: &str, d: &'static str| raw.get(k).map_or(d, String::as_str).to_string();
        let float = |k: &str, d: f64| -> Result<f64, String> {
            match raw.get(k).map(|v| v.trim()) {
                None | Some("") => Ok(d),
                Some(v) => v
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("{k}: “{v}” is not a number")),
            }
        };
        let flag = |k: &str, d: bool| match raw.get(k).map(|v| v.trim()) {
            None | Some("") => Ok(d),
            Some("1" | "true") => Ok(true),
            Some("0" | "false") => Ok(false),
            Some(v) => Err(format!("{k}: “{v}” is not 0 or 1")),
        };
        let enumerated = |k: &str, v: &str| format!("unsupported {k}: {v}");
        let mode = match text("mica_texture_skin", "none").as_str() {
            "none" | "" => Mode::None,
            "external" => Mode::External,
            "all" => Mode::All,
            v => return Err(enumerated("mica_texture_skin", v)),
        };
        let source = Source::parse(&text(
            "mica_texture_skin_texture",
            "procedural:knurl_diamond",
        ))?;
        let projection_text = text("mica_texture_skin_projection", "triplanar");
        let projection = Projection::parse(&projection_text)
            .ok_or_else(|| enumerated("mica_texture_skin_projection", &projection_text))?;
        let axis_text = text("mica_texture_skin_cylinder_axis", "z");
        let axis = Axis::parse(&axis_text)
            .ok_or_else(|| enumerated("mica_texture_skin_cylinder_axis", &axis_text))?;
        let direction = match text("mica_texture_skin_direction", "outward").as_str() {
            "outward" => Direction::Outward,
            "inward" => Direction::Inward,
            "both" => Direction::Both,
            v => return Err(enumerated("mica_texture_skin_direction", v)),
        };
        let falloff_curve = match text("mica_texture_skin_falloff_curve", "ease").as_str() {
            "linear" => Curve::Linear,
            "scurve" => Curve::SCurve,
            "ease" => Curve::Ease,
            v => return Err(enumerated("mica_texture_skin_falloff_curve", v)),
        };
        let surfaces = match text("mica_texture_skin_surfaces", "walls").as_str() {
            "walls" => Surfaces::Walls,
            "walls_and_tops" => Surfaces::WallsAndTops,
            v => return Err(enumerated("mica_texture_skin_surfaces", v)),
        };
        let mapping = Mapping {
            projection,
            scale_u: float("mica_texture_skin_scale", 3.)?,
            scale_v: float("mica_texture_skin_scale_v", 0.)?,
            offset_u: float("mica_texture_skin_offset_u", 0.)?,
            offset_v: float("mica_texture_skin_offset_v", 0.)?,
            rotation: float("mica_texture_skin_rotation", 0.)?,
            blend: float("mica_texture_skin_blend", 1.)?,
            cap_angle: float("mica_texture_skin_cap_angle", 20.)?,
            axis,
        };
        let s = Self {
            mode,
            source,
            mapping,
            amplitude: float("mica_texture_skin_amplitude", 0.4)?,
            direction,
            invert: flag("mica_texture_skin_invert", false)?,
            smoothing: float("mica_texture_skin_smoothing", 0.)?,
            top_angle: float("mica_texture_skin_top_angle", 10.)?,
            bottom_angle: float("mica_texture_skin_bottom_angle", 10.)?,
            overhang_clamp: flag("mica_texture_skin_overhang_clamp", true)?,
            first_layer: flag("mica_texture_skin_first_layer", false)?,
            point_distance: float("mica_texture_skin_point_dist", 0.25)?,
            pattern: procedural::Params {
                ratio: float("mica_texture_skin_pattern_ratio", 0.5)?,
                softness: float("mica_texture_skin_pattern_softness", 0.3)?,
            },
            falloff: float("mica_texture_skin_falloff", 1.)?,
            falloff_curve,
            surfaces,
            slots: parse_slots(&text("mica_texture_paint_slots", ""))?,
            raw,
        };
        let m = &s.mapping;
        let checks: [(bool, &str); 10] = [
            (
                (0. ..=MAX_DEPTH).contains(&s.amplitude),
                "texture depth must be 0 to 2 mm",
            ),
            (
                m.scale_u >= 0.1 && m.scale_u <= 1000.,
                "texture tile size must be 0.1 to 1000 mm",
            ),
            (
                m.scale_v == 0. || (0.1..=1000.).contains(&m.scale_v),
                "texture tile height must be 0 (keep proportions) or 0.1 to 1000 mm",
            ),
            (
                (0.05..=5.).contains(&s.point_distance),
                "texture point distance must be 0.05 to 5 mm",
            ),
            (
                (0. ..=50.).contains(&s.smoothing),
                "texture smoothing must be 0 to 50",
            ),
            (
                (0. ..=90.).contains(&s.top_angle) && (0. ..=90.).contains(&s.bottom_angle),
                "texture angle limits must be 0° to 90°",
            ),
            (
                (0. ..=1.).contains(&m.blend),
                "texture projection blend must be 0 to 1",
            ),
            (
                (0. ..=90.).contains(&m.cap_angle),
                "texture cap angle must be 0° to 90°",
            ),
            (
                (0. ..=1.).contains(&s.pattern.ratio) && (0. ..=1.).contains(&s.pattern.softness),
                "texture pattern values must be 0 to 1",
            ),
            (
                (0. ..=50.).contains(&s.falloff),
                "texture border falloff must be 0 to 50 mm",
            ),
        ];
        if let Some((_, why)) = checks.iter().find(|(ok, _)| !ok) {
            return Err((*why).into());
        }
        if let Source::Custom(hash) = &s.source
            && custom_png(hash).is_none()
            && (s.mode != Mode::None || !s.slots.is_empty())
        {
            return Err(format!(
                "the texture image sha256:{}… is not in this project",
                &hash[..12]
            ));
        }
        Ok(s)
    }

    /// Whether any wall of this region can get a texture (from its own settings or from paint).
    pub fn enabled(&self) -> bool {
        self.mode != Mode::None
    }

    /// The settings of painted slot `n`: these settings with the slot's overrides, applied to
    /// every outer wall the paint covers.
    pub fn slot(&self, n: u8) -> Result<Self, String> {
        let mut raw = self.raw.clone();
        raw.remove("mica_texture_paint_slots");
        if let Some(over) = self.slots.get(&n) {
            raw.extend(over.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        raw.insert("mica_texture_skin".into(), "all".into());
        let mut s = Self::from_raw(raw).map_err(|e| format!("texture paint slot {n}: {e}"))?;
        s.slots.clear();
        Ok(s)
    }

    /// The height map, smoothed and inverted as set, shared through a process-wide cache.
    /// The blur the map gets (px of a 512 px map): the Smoothing setting, but at least what
    /// keeps detail finer than the wall's point distance from aliasing (detail the nozzle
    /// cannot print either). Rounded to quarter pixels, so nearby tile sizes share a map.
    pub fn effective_smoothing(&self) -> f64 {
        let texel = self.mapping.scale_u.max(0.1) / 512.;
        let automatic = 0.35 * self.point_distance / texel;
        (self.smoothing.max(automatic) * 4.).round() / 4.
    }

    /// What the height map depends on (source, smoothing, inversion, pattern parameters).
    pub fn map_key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.source.key(),
            self.effective_smoothing(),
            self.invert,
            self.pattern.ratio,
            self.pattern.softness
        )
    }

    pub fn map(&self) -> Result<Arc<HeightMap>, String> {
        let key = self.map_key();
        static CACHE: OnceLock<Mutex<HashMap<String, Arc<HeightMap>>>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(m) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(m.clone());
        }
        let mut map = match &self.source {
            Source::Builtin(id) => {
                let b = catalog::builtin(id).ok_or("unknown bundled texture")?;
                HeightMap::from_png(b.png)?
            }
            Source::Procedural(id) => {
                let info = procedural::by_id(id).ok_or("unknown procedural texture")?;
                procedural::generate(info, self.pattern)
            }
            Source::Custom(hash) => {
                let png = custom_png(hash).ok_or_else(|| {
                    format!(
                        "the texture image sha256:{}… is not in this project",
                        &hash[..12]
                    )
                })?;
                HeightMap::from_png(&png)?
            }
        };
        let smoothing = self.effective_smoothing();
        if smoothing > 0. {
            map = map.smoothed(smoothing);
        }
        if self.invert {
            map = map.inverted();
        }
        let map = Arc::new(map);
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(key, map.clone());
        Ok(map)
    }

    /// Every non-default texture option, as text (for cache keys).
    pub fn fingerprint(&self) -> String {
        self.raw.iter().map(|(k, v)| format!("{k}={v};")).collect()
    }

    /// The custom images these settings (and their paint slots) refer to.
    pub fn custom_images(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Source::Custom(h) = &self.source {
            out.push(h.clone());
        }
        for over in self.slots.values() {
            if let Some(Ok(Source::Custom(h))) = over
                .get("mica_texture_skin_texture")
                .map(|t| Source::parse(t))
            {
                out.push(h);
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Custom images of every loaded project and every import, by content hash. Content addressing
/// keeps this process-wide store deterministic: a hash names exactly one image.
fn custom_store() -> &'static RwLock<HashMap<String, Arc<Vec<u8>>>> {
    static STORE: OnceLock<RwLock<HashMap<String, Arc<Vec<u8>>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Adds a height map as a custom texture: limited to [`heightmap::CUSTOM_MAX`] px, stored as an
/// 8-bit grey PNG. Returns the setting value (`sha256:<hex>`).
pub fn register_map(map: &HeightMap) -> String {
    let png = map.fit(heightmap::CUSTOM_MAX).to_png();
    let hash = sha256_hex(&png);
    custom_store()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(hash.clone())
        .or_insert_with(|| Arc::new(png));
    format!("sha256:{hash}")
}

/// Adds any PNG as a custom texture (colour images become their brightness).
pub fn register_png(bytes: &[u8]) -> Result<String, String> {
    Ok(register_map(&HeightMap::from_png(bytes)?))
}

/// Adds an image read from a project, stored under the hash of its bytes. Returns the hash.
pub fn register_stored(png: Vec<u8>) -> Result<String, String> {
    HeightMap::from_png(&png)?;
    let hash = sha256_hex(&png);
    custom_store()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(hash.clone())
        .or_insert_with(|| Arc::new(png));
    Ok(hash)
}

/// The stored PNG of a custom texture.
pub fn custom_png(hash: &str) -> Option<Arc<Vec<u8>>> {
    custom_store()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(hash)
        .cloned()
}

/// Every custom texture known to this process (loaded projects and imports), sorted.
pub fn custom_hashes() -> Vec<String> {
    let mut out: Vec<String> = custom_store()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect();
    out.sort();
    out
}

/// The archive path of a custom texture inside a project 3MF.
pub fn archive_path(hash: &str) -> String {
    format!("Metadata/Mica/textures/{hash}.png")
}

/// Custom image hashes named by texture options in `settings` (option name → value maps).
pub fn custom_images_in<'a>(settings: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in settings {
        if k == "mica_texture_skin_texture"
            && let Ok(Source::Custom(h)) = Source::parse(v)
        {
            out.push(h);
        } else if k == "mica_texture_paint_slots"
            && let Ok(slots) = parse_slots(v)
        {
            for over in slots.values() {
                if let Some(Ok(Source::Custom(h))) = over
                    .get("mica_texture_skin_texture")
                    .map(|t| Source::parse(t))
                {
                    out.push(h);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_valid() {
        let s = TextureSettings::default();
        assert_eq!(s.mode, Mode::None);
        assert_eq!(s.direction, Direction::Outward);
        let c = crate::config::Config::defaults();
        assert_eq!(TextureSettings::from_config(&c).unwrap(), s);
    }

    #[test]
    fn direction_offsets() {
        assert_eq!(Direction::Outward.offset(1., 0.4), 0.4);
        assert_eq!(Direction::Outward.offset(0., 0.4), 0.);
        assert_eq!(Direction::Inward.offset(1., 0.4), 0.);
        assert!((Direction::Inward.offset(0., 0.4) + 0.4).abs() < 1e-12);
        assert!((Direction::Both.offset(1., 0.4) - 0.2).abs() < 1e-12);
        assert_eq!(Direction::Both.reach(0.4), (0.2, 0.2));
    }

    #[test]
    fn slots_override_the_region_settings() {
        let s = TextureSettings::from_lookup(|k| match k {
            "mica_texture_paint_slots" => {
                Some("1:texture=builtin:bark,scale=6;3:amplitude=0.2".into())
            }
            "mica_texture_skin_amplitude" => Some("0.5".into()),
            _ => None,
        })
        .unwrap();
        let one = s.slot(1).unwrap();
        assert_eq!(one.source, Source::Builtin("bark".into()));
        assert_eq!(one.mapping.scale_u, 6.);
        assert_eq!(one.amplitude, 0.5);
        assert_eq!(one.mode, Mode::All);
        assert_eq!(s.slot(3).unwrap().amplitude, 0.2);
        // A slot without overrides uses the region's texture.
        assert_eq!(s.slot(2).unwrap().source, s.source);
        assert_eq!(
            format_slots(&s.slots),
            "1:scale=6,texture=builtin:bark;3:amplitude=0.2"
        );
        assert!(
            TextureSettings::from_lookup(
                |k| (k == "mica_texture_paint_slots").then(|| "1:nonsense=2".into())
            )
            .is_err()
        );
    }

    #[test]
    fn missing_custom_images_are_an_error_only_when_used() {
        let hash = "ab".repeat(32);
        let get = |mode: &'static str| {
            let hash = hash.clone();
            move |k: &str| match k {
                "mica_texture_skin_texture" => Some(format!("sha256:{hash}")),
                "mica_texture_skin" => Some(mode.to_string()),
                _ => None,
            }
        };
        assert!(TextureSettings::from_lookup(get("none")).is_ok());
        assert!(TextureSettings::from_lookup(get("external")).is_err());
    }

    #[test]
    fn registered_images_resolve_by_hash() {
        let map = HeightMap::from_gray8(4, 2, &[0, 40, 80, 120, 160, 200, 240, 255]).unwrap();
        let value = register_map(&map);
        let s = TextureSettings::from_lookup(|k| match k {
            "mica_texture_skin_texture" => Some(value.clone()),
            "mica_texture_skin" => Some("external".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(s.map().unwrap().to_gray8(), map.to_gray8());
        assert_eq!(s.custom_images().len(), 1);
        assert_eq!(
            custom_images_in([("mica_texture_skin_texture", value.as_str())]),
            s.custom_images()
        );
    }
}
