//! Edit › Cage Transform on the canvas.
//!
//! A closed polygon starts out hugging the layer; drag its corners and the image follows
//! smoothly (mean value coordinates, `photocraft_algo::cage`). Click an edge to add a corner,
//! ⌥-click a corner to remove it. ↩ / Commit runs `edit.cageTransform` with the cage. The preview
//! is the layer texture on a 40×40 mesh pushed through the same map the engine uses.

use std::sync::Arc;

use egui::{Color32, Pos2, Stroke, TextureHandle, pos2};
use photocraft_algo::cage::Cage;
use photocraft_doc::{Document, LayerId};
use photocraft_geom::Rect;
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};

const GRID: usize = 40;

pub struct CageSession {
    pub key: u64,
    pub layer: LayerId,
    bounds: Rect,
    pub cage: Cage,
    /// Corner being dragged, and the pointer offset from it.
    drag: Option<(usize, [f64; 2])>,
    texture: TextureHandle,
    opacity: f32,
    pub preview_doc: Arc<Document>,
    grid: Vec<[f64; 2]>,
    /// Started by selecting the Cage Transform tool (so changing tool ends it).
    pub from_tool: bool,
}

impl CageSession {
    pub fn describe(&self) -> Value {
        json!({"layer": self.layer.0, "cage": self.cage})
    }

    fn update_grid(&mut self) {
        let b = self.bounds;
        self.grid.clear();
        for j in 0..=GRID {
            for i in 0..=GRID {
                let x = f64::from(b.x0) + f64::from(b.width()) * i as f64 / GRID as f64;
                let y = f64::from(b.y0) + f64::from(b.height()) * j as f64 / GRID as f64;
                let (u, v) = self.cage.map(x, y);
                self.grid.push([u, v]);
            }
        }
    }

