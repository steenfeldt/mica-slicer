// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/slic3r/GUI/Gizmos/GLGizmoMeasure.cpp :: GLGizmoMeasure::on_mouse, GLGizmoMeasure::render_dimensioning, GLGizmoMeasure::on_render_input_window
// Source: orca@8500fcdcca src/slic3r/GUI/Gizmos/GLGizmoMeasure.cpp :: GLGizmoMeasure::on_init
// Changes: Selection rules ported (first / second feature, clicking a selected one drops it and promotes the second, Shift picks a point on the feature, clicking a circle's or polygon's centre marker measures from the centre, Delete restarts, Esc unselects then leaves), the rest re-implemented for egui: features are found on whichever object is under the pointer (cached per object and transform) rather than a composite of the selection, hover / selection / dimension lines are drawn as screen-space overlays instead of raycast GL models, the result card copies "label: value" per row like upstream's clipboard buttons, and there is no "edit to scale". Key U as upstream (WXK_CONTROL_U).

//! The measure tool: point, edge, circle and flat-face features under the pointer, one or two
//! of them selected, and the distances / angle between them with dimension lines in the view.

use super::gizmo;
use crate::{
    measure::{Feature, MeasurementResult, Measuring, measure},
    scene::{Object, picking},
    theme,
};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2 as EVec2};
use glam::{DVec3, Vec3};
use std::{collections::HashMap, sync::Arc};

/// Upstream's selection colours: first teal, second magenta.
pub const FIRST: Color32 = Color32::from_rgb(64, 191, 191);
pub const SECOND: Color32 = Color32::from_rgb(191, 64, 191);
/// The hovered feature (the theme's hot handle colour, as upstream's hover).
pub const HOVER: Color32 = theme::HANDLE_HOT;

/// A selected feature: what is measured, and where it came from (a centre or a point on it).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub feature: Feature,
    /// The feature a centre or a Shift-picked point belongs to (`None` for the feature itself).
    pub source: Option<Feature>,
    pub is_center: bool,
    /// Scene object id the feature lies on (for drawing its plane).
    pub object: u64,
}
impl Item {
    fn whole(feature: Feature, object: u64) -> Self {
        Self {
            feature,
            source: None,
            is_center: false,
            object,
        }
    }
    /// "Edge", "Point on edge", "Center of circle", …
    pub fn describe(&self) -> String {
        let plain = |f: &Feature| match f {
            Feature::Point(_) => "Point",
            Feature::Edge { .. } => "Edge",
            Feature::Circle { .. } => "Circle",
            Feature::Plane { .. } => "Flat face",
        };
        match (&self.source, self.is_center) {
            (Some(s), true) => format!("Center of {}", plain(s).to_lowercase()),
            (Some(s), false) if !matches!(s, Feature::Point(_)) => {
                format!("Point on {}", plain(s).to_lowercase())
            }
            _ => plain(&self.feature).to_string(),
        }
    }
}

/// What the pointer is over: the feature, the hit point and the object.
#[derive(Clone, Debug)]
struct Hovered {
    feature: Feature,
    point: DVec3,
    object: u64,
}

/// A row of the result card: label, value text and the number copied with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub value: String,
}

#[derive(Default)]
pub struct MeasureTool {
    /// Features per object id, rebuilt when the object's transform changes.
    cache: HashMap<u64, (u64, Arc<Measuring>)>,
    hover: Option<Hovered>,
    /// Hovering the centre marker of a selected circle / polygon (index 0 or 1).
    hover_center: Option<usize>,
    pub first: Option<Item>,
    pub second: Option<Item>,
    pub result: Option<MeasurementResult>,
    /// The last copied row, shown as a confirmation.
    copied: Option<String>,
}

fn transform_key(o: &Object) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    o.transform().to_cols_array().map(f32::to_bits).hash(&mut h);
    (Arc::as_ptr(&o.mesh) as usize).hash(&mut h);
    h.finish()
}

fn v3(p: DVec3) -> Vec3 {
    p.as_vec3()
}

/// `12.345`
pub fn format_mm(v: f64) -> String {
    format!("{v:.3}")
}

