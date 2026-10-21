// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Surface textures in the GUI: the texture row of the Texture settings, the texture gallery
//! (bundled, procedural and imported textures with search and favorites), Texture painting's
//! slots, thumbnails, and the Textures feature switch. The engine side is
//! `mica_core::texture` and `mica_core::texture_skin`. Inspired by CNC Kitchen's BumpMesh.

use crate::app::App;
use crate::panels::widgets;
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use mica_core::texture::{self, Source, catalog::Category, heightmap::HeightMap, procedural};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

pub const TEXTURE_KEY: &str = "mica_texture_skin_texture";
pub const SLOTS_KEY: &str = "mica_texture_paint_slots";
/// Texture paint slots offered in the paint card.
pub const SLOTS: u8 = 4;

/// One texture the gallery offers.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The setting value (`builtin:…`, `procedural:…`, `sha256:…`).
    pub value: String,
    pub name: String,
    pub category: Category,
    pub credit: Option<String>,
    /// Suggested tile size, mm.
    pub tile: f64,
}

/// Every texture: Mica's patterns, the bundled maps and the custom images known so far.
pub fn entries() -> Vec<Entry> {
    let mut out: Vec<Entry> = procedural::ALL
        .iter()
        .map(|p| Entry {
            value: format!("procedural:{}", p.id),
            name: p.name.into(),
            category: Category::Procedural,
            credit: Some("Made by Mica from a few parameters (Pattern settings)".into()),
            tile: p.tile,
        })
        .collect();
    out.extend(texture::catalog::BUILTINS.iter().map(|b| Entry {
        value: format!("builtin:{}", b.id),
        name: b.name.into(),
        category: b.category(),
        credit: Some(b.credit_line()),
        tile: b.tile,
    }));
    out.extend(texture::custom_hashes().into_iter().map(|h| Entry {
        name: Source::Custom(h.clone()).name(),
        value: format!("sha256:{h}"),
        category: Category::Custom,
        credit: Some("Your image, stored in the project".into()),
        tile: 10.,
    }));
    out
}

/// The name shown for a texture setting value.
pub fn display_name(value: &str) -> String {
    Source::parse(value).map_or_else(|_| value.to_string(), |s| s.name())
}

/// The grey map behind a setting value, at its default pattern parameters.
fn map_of(value: &str) -> Option<HeightMap> {
    match Source::parse(value).ok()? {
        Source::Builtin(id) => HeightMap::from_png(texture::catalog::builtin(&id)?.png).ok(),
        Source::Procedural(id) => Some(procedural::generate(
            procedural::by_id(&id)?,
            procedural::Params::default(),
        )),
        Source::Custom(hash) => HeightMap::from_png(&texture::custom_png(&hash)?).ok(),
    }
}

/// A square thumbnail of a height map, lit from the top left so the relief reads.
pub fn thumbnail_image(map: &HeightMap, size: usize) -> egui::ColorImage {
    let small = map.fit(size * 2);
    let (w, h) = (small.width(), small.height());
    let px = small.pixels();
    let at = |x: usize, y: usize| px[(y % h) * w + (x % w)];
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            // Two repeats each way, so the thumbnail shows how the texture tiles.
            let (sx, sy) = ((x * 2 * w / size) % w, (y * 2 * h / size) % h);
            let v = at(sx, sy);
            let dx = at(sx + 1, sy) - at(sx + w - 1, sy);
            let dy = at(sx, sy + 1) - at(sx, sy + h - 1);
            let shade = (0.22 + 0.62 * v - 1.6 * (dx + dy)).clamp(0., 1.);
            let c = (shade * 255.) as u8;
            rgba.extend_from_slice(&[
                c,
                (f32::from(c) * 0.97) as u8,
                (f32::from(c) * 0.92) as u8,
                255,
            ]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([size, size], &rgba)
}

/// Thumbnails by setting value; a few are made per frame so the gallery opens at once.
fn thumbnails() -> &'static Mutex<HashMap<String, egui::TextureHandle>> {
    static T: OnceLock<Mutex<HashMap<String, egui::TextureHandle>>> = OnceLock::new();
    T.get_or_init(Default::default)
}
const THUMB: usize = 96;
pub fn thumbnail(ctx: &egui::Context, value: &str) -> Option<egui::TextureHandle> {
    let mut cache = thumbnails().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = cache.get(value) {
        return Some(t.clone());
    }
    let budget_id = egui::Id::unique("mica-texture-thumb-budget");
    let frame = ctx.cumulative_frame_nr();
    let (seen, made) = ctx
        .data(|d| d.get_temp::<(u64, u32)>(budget_id))
        .unwrap_or((frame, 0));
    let made = if seen == frame { made } else { 0 };
    if made >= 4 {
        ctx.request_repaint();
        return None;
    }
    ctx.data_mut(|d| d.insert_temp(budget_id, (frame, made + 1)));
    let map = map_of(value)?;
    let t = ctx.load_texture(
        format!("mica-texture-{value}"),
        thumbnail_image(&map, THUMB),
        egui::TextureOptions::LINEAR,
    );
    cache.insert(value.to_string(), t.clone());
    Some(t)
}

