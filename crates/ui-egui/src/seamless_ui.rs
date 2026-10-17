//! The Seamless Clone tool: drag the active layer to where it should go; on release it is placed
//! there and its colours are fitted to what is underneath (`edit.seamlessClone`, a Poisson solve).
//! While dragging, the layer's outline follows the pointer.

use egui::{Color32, Stroke};
use photocraft_geom::Rect;
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};
use crate::state::Tool;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeamlessDrag {
    from: [f64; 2],
    to: [f64; 2],
    bounds: Rect,
}

impl SeamlessDrag {
    fn offset(&self) -> [i32; 2] {
        [(self.to[0] - self.from[0]).round() as i32, (self.to[1] - self.from[1]).round() as i32]
    }
}

fn layer_bounds(app: &PhotocraftApp) -> Option<Rect> {
    let st = app.session.active()?;
    let b = st.doc.layer(st.active_layer?)?.surface()?.content_bounds();
    (!b.is_empty()).then_some(b)
}

/// True when the event was the tool's.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent) -> bool {
    if app.ui.tool != Tool::SeamlessClone {
        return false;
    }
    match ev {
        ToolEvent::Down { x, y, .. } => match layer_bounds(app) {
            Some(bounds) => app.distort.seamless = Some(SeamlessDrag { from: [x, y], to: [x, y], bounds }),
            None => app.ui.status = tl!("Seamless Clone needs a pixel layer with content").into(),
        },
        ToolEvent::Move { x, y, .. } => {
            if let Some(d) = app.distort.seamless.as_mut() {
                d.to = [x, y];
            }
        }
        ToolEvent::Up { x, y } => {
            if let Some(mut d) = app.distort.seamless.take() {
                d.to = [x, y];
                let off = d.offset();
                if off != [0, 0] {
                    let _ = app.run("edit.seamlessClone", json!({"offset": off}));
                }
            }
        }
    }
    true
}

pub fn draw(d: &SeamlessDrag, painter: &egui::Painter, xf: &ViewXform) {
    let [dx, dy] = d.offset();
    let b = d.bounds.translate(dx, dy);
    let r = egui::Rect::from_two_pos(xf.to_screen(b.x0 as f32, b.y0 as f32), xf.to_screen(b.x1 as f32, b.y1 as f32));
    let accent = crate::theme::Tokens::get(painter.ctx()).accent;
    painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 24));
    painter.rect_stroke(r, 0.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_moves_the_layer_and_blends_it_in_one_step() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width": 80, "height": 60, "depth": 8})).unwrap();
        app.session.execute("layer.new.layer", json!({})).unwrap();
        app.session
            .edit("paint", |doc, active| {
                doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap().fill_rect(Rect::new(5, 5, 15, 15), &[1.0, 0.0, 0.0, 1.0]);
                Ok(())
            })
            .unwrap();
        app.sync_views();
        app.ui.tool = Tool::SeamlessClone;
        let before = app.session.active().unwrap().history.past_len();
        assert!(crate::distort_ui::pointer(&mut app, ToolEvent::Down { x: 10.0, y: 10.0, pressure: 1.0 }, egui::Modifiers::NONE));
        crate::distort_ui::pointer(&mut app, ToolEvent::Move { x: 40.0, y: 20.0, pressure: 1.0 }, egui::Modifiers::NONE);
        assert_eq!(app.distort.seamless.map(|d| d.offset()), Some([30, 10]));
        crate::distort_ui::pointer(&mut app, ToolEvent::Up { x: 40.0, y: 20.0 }, egui::Modifiers::NONE);
        assert!(app.distort.seamless.is_none());
        let st = app.session.active().unwrap();
        assert_eq!(st.history.past_len(), before + 1);
        let b = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().content_bounds();
        assert_eq!(b, Rect::new(35, 15, 45, 25));
    }

    #[test]
    fn a_click_without_a_drag_does_nothing() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width": 40, "height": 40, "depth": 8})).unwrap();
        app.session.execute("layer.new.layer", json!({})).unwrap();
        app.session
            .edit("paint", |doc, active| {
                doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap().fill_rect(Rect::new(5, 5, 15, 15), &[1.0, 0.0, 0.0, 1.0]);
                Ok(())
            })
            .unwrap();
        app.ui.tool = Tool::SeamlessClone;
        let before = app.session.active().unwrap().history.past_len();
        crate::distort_ui::pointer(&mut app, ToolEvent::Down { x: 10.0, y: 10.0, pressure: 1.0 }, egui::Modifiers::NONE);
        crate::distort_ui::pointer(&mut app, ToolEvent::Up { x: 10.0, y: 10.0 }, egui::Modifiers::NONE);
        assert_eq!(app.session.active().unwrap().history.past_len(), before);
    }
}