impl MeasureTool {
    /// The features of an object, built on first use (and after it moved).
    pub fn measuring(&mut self, o: &Object) -> Arc<Measuring> {
        let key = transform_key(o);
        if let Some((k, m)) = self.cache.get(&o.id)
            && *k == key
        {
            return m.clone();
        }
        let m = Arc::new(Measuring::new(&o.mesh, o.transform()));
        self.cache.insert(o.id, (key, m.clone()));
        m
    }
    /// Drops cached objects that left the scene.
    pub fn retain(&mut self, objects: &[Object]) {
        self.cache
            .retain(|id, _| objects.iter().any(|o| o.id == *id));
    }
    /// Esc: drop the selection (true when there was one to drop).
    pub fn unselect(&mut self) -> bool {
        let had = self.first.is_some() || self.second.is_some();
        self.first = None;
        self.second = None;
        self.result = None;
        had
    }
    /// Finds the feature under the pointer ray. `shift` turns it into a point on the feature.
    pub fn update_hover(
        &mut self,
        objects: &[Object],
        ray: (Vec3, Vec3),
        gp: &gizmo::Painter<'_>,
        pointer: Option<Pos2>,
    ) {
        self.hover = None;
        self.hover_center = None;
        let Some(pointer) = pointer else {
            return;
        };
        // A selected feature's centre marker wins over the surface under it.
        for (i, item) in [&self.first, &self.second].into_iter().enumerate() {
            if let Some(item) = item {
                let source = item.source.as_ref().unwrap_or(&item.feature);
                if let Some(c) = source.center()
                    && let Some(at) = gp.project(v3(c))
                    && at.distance(pointer) < 9.
                {
                    self.hover_center = Some(i);
                    return;
                }
            }
        }
        let (origin, dir) = ray;
        let Some(hit) = picking::pick_face(objects, origin, dir) else {
            return;
        };
        let Some(o) = objects.get(hit.object) else {
            return;
        };
        let m = self.measuring(o);
        let point = (origin + dir * hit.distance).as_dvec3();
        if let Some(feature) = m.feature_at(hit.face, point) {
            self.hover = Some(Hovered {
                feature,
                point,
                object: o.id,
            });
        }
    }
    /// The point on `feature` nearest to `p` (Shift point selection).
    fn point_on(feature: &Feature, p: DVec3) -> DVec3 {
        match feature {
            Feature::Point(q) => *q,
            Feature::Edge { a, b, .. } => {
                let ab = *b - *a;
                let t = ((p - *a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0., 1.);
                *a + ab * t
            }
            Feature::Circle {
                center,
                radius,
                normal,
            } => {
                let proj = p - *normal * normal.dot(p - *center);
                let d = (proj - *center).normalize_or(crate::measure::get_orthogonal(*normal));
                *center + d * *radius
            }
            Feature::Plane { .. } => p,
        }
    }
    /// The item a click would select now, if any.
    fn candidate(&self, shift: bool) -> Option<Item> {
        if let Some(i) = self.hover_center {
            let item = if i == 0 { &self.first } else { &self.second };
            let item = item.as_ref()?;
            let source = item.source.clone().unwrap_or_else(|| item.feature.clone());
            if item.is_center {
                // Clicking the centre of a centre-selection goes back to the whole feature.
                return Some(Item::whole(source, item.object));
            }
            let c = source.center()?;
            return Some(Item {
                feature: Feature::Point(c),
                source: Some(source),
                is_center: true,
                object: item.object,
            });
        }
        let h = self.hover.as_ref()?;
        if shift && !matches!(h.feature, Feature::Point(_)) {
            return Some(Item {
                feature: Feature::Point(Self::point_on(&h.feature, h.point)),
                source: Some(h.feature.clone()),
                is_center: false,
                object: h.object,
            });
        }
        Some(Item::whole(h.feature.clone(), h.object))
    }
    /// A click with the tool on: upstream's selection rules. Returns whether it was used.
    pub fn click(&mut self, shift: bool) -> bool {
        let Some(item) = self.candidate(shift) else {
            return false;
        };
        self.select(item);
        true
    }
    /// Applies a click on `item`.
    pub fn select(&mut self, item: Item) {
        let center_switch = |slot: &Option<Item>| {
            slot.as_ref().is_some_and(|s| {
                let s_source = s.source.as_ref().unwrap_or(&s.feature);
                let i_source = item.source.as_ref().unwrap_or(&item.feature);
                s_source == i_source && s.is_center != item.is_center
            })
        };
        match &self.first {
            None => self.first = Some(item),
            Some(first) if *first == item => {
                // Deselecting the first promotes the second.
                self.first = self.second.take();
            }
            _ if center_switch(&self.first) => self.first = Some(item),
            _ if center_switch(&self.second) => self.second = Some(item),
            _ if self.second.as_ref() == Some(&item) => self.second = None,
            _ => self.second = Some(item),
        }
        self.copied = None;
        self.result = match (&self.first, &self.second) {
            (Some(a), Some(b)) => Some(measure(&a.feature, &b.feature)),
            _ => None,
        };
    }
    /// Rows of the result card (also what the copy buttons copy).
    pub fn rows(&self) -> Vec<Row> {
        let mut out = vec![];
        let single = |item: &Item, out: &mut Vec<Row>| match &item.feature {
            Feature::Circle { radius, .. } => {
                out.push(Row {
                    label: "Diameter",
                    value: format!("{} mm", format_mm(2. * radius)),
                });
                out.push(Row {
                    label: "Radius",
                    value: format!("{} mm", format_mm(*radius)),
                });
            }
            Feature::Edge { a, b, .. } => out.push(Row {
                label: "Length",
                value: format!("{} mm", format_mm((*b - *a).length())),
            }),
            _ => {}
        };
        match (&self.first, &self.second, &self.result) {
            (Some(a), None, _) => single(a, &mut out),
            (Some(_), Some(_), Some(m)) => {
                if let Some(angle) = m.angle
                    && angle != crate::measure::AngleAndEdges::DUMMY
                {
                    out.push(Row {
                        label: "Angle",
                        value: format!("{:.2}°", angle.angle.to_degrees()),
                    });
                }
                let show_strict = m.distance_strict.is_some_and(|s| {
                    m.distance_infinite
                        .is_none_or(|i| (s.dist - i.dist).abs() > crate::measure::EPSILON)
                });
                if let Some(d) = m.distance_infinite {
                    out.push(Row {
                        label: if show_strict {
                            "Perpendicular distance"
                        } else {
                            "Distance"
                        },
                        value: format!("{} mm", format_mm(d.dist)),
                    });
                }
                if show_strict && let Some(d) = m.distance_strict {
                    out.push(Row {
                        label: if m.distance_infinite.is_some() {
                            "Direct distance"
                        } else {
                            "Distance"
                        },
                        value: format!("{} mm", format_mm(d.dist)),
                    });
                }
                if let Some(xyz) = m.distance_xyz
                    && xyz.length() > crate::measure::EPSILON
                {
                    out.push(Row {
                        label: "Distance XYZ",
                        value: format!(
                            "X {}  Y {}  Z {} mm",
                            format_mm(xyz.x),
                            format_mm(xyz.y),
                            format_mm(xyz.z)
                        ),
                    });
                }
            }
            _ => {}
        }
        out
    }
    /// Copies one row as "label: value" (upstream's clipboard format) and remembers it.
    pub fn copy(&mut self, ctx: &egui::Context, row: &Row) -> String {
        let text = format!("{}: {}", row.label, row.value);
        ctx.copy_text(text.clone());
        self.copied = Some(row.label.to_string());
        text
    }

    // ----- drawing --------------------------------------------------------------------------

    /// Hovered feature, selections, centre markers and dimension lines.
    pub fn draw(&mut self, gp: &gizmo::Painter<'_>, objects: &[Object], shift: bool) {
        // Selections first, the hover on top.
        let selected: Vec<(Item, Color32)> =
            [(self.first.clone(), FIRST), (self.second.clone(), SECOND)]
                .into_iter()
                .filter_map(|(item, color)| item.map(|i| (i, color)))
                .collect();
        for (item, color) in &selected {
            let color = *color;
            {
                self.draw_feature(gp, objects, &item.feature, item.object, color, true);
                let source = item.source.as_ref().unwrap_or(&item.feature);
                if let Some(c) = source.center()
                    && !item.is_center
                    && let Some(p) = gp.project(v3(c))
                {
                    gp.painter
                        .circle(p, 4.5, Color32::TRANSPARENT, Stroke::new(1.5, color));
                }
            }
        }
        if let Some(i) = self.hover_center {
            let item = if i == 0 { &self.first } else { &self.second };
            if let Some(item) = item
                && let Some(c) = item.source.as_ref().unwrap_or(&item.feature).center()
                && let Some(p) = gp.project(v3(c))
            {
                gp.painter
                    .circle(p, 6., HOVER, Stroke::new(1.5, theme::OUTLINE));
            }
        } else if let Some(h) = self.hover.clone() {
            if shift && !matches!(h.feature, Feature::Point(_)) {
                let p = Self::point_on(&h.feature, h.point);
                self.draw_feature(
                    gp,
                    objects,
                    &h.feature,
                    h.object,
                    HOVER.gamma_multiply(0.5),
                    false,
                );
                self.draw_feature(gp, objects, &Feature::Point(p), h.object, HOVER, false);
            } else {
                self.draw_feature(gp, objects, &h.feature, h.object, HOVER, false);
            }
        }
        self.draw_dimensions(gp);
    }
    fn draw_feature(
        &mut self,
        gp: &gizmo::Painter<'_>,
        objects: &[Object],
        f: &Feature,
        object: u64,
        color: Color32,
        selected: bool,
    ) {
        let outline = Stroke::new(if selected { 6. } else { 5. }, theme::OUTLINE);
        match f {
            Feature::Point(p) => {
                if let Some(q) = gp.project(v3(*p)) {
                    gp.painter
                        .circle(q, 5., color, Stroke::new(1.5, theme::OUTLINE));
                }
            }
            Feature::Edge { a, b, .. } => {
                if let (Some(pa), Some(pb)) = (gp.project(v3(*a)), gp.project(v3(*b))) {
                    gp.painter.line_segment([pa, pb], outline);
                    gp.painter.line_segment([pa, pb], Stroke::new(3., color));
                }
            }
            Feature::Circle {
                center,
                radius,
                normal,
            } => {
                let [u, v, _] = crate::measure::orthonormal_basis(*normal);
                let pts: Vec<Pos2> = (0..=72)
                    .filter_map(|k| {
                        let t = k as f64 / 72. * std::f64::consts::TAU;
                        gp.project(v3(*center + (u * t.cos() + v * t.sin()) * *radius))
                    })
                    .collect();
                if pts.len() > 2 {
                    gp.painter.add(egui::Shape::line(pts.clone(), outline));
                    gp.painter
                        .add(egui::Shape::line(pts, Stroke::new(3., color)));
                }
                if let Some(c) = gp.project(v3(*center)) {
                    gp.painter.circle_filled(c, 2.5, color);
                }
            }
            Feature::Plane { index, .. } => {
                let Some(o) = objects.iter().find(|o| o.id == object) else {
                    return;
                };
                let m = self.measuring(o);
                let fill = color.gamma_multiply(if selected { 0.45 } else { 0.38 });
                // One unfeathered mesh (no seams between facets); very large regions are capped.
                let mut mesh = egui::Mesh::default();
                for tri in m.plane_triangles(*index).iter().take(20_000) {
                    let pts: Vec<Pos2> = tri.iter().filter_map(|p| gp.project(v3(*p))).collect();
                    if pts.len() == 3 {
                        let base = mesh.vertices.len() as u32;
                        for p in pts {
                            mesh.colored_vertex(p, fill);
                        }
                        mesh.add_triangle(base, base + 1, base + 2);
                    }
                }
                gp.painter.add(egui::Shape::mesh(mesh));
            }
        }
    }
    /// A dimension line with arrow heads and a label in the middle.
    fn dimension(gp: &gizmo::Painter<'_>, a: DVec3, b: DVec3, text: &str) {
        let (Some(pa), Some(pb)) = (gp.project(v3(a)), gp.project(v3(b))) else {
            return;
        };
        let d = pb - pa;
        if d.length() < 1. {
            label(gp, pa, text);
            return;
        }
        let dir = d.normalized();
        let n = EVec2::new(-dir.y, dir.x);
        let white = Color32::from_gray(245);
        gp.painter
            .line_segment([pa, pb], Stroke::new(4., theme::OUTLINE));
        gp.painter.line_segment([pa, pb], Stroke::new(1.6, white));
        // Arrows point outwards when the line is too short for them inside.
        let inward = d.length() > 40.;
        for (tip, back) in [(pa, -dir), (pb, dir)] {
            let back = if inward { -back } else { back };
            let pts = vec![tip, tip + back * 10. + n * 4.5, tip + back * 10. - n * 4.5];
            gp.painter.add(egui::Shape::convex_polygon(
                pts,
                white,
                Stroke::new(1., theme::OUTLINE),
            ));
        }
        label(gp, pa + d * 0.5, text);
    }
    fn draw_dimensions(&self, gp: &gizmo::Painter<'_>) {
        // A single circle: its diameter across.
        if let (Some(a), None) = (&self.first, &self.second)
            && let Feature::Circle {
                center,
                radius,
                normal,
            } = &a.feature
        {
            let u = crate::measure::get_orthogonal(*normal);
            Self::dimension(
                gp,
                *center - u * *radius,
                *center + u * *radius,
                &format!("Ø {} mm", format_mm(2. * radius)),
            );
            return;
        }
        let Some(m) = &self.result else {
            return;
        };
        if let Some(d) = m.distance_infinite {
            Self::dimension(gp, d.from, d.to, &format!("{} mm", format_mm(d.dist)));
        }
        if let Some(d) = m.distance_strict
            && m.distance_infinite
                .is_none_or(|i| (d.dist - i.dist).abs() > crate::measure::EPSILON)
        {
            Self::dimension(gp, d.from, d.to, &format!("{} mm", format_mm(d.dist)));
        }
        if let Some(angle) = m.angle
            && angle != crate::measure::AngleAndEdges::DUMMY
            && angle.radius > 0.
        {
            // The arc from the first edge to the second, about the angle's centre.
            let u = (angle.e1.1 - angle.e1.0).normalize_or_zero();
            let w = (angle.e2.1 - angle.e2.0).normalize_or_zero();
            let normal = u.cross(w).normalize_or_zero();
            let v = normal.cross(u);
            let r = angle.radius;
            let steps = 48;
            let pts: Vec<Pos2> = (0..=steps)
                .filter_map(|k| {
                    let t = angle.angle * k as f64 / steps as f64;
                    gp.project(v3(angle.center + (u * t.cos() + v * t.sin()) * r))
                })
                .collect();
            let white = Color32::from_gray(245);
            if pts.len() > 2 {
                gp.painter.add(egui::Shape::line(
                    pts.clone(),
                    Stroke::new(4., theme::OUTLINE),
                ));
                gp.painter
                    .add(egui::Shape::line(pts.clone(), Stroke::new(1.6, white)));
            }
            // Extension lines from the edges to the arc when they do not reach it.
            for (e, dir) in [(angle.e1, u), (angle.e2, w)] {
                let end = angle.center + dir * r;
                let near = if (e.0 - angle.center).length() < (e.1 - angle.center).length() {
                    e.0
                } else {
                    e.1
                };
                if let (Some(a), Some(b)) = (gp.project(v3(near)), gp.project(v3(end))) {
                    gp.painter
                        .line_segment([a, b], Stroke::new(1., white.gamma_multiply(0.7)));
                }
            }
            if let Some(mid) = pts.get(pts.len() / 2) {
                label(gp, *mid, &format!("{:.2}°", angle.angle.to_degrees()));
            }
        }
    }

    /// The card (top left of the view): selections, results with copy buttons, hints.
    pub fn card(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let rows = self.rows();
        let mut copy: Option<Row> = None;
        let mut restart = false;
        let describe = |item: &Option<Item>| {
            item.as_ref().map_or("Nothing".to_string(), |i| {
                let mut t = i.describe();
                match &i.feature {
                    Feature::Circle { radius, .. } => {
                        t.push_str(&format!(" · Ø {} mm", format_mm(2. * radius)))
                    }
                    Feature::Edge { a, b, .. } => {
                        t.push_str(&format!(" · {} mm", format_mm((*b - *a).length())))
                    }
                    _ => {}
                }
                t
            })
        };
        let (s1, s2) = (describe(&self.first), describe(&self.second));
        let copied = self.copied.clone();
        egui::Area::new(ui.scope_id().with("measure card"))
            .order(egui::Order::Middle)
            // Bottom left, above the axis triad (the object list owns the top left).
            .fixed_pos(Pos2::new(rect.left() + 14., rect.bottom() - 100.))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .show(ui.ctx(), |ui| {
                let p = theme::of(ui);
                theme::overlay_frame(ui)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(300.);
                        ui.spacing_mut().item_spacing = EVec2::new(8., 4.);
                        ui.label(
                            egui::RichText::new(crate::panels::widgets::spaced_upper("Measure"))
                                .font(theme::semibold(11.))
                                .color(p.muted),
                        );
                        for (n, (text, color)) in [(&s1, FIRST), (&s2, SECOND)].into_iter().enumerate() {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(EVec2::splat(10.), egui::Sense::hover());
                                ui.painter().rect_filled(r, 2., color);
                                ui.label(
                                    egui::RichText::new(format!("{} {text}", n + 1))
                                        .font(theme::sans(13.))
                                        .color(p.text),
                                );
                            });
                        }
                        if !rows.is_empty() {
                            ui.separator();
                        }
                        for row in &rows {
                            // A long value (XYZ) goes under its label.
                            let long = row.value.chars().count() > 18;
                            let label = egui::RichText::new(row.label)
                                .font(theme::sans(12.))
                                .color(p.label);
                            if long {
                                ui.label(label.clone());
                            }
                            ui.horizontal(|ui| {
                                if !long {
                                    ui.label(label);
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let tip = if copied.as_deref() == Some(row.label) {
                                        "Copied"
                                    } else {
                                        "Copy"
                                    };
                                    if crate::panels::widgets::styled_button(ui, tip, true, false, 22.)
                                        .on_hover_text("Copy this value to the clipboard")
                                        .clicked()
                                    {
                                        copy = Some(row.clone());
                                    }
                                    ui.label(
                                        egui::RichText::new(&row.value)
                                            .font(theme::mono(12.))
                                            .color(p.text),
                                    );
                                });
                            });
                        }
                        ui.add_space(2.);
                        ui.horizontal(|ui| {
                            if crate::panels::widgets::styled_button(
                                ui,
                                "Restart selection",
                                self.first.is_some(),
                                false,
                                24.,
                            )
                            .clicked()
                            {
                                restart = true;
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "Click a point, edge, circle or face · Shift: a point on it · click a center dot to measure from the center · Del restarts · Esc unselects",
                            )
                            .font(theme::sans(11.))
                            .color(p.muted),
                        );
                    });
            });
        if let Some(row) = copy {
            self.copy(ui.ctx(), &row);
        }
        if restart {
            self.unselect();
        }
    }
    /// The hover's feature kind, for the hint line.
    pub fn hover_name(&self) -> Option<&'static str> {
        self.hover.as_ref().map(|h| h.feature.name())
    }
    /// Scripted selection for screenshots and the monkey: picks the feature at a screen point.
    pub fn pick_at(
        &mut self,
        objects: &[Object],
        gp: &gizmo::Painter<'_>,
        camera_ray: (Vec3, Vec3),
        at: Pos2,
        shift: bool,
    ) -> bool {
        self.update_hover(objects, camera_ray, gp, Some(at));
        self.click(shift)
    }
}