fn paint_thumb(ui: &egui::Ui, rect: Rect, value: &str) {
    let p = theme::of(ui);
    match thumbnail(ui.ctx(), value) {
        Some(t) => {
            ui.painter().image(
                t.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                Color32::WHITE,
            );
        }
        None => {
            ui.painter().rect_filled(rect, 3., p.raised);
        }
    }
}

/// What the gallery is open for: which control gets the pick.
#[derive(Clone, Debug, Default)]
struct GalleryState {
    open: bool,
    target: Option<egui::Id>,
    current: String,
    search: String,
    filter: Filter,
    import_error: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Favorites,
    Category(Category),
}
fn gallery_id() -> egui::Id {
    egui::Id::unique("mica-texture-gallery")
}

/// Opens the gallery for the control `target` (which receives the pick).
pub fn open_gallery(ctx: &egui::Context, target: egui::Id, current: &str) {
    let mut st: GalleryState = ctx.data(|d| d.get_temp(gallery_id())).unwrap_or_default();
    st.open = true;
    st.target = Some(target);
    st.current = current.to_string();
    st.import_error = None;
    ctx.data_mut(|d| d.insert_temp(gallery_id(), st));
}

/// The texture row: a thumbnail and the texture's name; a click opens the gallery. Returns the
/// picked texture and its suggested tile size.
pub fn picker(
    ui: &mut egui::Ui,
    id: egui::Id,
    rect: Rect,
    raw: &str,
    enabled: bool,
) -> Option<(String, f64)> {
    let picked = ui.data_mut(|d| d.remove_temp::<(String, f64)>(id.with("picked")));
    let p = theme::of(ui);
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if response.hovered() && enabled {
        p.hover
    } else {
        p.field
    };
    ui.painter().rect_filled(rect, 5., fill);
    ui.painter().rect_stroke(
        rect,
        5.,
        Stroke::new(1., p.field_border),
        egui::StrokeKind::Inside,
    );
    let thumb = Rect::from_min_size(
        rect.left_top() + Vec2::splat(3.),
        Vec2::splat(rect.height() - 6.),
    );
    paint_thumb(ui, thumb, raw);
    // The name stops short of the "…" at the right edge.
    let name_clip = Rect::from_min_max(rect.min, Pos2::new(rect.right() - 20., rect.bottom()));
    ui.painter().with_clip_rect(name_clip).text(
        Pos2::new(thumb.right() + 8., rect.center().y),
        Align2::LEFT_CENTER,
        display_name(raw),
        theme::sans(13.),
        if enabled { p.text } else { p.muted },
    );
    ui.painter().text(
        Pos2::new(rect.right() - 8., rect.center().y),
        Align2::RIGHT_CENTER,
        "…",
        theme::sans(13.),
        p.muted,
    );
    if response
        .on_hover_text("Choose a texture: Mica's patterns, bundled maps or your own image")
        .clicked()
    {
        open_gallery(ui.ctx(), id, raw);
    }
    picked
}

