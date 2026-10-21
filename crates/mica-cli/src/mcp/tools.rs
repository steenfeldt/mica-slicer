// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The Mica MCP tools that work on a [`Scene`], as plain functions over JSON arguments, so the
//! headless server (`mica mcp` on its own scene) and the open Mica window (live mode, which
//! runs them on a scene synced from the window's project) take exactly the same code path.
//! The rmcp layer (`super::MicaMcp`) only describes the tools and forwards to [`scene_tool`].

use super::colors;
use super::live::Content;
use super::render;
use super::session::{self, Painted, Scene};
use mica_core::fdm_mixer;
use mica_core::full_spectrum::{Component, FullSpectrum, Pattern, VirtualExtruder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The tools [`scene_tool`] runs; the others (`open`, `undo`, `slice_summary`,
/// `save_project`, `status`) differ between the headless scene and the Mica window.
pub const SCENE_TOOLS: &[&str] = &[
    "scene_info",
    "render_view",
    "paint_at",
    "pick",
    "paint_height_band",
    "paint_by_direction",
    "set_part_tool",
    "set_filament_colors",
    "create_mix",
    "clear_paint",
];

/// What the Mica window's undo list calls a scene tool's edit (`None`: the tool only reads).
pub fn edit_label(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "paint_at" | "paint_height_band" | "paint_by_direction" => "paint",
        "set_part_tool" => "part tool",
        "set_filament_colors" => "filament colors",
        "clear_paint" => "clear paint",
        "create_mix" => "create mix",
        _ => return None,
    })
}