/// A value label in the dark read-out style, centred on a point.
fn label(gp: &gizmo::Painter<'_>, at: Pos2, text: &str) {
    let galley = gp
        .painter
        .layout_no_wrap(text.to_owned(), theme::mono(12.), theme::DARK.text);
    let r = Rect::from_center_size(at, galley.size() + EVec2::new(12., 6.));
    let r = r.translate(super::clamp_shift(gp.rect, r));
    gp.painter
        .rect_filled(r, 5., theme::DARK.dark_box.gamma_multiply(0.92));
    gp.painter.rect_stroke(
        r,
        5.,
        Stroke::new(1., theme::DARK.border_raised),
        egui::StrokeKind::Inside,
    );
    gp.painter
        .galley(r.min + EVec2::new(6., 3.), galley, theme::DARK.text);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edge(a: [f64; 3], b: [f64; 3]) -> Item {
        Item::whole(
            Feature::Edge {
                a: DVec3::from(a),
                b: DVec3::from(b),
                center: None,
            },
            1,
        )
    }
    #[test]
    fn selection_rules_follow_upstream() {
        let mut t = MeasureTool::default();
        let a = edge([0., 0., 0.], [10., 0., 0.]);
        let b = edge([0., 0., 0.], [0., 10., 0.]);
        let c = edge([0., 0., 5.], [10., 0., 5.]);
        t.select(a.clone());
        assert_eq!(t.first, Some(a.clone()));
        assert!(t.result.is_none());
        // Single edge: its length.
        assert_eq!(t.rows()[0].label, "Length");
        assert_eq!(t.rows()[0].value, "10.000 mm");
        t.select(b.clone());
        assert_eq!(t.second, Some(b.clone()));
        let rows = t.rows();
        assert_eq!(rows[0].label, "Angle");
        assert_eq!(rows[0].value, "90.00°");
        // A third feature replaces the second.
        t.select(c.clone());
        assert_eq!(t.second, Some(c.clone()));
        assert_eq!(
            t.rows()
                .iter()
                .find(|r| r.label == "Distance")
                .unwrap()
                .value,
            "5.000 mm"
        );
        // Clicking the second again drops it; clicking the first promotes the second.
        t.select(c.clone());
        assert!(t.second.is_none());
        t.select(b.clone());
        t.select(a.clone());
        assert_eq!(t.first, Some(b));
        assert!(t.second.is_none());
        assert!(t.unselect());
        assert!(!t.unselect());
    }
    #[test]
    fn circle_centre_and_diameter() {
        let mut t = MeasureTool::default();
        let circle = Feature::Circle {
            center: DVec3::new(5., 5., 0.),
            radius: 2.5,
            normal: DVec3::Z,
        };
        t.select(Item::whole(circle.clone(), 1));
        let rows = t.rows();
        assert_eq!(
            rows[0],
            Row {
                label: "Diameter",
                value: "5.000 mm".into()
            }
        );
        // Switching the first selection to the circle's centre keeps one selection.
        t.select(Item {
            feature: Feature::Point(DVec3::new(5., 5., 0.)),
            source: Some(circle.clone()),
            is_center: true,
            object: 1,
        });
        assert!(t.second.is_none());
        assert_eq!(t.first.as_ref().unwrap().describe(), "Center of circle");
        // A point: centre to point distance with XYZ.
        t.select(Item::whole(Feature::Point(DVec3::new(8., 9., 0.)), 1));
        let rows = t.rows();
        assert_eq!(rows[0].value, "5.000 mm");
        assert!(rows.iter().any(|r| r.label == "Distance XYZ"));
        let ctx = egui::Context::default();
        let copied = t.copy(&ctx, &rows[0]);
        assert_eq!(copied, "Distance: 5.000 mm");
    }
    #[test]
    fn points_on_features() {
        let e = Feature::Edge {
            a: DVec3::ZERO,
            b: DVec3::new(10., 0., 0.),
            center: None,
        };
        assert_eq!(
            MeasureTool::point_on(&e, DVec3::new(3., 4., 0.)),
            DVec3::new(3., 0., 0.)
        );
        let c = Feature::Circle {
            center: DVec3::ZERO,
            radius: 2.,
            normal: DVec3::Z,
        };
        let p = MeasureTool::point_on(&c, DVec3::new(0., 7., 3.));
        assert!((p - DVec3::new(0., 2., 0.)).length() < 1e-12);
    }
}