impl App {
    /// The texture gallery window, while a texture row opened it.
    pub(crate) fn texture_gallery(&mut self, ctx: &egui::Context) {
        let mut st: GalleryState = ctx.data(|d| d.get_temp(gallery_id())).unwrap_or_default();
        if !st.open {
            return;
        }
        let mut open = true;
        let mut pick: Option<(String, f64)> = None;
        let mut star: Option<String> = None;
        let mut import = false;
        let favorites = self.prefs.texture_favorites.clone();
        egui::Window::new("Textures")
            .id(egui::Id::unique("mica-texture-gallery-window"))
            .open(&mut open)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(-320., 60.))
            .fixed_size([640., 560.])
            .collapsible(false)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                widgets::search_field(
                    ui,
                    egui::Id::unique("mica-texture-search"),
                    &mut st.search,
                    "Search textures",
                    "",
                );
                ui.add_space(6.);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6., 6.);
                    let mut filters = vec![
                        (Filter::All, None, "All".to_string()),
                        (Filter::Favorites, Some(widgets::Glyph::Star), "Favorites".to_string()),
                    ];
                    filters.extend(
                        Category::ALL
                            .iter()
                            .map(|&c| (Filter::Category(c), None, c.label().to_string())),
                    );
                    for (f, glyph, label) in filters {
                        if widgets::chip(ui, glyph, &label, st.filter == f, true).clicked() {
                            st.filter = f;
                        }
                    }
                    if widgets::chip(ui, Some(widgets::Glyph::Plus), "Import image…", false, true)
                        .on_hover_text("A grayscale PNG or JPG: white stands out, black stays low. It is stored in the project.")
                        .clicked()
                    {
                        import = true;
                    }
                });
                if let Some(e) = &st.import_error {
                    ui.label(egui::RichText::new(e).font(theme::sans(12.)).color(p.warn_text));
                }
                ui.add_space(6.);
                let query = st.search.to_lowercase();
                let shown: Vec<Entry> = entries()
                    .into_iter()
                    .filter(|e| match st.filter {
                        Filter::All => true,
                        Filter::Favorites => favorites.contains(&e.value),
                        Filter::Category(c) => e.category == c,
                    })
                    .filter(|e| query.is_empty() || e.name.to_lowercase().contains(&query))
                    .collect();
                egui::ScrollArea::vertical()
                    .max_height(420.)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if shown.is_empty() {
                            ui.label(
                                egui::RichText::new(match st.filter {
                                    Filter::Favorites => "No favorites yet: star a texture to keep it here.",
                                    Filter::Category(Category::Custom) => "No images yet: Import image… adds one.",
                                    _ => "No texture matches.",
                                })
                                .font(theme::sans(13.))
                                .color(p.muted),
                            );
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8., 8.);
                            for e in &shown {
                                let (rect, r) = ui.allocate_exact_size(Vec2::new(112., 132.), Sense::click());
                                let selected = e.value == st.current;
                                ui.painter().rect_filled(rect, 7., if r.hovered() { p.hover } else { p.raised });
                                ui.painter().rect_stroke(
                                    rect,
                                    7.,
                                    Stroke::new(if selected { 2. } else { 1. }, if selected { p.accent } else { p.border_raised }),
                                    egui::StrokeKind::Inside,
                                );
                                let thumb = Rect::from_min_size(rect.left_top() + Vec2::new(8., 8.), Vec2::splat(96.));
                                paint_thumb(ui, thumb, &e.value);
                                ui.painter().text(
                                    Pos2::new(rect.center().x, rect.bottom() - 14.),
                                    Align2::CENTER_CENTER,
                                    &e.name,
                                    theme::sans(11.5),
                                    p.text,
                                );
                                // Favorite star in the thumbnail's corner.
                                let star_rect = Rect::from_min_size(thumb.right_top() + Vec2::new(-22., 2.), Vec2::splat(20.));
                                let fav = favorites.contains(&e.value);
                                let sr = ui.interact(star_rect, egui::Id::unique(("mica-texture-star", &e.value)), Sense::click());
                                if fav || r.hovered() || sr.hovered() {
                                    ui.painter().circle_filled(star_rect.center(), 9., Color32::from_black_alpha(140));
                                    widgets::glyph(
                                        ui.painter(),
                                        star_rect.shrink(3.),
                                        widgets::Glyph::Star,
                                        if fav { p.accent_hi } else { Color32::WHITE },
                                    );
                                }
                                if sr.on_hover_text(if fav { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                                    star = Some(e.value.clone());
                                } else if r
                                    .on_hover_text(format!(
                                        "{}\nSuggested tile: {} mm\n{}",
                                        e.name,
                                        e.tile,
                                        e.credit.clone().unwrap_or_default()
                                    ))
                                    .clicked()
                                {
                                    pick = Some((e.value.clone(), e.tile));
                                }
                            }
                        });
                    });
                ui.add_space(6.);
                ui.label(
                    egui::RichText::new(
                        "Patterns: Hero Patterns by Steve Schoger (CC BY 4.0). CC0 maps from ambientCG and Poly Haven. \
                         Maps prepared by CNC Kitchen for BumpMesh, which inspired Mica's textures.",
                    )
                    .font(theme::sans(11.))
                    .color(p.muted),
                );
            });
        if import
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg"])
                .set_title("Choose a texture image")
                .pick_file()
        {
            match import_image(&path) {
                Ok(value) => {
                    st.filter = Filter::Category(Category::Custom);
                    st.import_error = None;
                    pick = Some((value, 10.));
                }
                Err(e) => st.import_error = Some(format!("{}: {e}", path.display())),
            }
        }
        if let Some(v) = star {
            let favs = &mut self.prefs.texture_favorites;
            if let Some(i) = favs.iter().position(|f| *f == v) {
                favs.remove(i);
            } else {
                favs.push(v);
            }
            self.prefs_dirty = true;
        }
        if let Some(pick) = pick {
            if let Some(target) = st.target {
                ctx.data_mut(|d| d.insert_temp(target.with("picked"), pick));
            }
            st.open = false;
        }
        if !open {
            st.open = false;
        }
        ctx.data_mut(|d| d.insert_temp(gallery_id(), st));
    }

    /// The Texture painting slots: the slot the brush paints and that slot's texture and depth
    /// (`mica_texture_paint_slots` on the object), plus the soft border width.
    pub(crate) fn texture_paint_slots(&mut self, ui: &mut egui::Ui, slot: &mut u8) {
        let Some(i) = self.selected else { return };
        let Some(o) = self.plates.current().objects.get(i) else {
            return;
        };
        let p = theme::of(ui);
        let base = self
            .effective_config()
            .map(|c| {
                texture::TextureSettings::from_lookup(|k| {
                    o.overrides.get(k).cloned().or_else(|| c.raw(k))
                })
            })
            .and_then(Result::ok);
        let mut slots: BTreeMap<u8, BTreeMap<String, String>> = o
            .overrides
            .get(SLOTS_KEY)
            .and_then(|v| texture::parse_slots(v).ok())
            .unwrap_or_default();
        let value_of = |n: u8| {
            base.as_ref().and_then(|b| b.slot(n).ok()).map_or_else(
                || "procedural:knurl_diamond".to_string(),
                |s| s.source.key(),
            )
        };
        let depth_of = |n: u8| {
            base.as_ref()
                .and_then(|b| b.slot(n).ok())
                .map_or(0.4, |s| s.amplitude)
        };
        let falloff = base.as_ref().map_or(1., |b| b.falloff);
        let mut edits: Vec<(String, String)> = Vec::new();
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                for n in 1..=SLOTS {
                    let on = *slot == n;
                    let (rect, r) = ui.allocate_exact_size(Vec2::new(56., 32.), Sense::click());
                    ui.painter().rect_filled(rect, 7., if r.hovered() { p.hover } else { p.raised });
                    ui.painter().rect_stroke(
                        rect,
                        7.,
                        Stroke::new(1., if on { p.accent } else { p.border_raised }),
                        egui::StrokeKind::Inside,
                    );
                    paint_thumb(ui, Rect::from_min_size(rect.left_top() + Vec2::new(5., 5.), Vec2::splat(22.)), &value_of(n));
                    ui.painter().text(
                        Pos2::new(rect.right() - 9., rect.center().y),
                        Align2::RIGHT_CENTER,
                        n.to_string(),
                        theme::mono(12.),
                        if on { p.accent_hi } else { p.text },
                    );
                    if r.on_hover_text(format!("Paint with texture slot {n}: {}", display_name(&value_of(n)))).clicked() {
                        *slot = n;
                    }
                }
            });
            ui.add_space(2.);
            let n = (*slot).clamp(1, SLOTS);
            let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::hover());
            ui.painter().text(
                Pos2::new(row.left(), row.center().y),
                Align2::LEFT_CENTER,
                format!("Slot {n}"),
                theme::sans(12.),
                p.muted_alt,
            );
            let field = Rect::from_min_max(Pos2::new(row.left() + 52., row.top() + 1.), Pos2::new(row.right(), row.bottom() - 1.));
            if let Some((value, tile)) = picker(ui, ui.make_persistent_id(("texture-slot", n)), field, &value_of(n), true) {
                let entry = slots.entry(n).or_default();
                entry.insert(TEXTURE_KEY.into(), value);
                entry.insert("mica_texture_skin_scale".into(), widgets::format_number(tile, 1));
                edits.push((SLOTS_KEY.into(), texture::format_slots(&slots)));
            }
            let number = |ui: &mut egui::Ui, label: &str, id: &str, v: f64, max: f64, tip: &str| {
                let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
                ui.painter().text(Pos2::new(row.left(), row.center().y), Align2::LEFT_CENTER, label, theme::sans(12.), p.muted_alt);
                let field = Rect::from_min_size(Pos2::new(row.left() + 104., row.top() + 1.), Vec2::new(72., 26.));
                let out = widgets::number_with(
                    ui,
                    ui.make_persistent_id(id),
                    field,
                    v,
                    &widgets::NumberSpec { unit: "mm", decimals: 2, min: 0., max, speed: 0.01 },
                    true,
                    false,
                );
                ui.interact(field, ui.make_persistent_id((id, "tip")), Sense::hover()).on_hover_text(tip);
                out
            };
            let own = slots.get(&n).is_some_and(|m| m.contains_key(DEPTH_KEY));
            let tip = if own {
                "How deep this slot's texture goes"
            } else {
                "How deep this slot's texture goes; now the object's Depth. Set a value to give the slot its own."
            };
            let depth = number(ui, "Depth", &format!("texture-slot-depth-{n}"), depth_of(n), texture::MAX_DEPTH, tip);
            // Next to the field: whether the slot has its own depth, and a way back to the object's.
            let at = Pos2::new(ui.min_rect().left() + 186., ui.min_rect().bottom() - 14.);
            if own {
                let r = ui.put(
                    Rect::from_min_size(Pos2::new(at.x, at.y - 10.), Vec2::new(120., 20.)),
                    egui::Label::new(egui::RichText::new("Use object's").font(theme::sans(12.)).color(p.accent)).sense(Sense::click()),
                );
                if r.on_hover_text("Remove this slot's own depth: it follows the object's Depth again").clicked() {
                    set_slot_depth(&mut slots, n, None);
                    edits.push((SLOTS_KEY.into(), texture::format_slots(&slots)));
                }
            } else {
                ui.painter().text(at, Align2::LEFT_CENTER, "Object's depth", theme::sans(12.), p.muted_alt);
            }
            if let Some(d) = depth {
                set_slot_depth(&mut slots, n, Some(d));
                edits.push((SLOTS_KEY.into(), texture::format_slots(&slots)));
            }
            if let Some(f) = number(ui, "Soft border", "texture-paint-falloff", falloff, 50., "The texture fades in over this distance from the painted area's edge (0: hard edge)") {
                edits.push(("mica_texture_skin_falloff".into(), widgets::format_number(f, 2)));
            }
            ui.label(
                egui::RichText::new("Painted faces get the slot's texture, also with Texture off.")
                    .font(theme::sans(12.))
                    .color(p.muted_alt),
            );
        });
        for (key, raw) in edits {
            self.apply_edit(None, crate::panels::editor::Edit::Set { key, raw });
            self.end_gesture();
        }
    }
}