/// The `open` tool's arguments.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct OpenArgs {
    /// Path of the .stl or .3mf to open. It is only read, never written.
    pub path: String,
    /// Printer: "U1" (Snapmaker U1, 4 tools), "XL" (Prusa XL 5T, 5 tools), "CORE One"
    /// (1 tool) or an exact preset name. Default: a 3MF's own printer, else CORE One.
    pub printer: Option<String>,
    /// Profile vendor for an exact printer name ("Prusa" or an Orca vendor like "Snapmaker").
    pub vendor: Option<String>,
    /// Print (process) preset name; default: the printer's default.
    pub print: Option<String>,
    /// Filament preset name for all tools; default: the printer's default.
    pub filament: Option<String>,
    /// Orca bed/plate type, e.g. "High Temp Plate".
    pub bed: Option<String>,
    /// Filament colors per tool in slot order, names ("red", "light blue") or "#RRGGBB".
    pub filament_colors: Option<Vec<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RenderArgs {
    /// Named view: front, back, left, right, top, bottom, iso (front-right from above),
    /// iso_front_left, iso_back_left, iso_back_right. Default iso. Overridden by azimuth/elevation.
    pub view: Option<String>,
    /// Degrees around the model: 0 front (from -y), 90 right (from +x), 180 back, 270 left.
    pub azimuth: Option<f32>,
    /// Degrees above the horizon: 0 level, 90 straight down, negative from below.
    pub elevation: Option<f32>,
    /// Image width and height in pixels, 128 to 1024 (default 768).
    pub size: Option<u32>,
    /// Frame only this object (1-based, see scene_info); default all objects.
    pub object: Option<u32>,
    /// Magnification over the framed view (default 1; 2 = twice as large).
    pub zoom: Option<f32>,
    /// Bed point [x, y, z] mm to center on (with zoom, to look closely at a detail).
    pub center: Option<[f32; 3]>,
    /// Wash out unpainted surfaces and outline the borders between tools (default false).
    pub highlight_paint: Option<bool>,
    /// Faint grid every 100 px with pixel coordinates along the edges (default true).
    pub grid: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PaintAtArgs {
    /// view_id returned by render_view (the last few views are kept).
    pub view_id: u32,
    /// Pixel column in that view's image, from the left.
    pub x: u32,
    /// Pixel row in that view's image, from the top.
    pub y: u32,
    /// Tool to paint, 1-based like the printer's slots; 0 erases back to the part's tool.
    pub tool: u32,
    /// smart_fill (default): the surface around the pixel up to edges sharper than `angle`;
    /// connected: the whole connected surface; triangle: the one mesh facet;
    /// sphere: everything within `radius` mm of the point.
    pub mode: Option<String>,
    /// smart_fill: stop at edges where neighbouring facets differ by more than this many
    /// degrees (default 30; lower stops at gentler edges).
    pub angle: Option<f32>,
    /// sphere: radius in mm (default 3).
    pub radius: Option<f32>,
    /// smart_fill and connected: how far the fill may spread from the clicked point,
    /// measured along the surface, in mm (default 0 = no limit). Use it on smooth models,
    /// where a fill otherwise runs over the whole part.
    pub max_distance: Option<f32>,
    /// smart_fill and connected: allow one fill to cover more than 40% of the part (default
    /// false: such a fill is refused with its area and a suggested max_distance).
    pub allow_large: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PickArgs {
    /// view_id returned by render_view.
    pub view_id: u32,
    /// Pixel column, from the left.
    pub x: u32,
    /// Pixel row, from the top.
    pub y: u32,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct HeightBandArgs {
    /// Object, 1-based (may be omitted when there is one).
    pub object: Option<u32>,
    /// Bottom of the band, mm above the bed.
    pub z_min: f32,
    /// Top of the band, mm above the bed.
    pub z_max: f32,
    /// Tool to paint (1-based; 0 erases).
    pub tool: u32,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DirectionArgs {
    /// Object, 1-based (may be omitted when there is one).
    pub object: Option<u32>,
    /// Only this part (1-based); default all model parts.
    pub part: Option<u32>,
    /// up, down, side (any horizontal-facing surface), front, back, left, right, or a
    /// vector "x,y,z".
    pub direction: String,
    /// Largest angle in degrees between a facet's normal and the direction (for side: from
    /// the horizontal). Default 30.
    pub max_angle: Option<f32>,
    /// Tool to paint (1-based; 0 erases).
    pub tool: u32,
    /// Only facets whose centre is at or above this height, mm.
    pub z_min: Option<f32>,
    /// Only facets whose centre is at or below this height, mm.
    pub z_max: Option<f32>,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PartToolArgs {
    /// Object, 1-based (may be omitted when there is one).
    pub object: Option<u32>,
    /// Part, 1-based; omit for the whole object.
    pub part: Option<u32>,
    /// Tool, 1-based.
    pub tool: u32,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClearArgs {
    /// Object, 1-based; omit to clear every object.
    pub object: Option<u32>,
    /// Part, 1-based; omit for all parts.
    pub part: Option<u32>,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ColorsArgs {
    /// Colors per tool in slot order: names ("red", "white", "light blue") or "#RRGGBB";
    /// an empty string keeps that slot's color.
    pub colors: Vec<String>,
}

/// Find a printable mixed color from the loaded physical filaments.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CreateMixArgs {
    /// Target color name, such as orange or light blue, or an RGB hex value.
    pub target: String,
    /// Allowed physical tool numbers (one-based); default all loaded physical tools.
    pub tools: Option<Vec<usize>>,
    /// Maximum components: 2 (default) or 3.
    pub max_components: Option<usize>,
    /// layers (default), outer_wall_first or gradient.
    pub pattern: Option<String>,
    /// Optional name for the mixed color.
    pub name: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SaveArgs {
    /// New .3mf path to write, in the user's folders (`~/` is the home folder). Omit it to
    /// save a new file on the Desktop. Never the opened input file or Mica's own source folder.
    pub path: Option<String>,
    /// "orca" (OrcaSlicer/Snapmaker Orca, Mica) or "prusa" (PrusaSlicer, Mica). Default:
    /// prusa for Prusa printers, orca otherwise.
    pub flavour: Option<String>,
    /// Replace an existing file at `path` (default false).
    pub overwrite: Option<bool>,
    /// Live mode only: with overwrite: true, replace the project file that is open in the
    /// Mica window. Without it, saving over the user's open project is refused.
    pub replace_open_project: Option<bool>,
}

/// The `add_shape` tool's arguments.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AddShapeArgs {
    /// benchy (the original #3DBenchy), cube, cylinder, cone, sphere, torus, pyramid, wedge
    /// (a 45° ramp), tube (hollow cylinder) or disc (40 × 2 mm).
    pub shape: String,
    /// Size in mm: one number (the largest side; proportions kept) or [x, y, z] (the
    /// bounding box). Default: the shape's own size (20 mm for most shapes; torus 30 × 30 ×
    /// 10, disc 40 × 40 × 2, the Benchy about 60 × 31 × 48).
    pub size: Option<ShapeSize>,
    /// Bed point [x, y] mm for the new object's center. Default: the bed's middle, or a free
    /// spot beside the other objects. New objects only.
    pub position: Option<[f32; 2]>,
    /// object (default: a new object), part (printed, joins the object), modifier (its own
    /// settings, starts at 100% infill), negative_volume (a hole), support_enforcer or
    /// support_blocker. Every kind but object joins an existing object.
    pub kind: Option<String>,
    /// The object (1-based, see scene_info) a part or volume joins; may be omitted when there
    /// is one object (or, live, one is selected in the window).
    pub object: Option<u32>,
}

/// `add_shape`'s size: one number or one per axis.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ShapeSize {
    /// The largest side, mm (proportions kept).
    Largest(f32),
    /// [x, y, z] mm.
    Axes(Vec<f32>),
}

/// What `add_shape` asks for: the model, its size and the part kind (`None`: a new object).
pub fn shape_request(
    a: &AddShapeArgs,
) -> Result<
    (
        mica_core::builtin_models::BuiltinModel,
        [f32; 3],
        Option<mica_core::threemf::PartKind>,
    ),
    String,
> {
    use mica_core::builtin_models::BuiltinModel;
    use mica_core::threemf::PartKind;
    let model = BuiltinModel::parse(&a.shape).ok_or_else(|| {
        let names: Vec<&str> = BuiltinModel::ALL.iter().map(|m| m.id()).collect();
        format!("unknown shape '{}'; one of: {}", a.shape, names.join(", "))
    })?;
    let own = model.default_size();
    let largest = |s: f32| {
        let max = own.iter().copied().fold(0f32, f32::max);
        own.map(|v| v * s / max)
    };
    let size = match &a.size {
        None => own,
        Some(ShapeSize::Largest(s)) => largest(*s),
        Some(ShapeSize::Axes(v)) if v.len() == 1 => largest(v[0]),
        Some(ShapeSize::Axes(v)) if v.len() == 3 => [v[0], v[1], v[2]],
        Some(ShapeSize::Axes(other)) => {
            return Err(format!(
                "size is one number or [x, y, z], not {} numbers",
                other.len()
            ));
        }
    };
    if size
        .iter()
        .any(|s| !s.is_finite() || *s < 0.1 || *s > 2000.)
    {
        return Err(format!("every size must be 0.1 to 2000 mm, not {size:?}"));
    }
    let key: String = a
        .kind
        .as_deref()
        .unwrap_or("object")
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let kind = match key.as_str() {
        "object" | "newobject" => None,
        "part" | "modelpart" => Some(PartKind::Model),
        "modifier" => Some(PartKind::Modifier),
        "negativevolume" | "negative" | "hole" => Some(PartKind::NegativeVolume),
        "supportenforcer" | "enforcer" => Some(PartKind::SupportEnforcer),
        "supportblocker" | "blocker" => Some(PartKind::SupportBlocker),
        _ => {
            return Err(format!(
                "unknown kind '{}'; one of: object, part, modifier, negative_volume, \
                 support_enforcer, support_blocker",
                a.kind.as_deref().unwrap_or_default()
            ));
        }
    };
    if kind.is_some() && a.position.is_some() {
        return Err(
            "position is for new objects; a part or volume is placed on its object (move it \
             in Mica afterwards)"
                .into(),
        );
    }
    Ok((model, size, kind))
}

/// Tool arguments from JSON, with a message the AI can act on.
pub fn parse<T: serde::de::DeserializeOwned>(tool: &str, args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| format!("invalid arguments for {tool}: {e}"))
}

/// A JSON result as the pretty-printed text block the AI reads.
pub fn text(v: &Value) -> Content {
    Content::Text {
        text: serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()),
    }
}

fn painted_json(scene: &Scene, p: &Painted, tool: u32, changed: bool) -> Value {
    let pct = scene.object_tool_pct(p.object);
    json!({
        "changed": changed,
        "tool": tool,
        "triangles_painted": p.triangles,
        "area_painted_mm2": (p.area * 10.).round() / 10.,
        "object_area_by_tool_pct": pct
            .iter()
            .take(scene.logical_tools())
            .enumerate()
            .filter(|(_, v)| **v > 0.)
            .map(|(t, v)| json!({"tool": t + 1, "pct": (v * 100.).round() / 100.}))
            .collect::<Vec<_>>(),
        "undo_steps": scene.undo_steps(),
        "next": "render_view again to check the result",
    })
}

/// Runs one of [`SCENE_TOOLS`] on `scene`; `None` when `tool` is not one of them.
pub fn scene_tool(
    scene: &mut Scene,
    tool: &str,
    args: Value,
) -> Option<Result<Vec<Content>, String>> {
    let result = match tool {
        "scene_info" => Ok(vec![text(&scene.info())]),
        "render_view" => parse(tool, args).and_then(|a| render_view(scene, a)),
        "paint_at" => parse(tool, args).and_then(|a| paint_at(scene, a)),
        "pick" => parse::<PickArgs>(tool, args)
            .and_then(|a| Ok(vec![text(&scene.pick(a.view_id, a.x, a.y)?)])),
        "paint_height_band" => parse::<HeightBandArgs>(tool, args).and_then(|a| {
            let label = format!("paint_height_band {}..{} tool {}", a.z_min, a.z_max, a.tool);
            let (p, changed) = scene.edit(&label, |sc| {
                sc.paint_height_band(a.object, a.z_min, a.z_max, a.tool)
            })?;
            Ok(vec![text(&painted_json(scene, &p, a.tool, changed))])
        }),
        "paint_by_direction" => parse::<DirectionArgs>(tool, args).and_then(|a| {
            let label = format!("paint_by_direction {} tool {}", a.direction, a.tool);
            let (p, changed) = scene.edit(&label, |sc| {
                sc.paint_by_direction(
                    a.object,
                    a.part,
                    &a.direction,
                    a.max_angle.unwrap_or(30.),
                    a.tool,
                    a.z_min,
                    a.z_max,
                )
            })?;
            Ok(vec![text(&painted_json(scene, &p, a.tool, changed))])
        }),
        "set_part_tool" => parse::<PartToolArgs>(tool, args).and_then(|a| {
            let (msg, changed) = scene.edit(&format!("set_part_tool {}", a.tool), |sc| {
                sc.set_part_tool(a.object, a.part, a.tool)
            })?;
            Ok(vec![text(
                &json!({"changed": changed, "result": msg, "undo_steps": scene.undo_steps()}),
            )])
        }),
        "set_filament_colors" => parse::<ColorsArgs>(tool, args).and_then(|a| {
            let (msg, changed) = scene.edit("set_filament_colors", |sc| {
                sc.set_filament_colors(&a.colors)
            })?;
            Ok(vec![text(&json!({"changed": changed, "filaments": msg}))])
        }),
        "create_mix" => parse::<CreateMixArgs>(tool, args).and_then(|a| {
            let (mut result, changed) = scene.edit("create_mix", |sc| create_mix(sc, &a))?;
            result["changed"] = json!(changed);
            result["undo_steps"] = json!(scene.undo_steps());
            Ok(vec![text(&result)])
        }),
        "clear_paint" => parse::<ClearArgs>(tool, args).and_then(|a| {
            let (msg, changed) =
                scene.edit("clear_paint", |sc| sc.clear_paint(a.object, a.part))?;
            Ok(vec![text(
                &json!({"changed": changed, "result": msg, "undo_steps": scene.undo_steps()}),
            )])
        }),
        _ => return None,
    };
    Some(result)
}

fn render_view(scene: &mut Scene, a: RenderArgs) -> Result<Vec<Content>, String> {
    let (mut az, mut el) = match a.view.as_deref().map(str::to_ascii_lowercase) {
        None => (45., 30.),
        Some(v) => render::NAMED
            .iter()
            .find(|(n, ..)| *n == v.replace([' ', '-'], "_"))
            .map(|(_, az, el)| (*az, *el))
            .ok_or_else(|| {
                format!(
                    "unknown view '{v}'; use one of {}",
                    render::NAMED
                        .iter()
                        .map(|n| n.0)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?,
    };
    if let Some(v) = a.azimuth {
        az = v;
    }
    if let Some(v) = a.elevation {
        el = v;
    }
    let options = render::Options {
        highlight_paint: a.highlight_paint.unwrap_or(false),
        grid: a.grid.unwrap_or(true),
    };
    let (_, png, info) = scene.render_view(
        az,
        el,
        a.size.unwrap_or(768),
        a.object,
        a.zoom.unwrap_or(1.),
        a.center,
        options,
    )?;
    Ok(vec![
        Content::Image {
            data: render::base64(&png),
            mime: "image/png".into(),
        },
        text(&info),
    ])
}

fn paint_at(scene: &mut Scene, a: PaintAtArgs) -> Result<Vec<Content>, String> {
    let d = session::PaintMode::default();
    let how = session::PaintMode {
        mode: a.mode.unwrap_or(d.mode),
        angle: a.angle.unwrap_or(d.angle),
        radius: a.radius.unwrap_or(d.radius),
        max_distance: a.max_distance.unwrap_or(d.max_distance),
        allow_large: a.allow_large.unwrap_or(d.allow_large),
    };
    let label = format!("paint_at {} tool {}", how.mode, a.tool);
    let ((painted, hit), changed) =
        scene.edit(&label, |sc| sc.paint_at(a.view_id, a.x, a.y, a.tool, &how))?;
    let mut v = painted_json(scene, &painted, a.tool, changed);
    v["hit"] = hit;
    v["mode"] = json!(how.mode);
    if how.max_distance > 0. {
        v["max_distance"] = json!(how.max_distance);
    }
    Ok(vec![text(&v)])
}

/// The flavour argument of `save_project`.
pub fn flavour(arg: Option<&str>) -> Result<Option<mica_core::threemf::Flavour>, String> {
    match arg.map(str::to_ascii_lowercase).as_deref() {
        None => Ok(None),
        Some("orca") => Ok(Some(mica_core::threemf::Flavour::Orca)),
        Some("prusa") => Ok(Some(mica_core::threemf::Flavour::Prusa)),
        Some(other) => Err(format!("flavour '{other}': use orca or prusa")),
    }
}

/// Search positive shares in 5% steps, scoring the engine's snapped printable recipe.
/// Ties keep the first (fewest components, then ascending tool/ratio order).
fn create_mix(scene: &mut Scene, a: &CreateMixArgs) -> Result<Value, String> {
    if !scene.color_mix_enabled {
        return Err("ColorMix is turned off. Enable ColorMix in Settings › Features to create mixed colors.".into());
    }
    let target = colors::parse(&a.target).ok_or_else(|| {
        format!(
            "Unknown target color; use a color name such as orange or light blue. Known names: {}",
            colors::known()
        )
    })?;
    let max = a.max_components.unwrap_or(2);
    if !(2..=3).contains(&max) {
        return Err("max_components must be 2 or 3".into());
    }
    let pattern = Pattern::from_key(a.pattern.as_deref().unwrap_or("layers"))
        .ok_or("pattern must be layers, outer_wall_first or gradient")?;
    let mut tools = a
        .tools
        .clone()
        .unwrap_or_else(|| (1..=scene.tools()).collect());
    tools.sort_unstable();
    tools.dedup();
    if tools.iter().any(|t| *t == 0 || *t > scene.tools()) {
        return Err(format!(
            "tools must contain only loaded physical tool numbers 1..={}",
            scene.tools()
        ));
    }
    if tools.len() < 2 {
        return Err("A mixed color needs at least two loaded physical tools; load another filament and select a multi-tool printer.".into());
    }
    let target_lab = fdm_mixer::rgb_to_lab(target);
    let mut best: Option<(VirtualExtruder, [u8; 3], f64)> = None;
    let mut consider = |selected: &[usize], shares: &[f64]| {
        let mut mix = VirtualExtruder {
            pattern,
            components: selected
                .iter()
                .zip(shares)
                .map(|(t, p)| Component {
                    extruder: *t,
                    ratio: p / 100.,
                })
                .collect(),
            ..Default::default()
        };
        let snapped = mix.snapped_percents();
        for (c, p) in mix.components.iter_mut().zip(snapped) {
            c.ratio = p / 100.;
        }
        if pattern == Pattern::Gradient {
            // Like a new GUI gradient draft: match at the bottom, reversed shares at the top.
            mix.end_ratios = mix.components.iter().rev().map(|c| c.ratio).collect();
        }
        let parts = mix
            .components
            .iter()
            .map(|c| fdm_mixer::Part {
                rgb: scene.colors[c.extruder - 1],
                ratio: c.ratio,
            })
            .collect::<Vec<_>>();
        let rgb = fdm_mixer::mix_rgb(&parts).expect("two or three components");
        let delta = fdm_mixer::delta_e_2000(target_lab, fdm_mixer::rgb_to_lab(rgb));
        if best.as_ref().is_none_or(|(m, _, d)| {
            delta < *d || (delta == *d && mix.components.len() < m.components.len())
        }) {
            best = Some((mix, rgb, delta));
        }
    };
    for (i, &a) in tools.iter().enumerate() {
        for (j, &b) in tools.iter().enumerate().skip(i + 1) {
            for r in 1..20 {
                consider(&[a, b], &[f64::from(r * 5), f64::from(100 - r * 5)]);
            }
            if max == 3 {
                for &c in tools.iter().skip(j + 1) {
                    // Equal thirds fall between the 5% grid points and are a useful exact cycle.
                    consider(&[a, b, c], &[100. / 3.; 3]);
                    for r in 1..19 {
                        for s in 1..20 - r {
                            consider(
                                &[a, b, c],
                                &[
                                    f64::from(r * 5),
                                    f64::from(s * 5),
                                    f64::from((20 - r - s) * 5),
                                ],
                            );
                        }
                    }
                }
            }
        }
    }
    let (mut mix, predicted, delta) = best.ok_or("No printable mixture found")?;
    mix.name = a
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    let spectrum = scene.mixes()?;
    let same = |m: &&VirtualExtruder| {
        m.pattern == mix.pattern
            && m.components.len() == mix.components.len()
            && mix.components.iter().enumerate().all(|(i, c)| {
                m.components.iter().enumerate().any(|(j, other)| {
                    c.extruder == other.extruder
                        && (c.ratio - other.ratio).abs() < 1e-9
                        && (pattern != Pattern::Gradient
                            || (mix.end_ratios[i] - m.end_ratios[j]).abs() < 1e-9)
                })
            })
    };
    let existing = spectrum.extruders().iter().find(same);
    let reused = existing.is_some();
    if let Some(existing) = existing {
        mix = existing.clone();
    } else {
        mix.id = scene.tools().max(spectrum.max_id()) + 1;
        // FullSpectrum validates the engine's one-byte virtual tool limit (255).
        let mut list = spectrum.extruders().to_vec();
        list.push(mix.clone());
        let spectrum = FullSpectrum::new(list, scene.tools())
            .map_err(|e| format!("Cannot create another mixed color: {e}"))?;
        scene.set_mixes(&spectrum)?;
    }
    let quality = if delta < 3. {
        "very close".into()
    } else if delta < 6. {
        "close".into()
    } else if delta < 12. {
        "rough".into()
    } else {
        format!(
            "can't match well — load a filament closer to {}",
            colors::name(target)
        )
    };
    Ok(json!({
        "tool": mix.id, "existing": reused, "name": mix.name,
        "components": mix.components.iter().map(|c| json!({"tool": c.extruder, "ratio": c.ratio})).collect::<Vec<_>>(),
        "pattern": mix.pattern.key(), "end_ratios": mix.end_ratios,
        "predicted_color": {"name": colors::name(predicted), "hex": colors::hex(predicted)},
        "delta_e": delta, "quality": quality,
        "prediction_scope": if pattern == Pattern::Gradient { "bottom of gradient; top reverses component shares" } else { "mixed color" },
        "time_cost": "Mixed colors alternate physical tools per layer; tool changes add time and may add purge and tower filament.",
        "next": "Use this tool number with set_part_tool or any paint tool, then render_view and slice_summary to check appearance and cost. Report colors by name.",
    }))
}
