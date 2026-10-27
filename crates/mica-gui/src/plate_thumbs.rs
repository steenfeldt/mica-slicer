// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Small pictures of the plates for the plate lists (Preview's plate list, the plate bar's
//! list, the project summary's timeline): the picture a saved project carries for the plate
//! (`mica_core::threemf::plate_picture`: its objects in their filament colours from the iso
//! camera, colour painting, translucent filaments). Drawn off the UI thread and kept per plate
//! until its objects, their placement, tools, painting or the filament colours change; while
//! one is drawn the plate keeps its old picture, or shows a placeholder. Empty plates have none.

use crate::scene::Object;
use eframe::egui;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, mpsc};

/// Pixels of a picture (shown at about 40 points, sharp on a 2x screen).
pub const PX: u16 = 96;

/// A plate's picture as the lists show it.
#[derive(Clone, Default)]
pub enum Thumb {
    /// No objects: nothing to picture.
    #[default]
    Empty,
    /// Being drawn.
    Pending,
    Ready(egui::TextureHandle),
}

impl Thumb {
    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        match self {
            Thumb::Ready(t) => Some(t),
            _ => None,
        }
    }
}

struct Entry {
    /// What the shown picture was drawn from.
    key: u64,
    texture: Option<egui::TextureHandle>,
    /// The key being drawn now.
    drawing: Option<u64>,
}

/// The pictures, by plate id.
pub struct PlateThumbs {
    entries: HashMap<u64, Entry>,
    tx: mpsc::Sender<(u64, u64, Vec<u8>)>,
    rx: mpsc::Receiver<(u64, u64, Vec<u8>)>,
    /// This frame's pictures in plate order.
    pub list: Vec<Thumb>,
}

impl Default for PlateThumbs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            entries: HashMap::new(),
            tx,
            rx,
            list: Vec::new(),
        }
    }
}

/// What a plate's picture depends on.
pub fn key(objects: &[Object], colours: &[[u8; 4]]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    colours.hash(&mut h);
    for o in objects {
        (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
        (Arc::as_ptr(&o.painting) as usize).hash(&mut h);
        o.tools.0[0].hash(&mut h);
        for v in o.transform().to_cols_array() {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// What a picture needs of an object (shared meshes: cheap to hand to the drawing thread).
struct Item {
    mesh: Arc<mica_core::mesh::TriangleMesh>,
    painting: Arc<mica_core::painting::FacetPainting>,
    transform: glam::Mat4,
    tool: usize,
}

fn items(objects: &[Object]) -> Vec<Item> {
    objects
        .iter()
        .map(|o| Item {
            mesh: o.mesh.clone(),
            painting: o.painting.clone(),
            transform: o.transform(),
            tool: o.tools.0[0].unwrap_or(0) + 1,
        })
        .collect()
}

/// Draws a plate's picture: RGBA, [`PX`] square.
pub fn draw(objects: &[Object], colours: &[[u8; 4]]) -> Vec<u8> {
    draw_items(&items(objects), colours)
}

fn draw_items(items: &[Item], colours: &[[u8; 4]]) -> Vec<u8> {
    let parts: Vec<mica_core::threemf::PicturePart> = items
        .iter()
        .map(|o| {
            let m = o.transform;
            let c = |i: usize| m.col(i);
            let row = |r: usize| {
                [
                    f64::from(c(0)[r]),
                    f64::from(c(1)[r]),
                    f64::from(c(2)[r]),
                    f64::from(c(3)[r]),
                ]
            };
            mica_core::threemf::PicturePart {
                mesh: &o.mesh,
                painting: &o.painting,
                transform: mica_core::threemf::Transform([row(0), row(1), row(2)]),
                tool: o.tool,
            }
        })
        .collect();
    mica_core::threemf::plate_picture(&parts, colours, PX)
}

impl PlateThumbs {
    /// Brings the pictures up to date with the plates (`(id, objects)` in plate order) and the
    /// tools' colours: takes finished pictures, starts the ones missing. `wait` draws them
    /// before returning (headless screenshots, tests).
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        plates: &[(u64, &[Object])],
        colours: &[[u8; 4]],
        wait: bool,
    ) {
        self.receive(ctx);
        let mut list = Vec::with_capacity(plates.len());
        for &(id, objects) in plates {
            if objects.is_empty() {
                self.entries.remove(&id);
                list.push(Thumb::Empty);
                continue;
            }
            let k = key(objects, colours);
            let entry = self.entries.entry(id).or_insert(Entry {
                key: 0,
                texture: None,
                drawing: None,
            });
            if entry.texture.is_none() || entry.key != k {
                if wait {
                    let rgba = draw(objects, colours);
                    entry.texture = Some(texture(ctx, id, &rgba));
                    entry.key = k;
                    entry.drawing = None;
                } else if entry.drawing != Some(k) {
                    entry.drawing = Some(k);
                    let items = items(objects);
                    let colours = colours.to_vec();
                    let tx = self.tx.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        let rgba = draw_items(&items, &colours);
                        let _ = tx.send((id, k, rgba));
                        ctx.request_repaint();
                    });
                }
            }
            list.push(match &entry.texture {
                Some(t) => Thumb::Ready(t.clone()),
                None => Thumb::Pending,
            });
        }
        // Plates removed since.
        let ids: Vec<u64> = plates.iter().map(|(id, _)| *id).collect();
        self.entries.retain(|id, _| ids.contains(id));
        self.list = list;
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((id, k, rgba)) = self.rx.try_recv() {
            if let Some(e) = self.entries.get_mut(&id)
                && e.drawing == Some(k)
            {
                e.texture = Some(texture(ctx, id, &rgba));
                e.key = k;
                e.drawing = None;
            }
        }
    }
}

fn texture(ctx: &egui::Context, id: u64, rgba: &[u8]) -> egui::TextureHandle {
    let size = usize::from(PX);
    ctx.load_texture(
        format!("plate picture {id}"),
        egui::ColorImage::from_rgba_unmultiplied([size, size], rgba),
        egui::TextureOptions::LINEAR,
    )
}

/// Paints a plate's picture into `rect`: the picture, a quiet placeholder while it is drawn,
/// nothing for an empty plate.
pub fn paint(painter: &egui::Painter, rect: egui::Rect, thumb: &Thumb) {
    let p = crate::theme::of_ctx(painter.ctx());
    match thumb {
        Thumb::Empty => {}
        Thumb::Pending => {
            painter.rect_filled(rect.shrink(2.), 5., p.raised);
        }
        Thumb::Ready(t) => {
            painter.image(
                t.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                egui::Color32::WHITE,
            );
        }
    }
}