/// The slot option that holds a slot's own depth.
const DEPTH_KEY: &str = "mica_texture_skin_amplitude";

/// Gives slot `n` its own depth (mm), or with `None` lets it follow the object's Depth again
/// (a slot without overrides is dropped from the list).
pub fn set_slot_depth(
    slots: &mut BTreeMap<u8, BTreeMap<String, String>>,
    n: u8,
    depth: Option<f64>,
) {
    match depth {
        Some(d) => {
            slots
                .entry(n)
                .or_default()
                .insert(DEPTH_KEY.into(), widgets::format_number(d, 2));
        }
        None => {
            if let Some(m) = slots.get_mut(&n) {
                m.remove(DEPTH_KEY);
                if m.is_empty() {
                    slots.remove(&n);
                }
            }
        }
    }
}

/// One textured surface for the Prepare view: an object, or the area painted with one slot,
/// drawn with the texture's relief (`render/surface_texture.wgsl`).
#[derive(Clone)]
pub struct SurfacePreview {
    pub object: u64,
    pub map_key: String,
    pub map: Arc<HeightMap>,
    /// The shader's projection parameters (see `surface_texture.wgsl`).
    pub params: [[f32; 4]; 7],
    /// The painted area's key and mesh-local triangles; `None` textures the whole object.
    pub area: Option<(u64, Arc<Vec<[f32; 3]>>)>,
    /// Changes whenever the drawing changes (for the frame hash).
    pub key: u64,
}