    fn corner_at(&self, p: [f64; 2], tol: f64) -> Option<usize> {
        self.cage
            .dst
            .iter()
            .enumerate()
            .map(|(i, c)| (i, (c[0] - p[0]).hypot(c[1] - p[1])))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Adds a corner on the edge nearest `p` (within `tol`), at the same fraction along the
    /// original polygon. Returns its index.
    fn insert_on_edge(&mut self, p: [f64; 2], tol: f64) -> Option<usize> {
        let n = self.cage.dst.len();
        let mut best: Option<(usize, f64, f64)> = None;
        for i in 0..n {
            let (a, b) = (self.cage.dst[i], self.cage.dst[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len2 = dx * dx + dy * dy;
            if len2 < 1e-9 {
                continue;
            }
            let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
            let d = (a[0] + dx * t - p[0]).hypot(a[1] + dy * t - p[1]);
            if d <= tol && best.is_none_or(|(_, _, bd)| d < bd) {
                best = Some((i, t, d));
            }
        }
        let (i, t, _) = best?;
        let (sa, sb) = (self.cage.src[i], self.cage.src[(i + 1) % n]);
        self.cage.src.insert(i + 1, [sa[0] + (sb[0] - sa[0]) * t, sa[1] + (sb[1] - sa[1]) * t]);
        self.cage.dst.insert(i + 1, p);
        Some(i + 1)
    }

    fn reset(&mut self) {
        self.cage.dst = self.cage.src.clone();
        self.update_grid();
    }
}

pub fn begin(app: &mut PhotocraftApp, ctx: &egui::Context, from_tool: bool) -> Result<(), String> {
    if app.ui.transform.is_some() {
        return Err("finish Free Transform first".into());
    }
    let (layer, surface, _) = crate::distort_ui::active_pixels(app)?;
    let st = app.session.active().ok_or("no document")?;
    let l = st.doc.layer(layer).ok_or("no layer")?;
    let bounds = surface.content_bounds();
    if bounds.is_empty() {
        return Err("Could not use Cage Transform: the layer is empty".into());
    }
    let opacity = l.opacity * l.fill_opacity;
    let preview_doc = crate::distort_ui::without_layer(&st.doc, layer);
    let key = app.ui.alloc_id();
    let texture = ctx.load_texture(format!("cage-{key}"), crate::distort_ui::surface_image(&surface, bounds, 2048), egui::TextureOptions::LINEAR);
    let (x0, y0, x1, y1) = (f64::from(bounds.x0), f64::from(bounds.y0), f64::from(bounds.x1), f64::from(bounds.y1));
    let corners = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let mut s = CageSession {
        key,
        layer,
        bounds,
        cage: Cage { src: corners.clone(), dst: corners },
        drag: None,
        texture,
        opacity,
        preview_doc,
        grid: Vec::new(),
        from_tool,
    };
    s.update_grid();
    app.distort.cage = Some(s);
    app.ui.status = tl!("Cage Transform: drag the corner points; click an edge to add one").into();
    Ok(())
}

pub fn commit(app: &mut PhotocraftApp) {
    let Some(s) = app.distort.cage.take() else { return };
    if s.cage.is_identity() {
        return;
    }
    let _ = app.run(photocraft_engine::distort_cmds::CAGE, json!({"cage": s.cage, "layer": s.layer.0}));
}

/// Control channel: `edit.cageTransform {"ui": {...}}` while the mode is active.
pub fn control(app: &mut PhotocraftApp, ui: &Value) -> Result<Value, String> {
    if ui.get("commit").and_then(Value::as_bool) == Some(true) {
        commit(app);
        return Ok(json!({"committed": true}));
    }
    if ui.get("cancel").and_then(Value::as_bool) == Some(true) {
        app.distort.cage = None;
        return Ok(json!({"cancelled": true}));
    }
    let s = app.distort.cage.as_mut().ok_or(tl!("Cage Transform is not active"))?;
    if let Some(d) = ui.get("dst") {
        let dst: Vec<[f64; 2]> = serde_json::from_value(d.clone()).map_err(|e| format!("bad dst: {e}"))?;
        if dst.len() != s.cage.src.len() || dst.iter().flatten().any(|v| !v.is_finite()) {
            return Err("dst must hold one finite point per cage corner".into());
        }
        s.cage.dst = dst;
        s.drag = None;
        s.update_grid();
    }
    if ui.get("reset").and_then(Value::as_bool) == Some(true) {
        s.reset();
    }
    Ok(s.describe())
}

pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: egui::Modifiers) {
    let tol = crate::distort_ui::tolerance(app);
    let Some(s) = app.distort.cage.as_mut() else { return };
    match ev {
        ToolEvent::Down { x, y, .. } => {
            let p = [x, y];
            if let Some(i) = s.corner_at(p, tol) {
                if mods.alt {
                    if s.cage.dst.len() > 3 {
                        s.cage.src.remove(i);
                        s.cage.dst.remove(i);
                        s.update_grid();
                    }
                    return;
                }
                let c = s.cage.dst[i];
                s.drag = Some((i, [c[0] - x, c[1] - y]));
            } else if let Some(i) = s.insert_on_edge(p, tol * 1.5) {
                s.drag = Some((i, [0.0, 0.0]));
            }
        }
        ToolEvent::Move { x, y, .. } => {
            if let Some((i, off)) = s.drag
                && let Some(c) = s.cage.dst.get_mut(i)
            {
                *c = [x + off[0], y + off[1]];
                s.update_grid();
            }
        }
        ToolEvent::Up { x, y } => {
            if let Some((i, off)) = s.drag.take()
                && let Some(c) = s.cage.dst.get_mut(i)
            {
                *c = [x + off[0], y + off[1]];
            }
            s.update_grid();
        }
    }
}

pub fn draw(s: &CageSession, painter: &egui::Painter, xf: &ViewXform) {
    let scr = |p: [f64; 2]| xf.to_screen(p[0] as f32, p[1] as f32);
    let mut mesh = egui::Mesh::with_texture(s.texture.id());
    let tint = Color32::from_white_alpha((s.opacity.clamp(0.0, 1.0) * 255.0) as u8);
    for j in 0..=GRID {
        for i in 0..=GRID {
            let p = s.grid[j * (GRID + 1) + i];
            mesh.vertices.push(egui::epaint::Vertex { pos: scr(p), uv: pos2(i as f32 / GRID as f32, j as f32 / GRID as f32), color: tint });
        }
    }
    for j in 0..GRID {
        for i in 0..GRID {
            let a = (j * (GRID + 1) + i) as u32;
            mesh.add_triangle(a, a + 1, a + GRID as u32 + 2);
            mesh.add_triangle(a, a + GRID as u32 + 2, a + GRID as u32 + 1);
        }
    }
    painter.add(mesh);
    let accent = crate::theme::Tokens::get(painter.ctx()).accent;
    let faint = Stroke::new(0.75, Color32::from_rgba_unmultiplied(255, 255, 255, 120));
    let orig: Vec<Pos2> = s.cage.src.iter().map(|c| scr(*c)).collect();
    painter.add(egui::Shape::closed_line(orig, faint));
    let pts: Vec<Pos2> = s.cage.dst.iter().map(|c| scr(*c)).collect();
    painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(1.5, accent)));
    for c in pts {
        painter.circle_filled(c, 4.5, Color32::WHITE);
        painter.circle_stroke(c, 4.5, Stroke::new(1.25, accent));
    }
}

pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let Some(s) = app.distort.cage.as_mut() else { return };
    ui.label(crate::i18n::trn(crate::i18n::current(), s.cage.dst.len() as u64, "{n} point", "{n} points"));
    if ui.button(tl!("Reset")).clicked() {
        s.reset();
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if crate::widgets::primary_button(ui, "✓", 32.0)
            .on_hover_text(crate::i18n::fmt(tl!("Commit Cage Transform ({key})"), &[("key", &crate::shortcuts::pretty("Enter"))]))
            .clicked()
        {
            commit(app);
        } else if crate::widgets::secondary_button(ui, "⊘", 32.0).on_hover_text(tl!("Cancel (Esc)")).clicked() {
            app.distort.cage = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_block() -> (PhotocraftApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width": 120, "height": 90, "depth": 8})).unwrap();
        app.session.execute("layer.new.layer", json!({})).unwrap();
        app.session
            .edit("paint", |doc, active| {
                doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap().fill_rect(Rect::new(20, 20, 80, 60), &[0.9, 0.3, 0.1, 1.0]);
                Ok(())
            })
            .unwrap();
        app.sync_views();
        (app, ctx)
    }

    #[test]
    fn drag_corner_add_and_remove_points_then_commit() {
        let (mut app, ctx) = app_with_block();
        crate::distort_ui::menu(&mut app, &ctx, "edit.cageTransform", &json!({})).unwrap().unwrap();
        let ev = |app: &mut PhotocraftApp, e, m| crate::distort_ui::pointer(app, e, m);
        let none = egui::Modifiers::NONE;
        assert_eq!(app.distort.cage.as_ref().unwrap().cage.dst.len(), 4);
        // Drag the top-right corner out.
        ev(&mut app, ToolEvent::Down { x: 80.0, y: 20.0, pressure: 1.0 }, none);
        ev(&mut app, ToolEvent::Move { x: 95.0, y: 12.0, pressure: 1.0 }, none);
        ev(&mut app, ToolEvent::Up { x: 100.0, y: 10.0 }, none);
        assert_eq!(app.distort.cage.as_ref().unwrap().cage.dst[1], [100.0, 10.0]);
        // Click the bottom edge's middle: a fifth point appears, and dragging it moves only it.
        ev(&mut app, ToolEvent::Down { x: 50.0, y: 60.0, pressure: 1.0 }, none);
        ev(&mut app, ToolEvent::Up { x: 50.0, y: 75.0 }, none);
        let c = &app.distort.cage.as_ref().unwrap().cage;
        assert_eq!(c.dst.len(), 5);
        assert_eq!(c.src[3], [50.0, 60.0], "the original polygon gets the same new corner");
        assert_eq!(c.dst[3], [50.0, 75.0]);
        // ⌥-click removes it again.
        ev(&mut app, ToolEvent::Down { x: 50.0, y: 75.0, pressure: 1.0 }, egui::Modifiers::ALT);
        assert_eq!(app.distort.cage.as_ref().unwrap().cage.dst.len(), 4);
        let before = app.session.active().unwrap().history.past_len();
        commit(&mut app);
        assert_eq!(app.session.active().unwrap().history.past_len(), before + 1);
        assert!(app.distort.cage.is_none());
        let st = app.session.active().unwrap();
        let b = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().content_bounds();
        assert!(b.x1 > 80, "the corner pulled the content outwards: {b:?}");
    }

    #[test]
    fn an_untouched_cage_commits_nothing_and_a_triangle_cannot_lose_a_corner() {
        let (mut app, ctx) = app_with_block();
        crate::distort_ui::menu(&mut app, &ctx, "edit.cageTransform", &json!({})).unwrap().unwrap();
        let before = app.session.active().unwrap().history.past_len();
        commit(&mut app);
        assert_eq!(app.session.active().unwrap().history.past_len(), before);
        crate::distort_ui::menu(&mut app, &ctx, "edit.cageTransform", &json!({})).unwrap().unwrap();
        for _ in 0..3 {
            crate::distort_ui::pointer(&mut app, ToolEvent::Down { x: 20.0, y: 20.0, pressure: 1.0 }, egui::Modifiers::ALT);
        }
        assert_eq!(app.distort.cage.as_ref().unwrap().cage.dst.len(), 3, "never fewer than three");
    }
}