fn shader_params(s: &texture::TextureSettings, bounds: Bounds, aspect: f64) -> [[f32; 4]; 7] {
    let pl = texture::projection::placement(&s.mapping, bounds, aspect);
    let m = &s.mapping;
    let (sin, cos) = m.rotation.to_radians().sin_cos();
    let c: [f64; 3] = std::array::from_fn(|k| 0.5 * (bounds.min[k] + bounds.max[k]));
    let axis = match m.axis {
        texture::projection::Axis::X => 0.,
        texture::projection::Axis::Y => 1.,
        texture::projection::Axis::Z => 2.,
    };
    let f = |v: [f64; 4]| v.map(|x| x as f32);
    [
        f([
            bounds.min[0],
            bounds.min[1],
            bounds.min[2],
            f64::from(m.projection.index()),
        ]),
        f([pl.su, pl.sv, m.offset_u, m.offset_v]),
        f([cos, sin, s.amplitude, m.blend]),
        f([pl.cyl_center[0], pl.cyl_center[1], pl.cyl_radius, pl.cyl_su]),
        f([
            pl.sph_radius,
            pl.sph_su,
            m.cap_angle.to_radians().cos(),
            axis,
        ]),
        f([c[0], c[1], c[2], 0.]),
        f([
            s.top_angle,
            s.bottom_angle,
            if s.surfaces == texture::Surfaces::WallsAndTops {
                1.
            } else {
                0.
            },
            0.,
        ]),
    ]
}
use mica_core::texture::projection::Bounds;
use std::sync::Arc;

/// The texture previews of these objects with `config` (the plate's settings), cached per
/// object until its placement, settings or paint change.
pub fn previews(
    objects: &[crate::scene::Object],
    config: &mica_core::config::Config,
) -> Vec<SurfacePreview> {
    use std::hash::{Hash, Hasher};
    type Cache = HashMap<u64, (u64, Vec<SurfacePreview>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache.retain(|id, _| objects.iter().any(|o| o.id == *id));
    let mut out = Vec::new();
    for o in objects {
        let lookup = |k: &str| o.overrides.get(k).cloned().or_else(|| config.raw(k));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for k in texture::KEYS {
            lookup(k).hash(&mut h);
        }
        for v in o.transform().to_cols_array() {
            v.to_bits().hash(&mut h);
        }
        (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
        o.painting.texture.hash(&mut h);
        let key = h.finish();
        if let Some((k, v)) = cache.get(&o.id)
            && *k == key
        {
            out.extend(v.iter().cloned());
            continue;
        }
        let mut made = Vec::new();
        if let Ok(base) = texture::TextureSettings::from_lookup(lookup) {
            let painted = !o.painting.texture.is_empty();
            if base.enabled() || painted {
                // The texture anchors at the placed object's bounding box, as when slicing.
                let t = o.transform();
                let mut min = [f64::INFINITY; 3];
                let mut max = [f64::NEG_INFINITY; 3];
                for v in &o.mesh.vertices {
                    let w = t.transform_point3(glam::Vec3::from_array(*v));
                    for k in 0..3 {
                        min[k] = min[k].min(f64::from(w[k]));
                        max[k] = max[k].max(f64::from(w[k]));
                    }
                }
                let bounds = Bounds { min, max };
                let mut push = |s: &texture::TextureSettings, area| {
                    if let Ok(map) = s.map() {
                        made.push(SurfacePreview {
                            object: o.id,
                            map_key: s.map_key(),
                            params: shader_params(s, bounds, map.aspect()),
                            map,
                            area,
                            key,
                        });
                    }
                };
                if base.enabled() {
                    push(&base, None);
                }
                if painted {
                    let mut by_slot: BTreeMap<u8, Vec<[f32; 3]>> = BTreeMap::new();
                    o.painting.texture.visit(&o.mesh, false, |state, tri| {
                        if state > 0 {
                            by_slot
                                .entry(state)
                                .or_default()
                                .extend(tri.map(|p| p.map(|c| c as f32)));
                        }
                    });
                    for (n, triangles) in by_slot {
                        if let Ok(s) = base.slot(n) {
                            let mut ah = std::collections::hash_map::DefaultHasher::new();
                            (o.id, key, n).hash(&mut ah);
                            push(&s, Some((ah.finish(), Arc::new(triangles))));
                        }
                    }
                }
            }
        }
        cache.insert(o.id, (key, made.clone()));
        out.extend(made);
    }
    out
}

fn previews_id() -> egui::Id {
    egui::Id::unique("mica-surface-previews")
}
/// Hands this frame's texture previews to the 3D view.
pub fn publish(ctx: &egui::Context, previews: Vec<SurfacePreview>) {
    ctx.data_mut(|d| d.insert_temp(previews_id(), Arc::new(previews)));
}
/// The texture previews the app published for this frame.
pub fn published(ctx: &egui::Context) -> Vec<SurfacePreview> {
    ctx.data(|d| d.get_temp::<Arc<Vec<SurfacePreview>>>(previews_id()))
        .map(|p| p.as_ref().clone())
        .unwrap_or_default()
}

/// Reads a PNG or JPG, makes it grey and adds it as a custom texture; the setting value.
pub fn import_image(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let grey = img.to_luma8();
    let (w, h) = grey.dimensions();
    let map = HeightMap::from_gray8(w as usize, h as usize, grey.as_raw())?;
    Ok(texture::register_map(&map))
}

/// Whether settings or paint of these objects use textures.
pub fn objects_use_textures(
    objects: &[crate::scene::Object],
    config: &mica_core::config::Config,
) -> bool {
    let on = |get: &dyn Fn(&str) -> Option<String>| {
        get("mica_texture_skin").is_some_and(|v| v != "none")
    };
    on(&|k| config.raw(k))
        || objects.iter().any(|o| {
            !o.painting.texture.is_empty()
                || on(&|k| o.overrides.get(k).cloned())
                || o.volumes
                    .iter()
                    .any(|v| on(&|k| v.overrides.get(k).cloned()))
                || o.height_ranges
                    .iter()
                    .any(|r| on(&|k| r.overrides.get(k).cloned()))
                || o.region
                    .as_deref()
                    .is_some_and(mica_core::features::region_uses_textures)
        })
}

/// The objects without any texture settings or texture paint (the Textures feature is off).
pub fn without_textures(mut objects: Vec<crate::scene::Object>) -> Vec<crate::scene::Object> {
    let strip = |m: &mut BTreeMap<String, String>| m.retain(|k, _| !k.starts_with("mica_texture"));
    for o in &mut objects {
        strip(&mut o.overrides);
        for v in &mut o.volumes {
            strip(&mut v.overrides);
        }
        for r in &mut o.height_ranges {
            strip(&mut r.overrides);
        }
        if !o.painting.texture.is_empty() {
            std::sync::Arc::make_mut(&mut o.painting).texture = Default::default();
        }
        // An imported project's own parts (as `mica slice` strips them).
        if let Some(r) = &mut o.region
            && mica_core::features::region_uses_textures(r)
        {
            *r = crate::scene::derived_region(
                crate::scene::Derived::WithoutTextures,
                r,
                mica_core::features::region_without_textures,
            );
        }
    }
    objects
}

/// A configuration without textures (the Textures feature is off), as `mica slice` makes it.
pub fn config_without_textures(c: &mut mica_core::config::Config) {
    mica_core::features::config_without_textures(c);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_has_a_thumbnail() {
        let entries = entries();
        assert!(entries.len() >= 52);
        for e in entries.iter().filter(|e| e.category != Category::Custom) {
            let map = map_of(&e.value).expect(&e.value);
            let img = thumbnail_image(&map, 32);
            assert_eq!(img.size, [32, 32]);
        }
        assert_eq!(display_name("builtin:roof_tiles"), "Roof Tiles");
        assert_eq!(
            display_name("procedural:knurl_diamond"),
            "Knurling, diamond"
        );
    }

    #[test]
    fn slot_depth_follows_the_object_until_set() {
        let mut slots = texture::parse_slots("2:texture=builtin:bark").unwrap();
        let object = |slots: &BTreeMap<u8, BTreeMap<String, String>>| {
            let text = texture::format_slots(slots);
            texture::TextureSettings::from_lookup(|k| match k {
                "mica_texture_skin_amplitude" => Some("0.6".into()),
                SLOTS_KEY => Some(text.clone()),
                _ => None,
            })
            .unwrap()
        };
        assert_eq!(object(&slots).slot(2).unwrap().amplitude, 0.6);
        set_slot_depth(&mut slots, 2, Some(0.15));
        set_slot_depth(&mut slots, 3, Some(1.2));
        assert_eq!(
            texture::format_slots(&slots),
            "2:amplitude=0.15,texture=builtin:bark;3:amplitude=1.20"
        );
        assert_eq!(object(&slots).slot(2).unwrap().amplitude, 0.15);
        // Back to the object's depth: the texture stays, slot 3 has nothing left.
        set_slot_depth(&mut slots, 2, None);
        set_slot_depth(&mut slots, 3, None);
        assert_eq!(texture::format_slots(&slots), "2:texture=builtin:bark");
        assert_eq!(object(&slots).slot(2).unwrap().amplitude, 0.6);
        assert_eq!(object(&slots).slot(3).unwrap().amplitude, 0.6);
    }

    #[test]
    fn stripping_textures_leaves_other_overrides() {
        let mut c = mica_core::config::Config::defaults();
        c.set_raw("mica_texture_skin", "external").unwrap();
        config_without_textures(&mut c);
        assert_eq!(c.string("mica_texture_skin"), Some("none"));
    }
}
