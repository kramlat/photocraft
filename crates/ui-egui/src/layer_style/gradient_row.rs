//! The Gradient Overlay page's gradient editor: a ramp with colour stops below it and
//! transparency stops above it, as Photoshop's Gradient Editor lays them out. Click empty
//! ramp space to add a stop, drag a stop to move it, Delete (or the button) removes the
//! selected one, and the selected stop's location, colour or opacity is edited underneath.
//! It edits plain stop lists; the caller maps them onto the dialog's `stops` / `opacityStops`.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::theme::Tokens;
use crate::widgets;

/// A colour stop: location `0..=1` and sRGB colour.
pub(super) type ColorStop = (f32, [f32; 3]);
/// A transparency stop: location `0..=1` and opacity `0..=1`.
pub(super) type OpacityStop = (f32, f32);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Marker {
    Color(usize),
    Opacity(usize),
}

const HIT: f32 = 7.0;
const MIN_STOPS: usize = 2;

/// Linear interpolation between sorted `(location, value)` stops; ends hold their value.
fn lerp_at<const N: usize>(stops: &[(f32, [f32; N])], u: f32) -> [f32; N] {
    let Some(first) = stops.first() else { return [0.0; N] };
    let Some(last) = stops.last() else { return first.1 };
    if u <= first.0 {
        return first.1;
    }
    if u >= last.0 {
        return last.1;
    }
    for w in stops.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if u >= a && u <= b {
            let t = if b > a { (u - a) / (b - a) } else { 0.0 };
            return std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * t);
        }
    }
    last.1
}

/// The colour at `u` of sorted colour stops.
pub(super) fn color_at(stops: &[ColorStop], u: f32) -> [f32; 3] {
    lerp_at(stops, u)
}

/// The opacity at `u` of sorted transparency stops (none = opaque).
pub(super) fn opacity_at(stops: &[OpacityStop], u: f32) -> f32 {
    if stops.is_empty() {
        return 1.0;
    }
    let one: Vec<(f32, [f32; 1])> = stops.iter().map(|(l, o)| (*l, [*o])).collect();
    lerp_at(&one, u)[0]
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn c32(c: [f32; 3], a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(byte(c[0]), byte(c[1]), byte(c[2]), byte(a))
}

/// The ramp as one vertex-coloured mesh (no seams between cells).
fn ramp_mesh(bar: Rect, colors: &[ColorStop], opacity: &[OpacityStop]) -> egui::Mesh {
    const N: usize = 96;
    let mut mesh = egui::Mesh::default();
    for i in 0..=N {
        let u = i as f32 / N as f32;
        let x = bar.left() + u * bar.width();
        let color = c32(color_at(colors, u), opacity_at(opacity, u));
        mesh.colored_vertex(pos2(x, bar.top()), color);
        mesh.colored_vertex(pos2(x, bar.bottom()), color);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    mesh
}

/// Index a stop moved to `location` lands at once the list is sorted.
fn sorted_index(locations: impl Iterator<Item = f32>, moved: usize, location: f32) -> usize {
    locations.enumerate().filter(|(i, l)| *i != moved && *l < location).count()
}

/// Edits `colors` and `opacity` in place. Returns whether either changed. `salt` keeps the
/// selection per dialog instance.
pub(super) fn show(ui: &mut Ui, salt: egui::Id, colors: &mut Vec<ColorStop>, opacity: &mut Vec<OpacityStop>) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut changed = false;
    colors.sort_by(|a, b| a.0.total_cmp(&b.0));
    opacity.sort_by(|a, b| a.0.total_cmp(&b.0));
    let select_key = salt.with("gradient-row-selection");
    let drag_key = salt.with("gradient-row-drag");
    let mut selected: Marker = ui.data(|d| d.get_temp(select_key)).unwrap_or(Marker::Color(0));
    let mut dragging: Option<Marker> = ui.data(|d| d.get_temp(drag_key));
    // A selection that points past a shorter list (another effect, a delete) falls back.
    selected = match selected {
        Marker::Color(i) if i >= colors.len() => Marker::Color(colors.len().saturating_sub(1)),
        Marker::Opacity(i) if i >= opacity.len() => Marker::Color(0),
        m => m,
    };

    let w = ui.available_width().max(160.0);
    let (outer, resp) = ui.allocate_exact_size(vec2(w, 70.0), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Gradient stops"));
    let bar = Rect::from_min_max(pos2(outer.left() + 8.0, outer.top() + 16.0), pos2(outer.right() - 8.0, outer.top() + 40.0));
    let x_of = |loc: f32| bar.left() + loc.clamp(0.0, 1.0) * bar.width();
    let loc_of = |x: f32| ((x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0);

    // Pointer interaction. A press picks a marker (colour stops hang below the ramp,
    // transparency stops sit above it) or adds one on the ramp's own half.
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() || resp.clicked() && dragging.is_none() {
            let below = p.y >= bar.center().y;
            let hit = |x: f32| (p.x - x).abs() <= HIT;
            let picked = if below {
                colors.iter().position(|s| hit(x_of(s.0))).map(Marker::Color)
            } else {
                opacity.iter().position(|s| hit(x_of(s.0))).map(Marker::Opacity)
            };
            let in_zone = p.y >= outer.top() && p.y <= outer.bottom() && p.x >= bar.left() - HIT && p.x <= bar.right() + HIT;
            if let Some(m) = picked {
                selected = m;
                dragging = Some(m);
            } else if in_zone {
                let u = loc_of(p.x);
                if below {
                    colors.push((u, color_at(colors, u)));
                    colors.sort_by(|a, b| a.0.total_cmp(&b.0));
                    selected = Marker::Color(colors.iter().position(|s| s.0 == u).unwrap_or(0));
                } else {
                    let o = opacity_at(opacity, u);
                    if opacity.is_empty() {
                        opacity.extend([(0.0, 1.0), (1.0, 1.0)]);
                    }
                    opacity.push((u, o));
                    opacity.sort_by(|a, b| a.0.total_cmp(&b.0));
                    selected = Marker::Opacity(opacity.iter().position(|s| s.0 == u).unwrap_or(0));
                }
                dragging = Some(selected);
                changed = true;
            }
        }
        if let Some(m) = dragging.filter(|_| resp.dragged()) {
            let u = loc_of(p.x);
            match m {
                Marker::Color(i) => {
                    if let Some(s) = colors.get_mut(i) {
                        s.0 = u;
                        let at = sorted_index(colors.iter().map(|s| s.0), i, u);
                        let s = colors.remove(i);
                        colors.insert(at.min(colors.len()), s);
                        dragging = Some(Marker::Color(at));
                        selected = Marker::Color(at);
                        changed = true;
                    }
                }
                Marker::Opacity(i) => {
                    if let Some(s) = opacity.get_mut(i) {
                        s.0 = u;
                        let at = sorted_index(opacity.iter().map(|s| s.0), i, u);
                        let s = opacity.remove(i);
                        opacity.insert(at.min(opacity.len()), s);
                        dragging = Some(Marker::Opacity(at));
                        selected = Marker::Opacity(at);
                        changed = true;
                    }
                }
            }
        }
    }
    if !resp.dragged() {
        dragging = None;
    }

    // The ramp over a checkerboard.
    let painter = ui.painter();
    widgets::checker(&painter.with_clip_rect(bar), bar, 6.0);
    painter.add(egui::Shape::mesh(ramp_mesh(bar, colors, opacity)));
    painter.rect_stroke(bar, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
    let ink = |on: bool| if on { t.accent } else { t.text_dim };
    for (i, (loc, c)) in colors.iter().enumerate() {
        let x = x_of(*loc);
        let on = selected == Marker::Color(i);
        let body = Rect::from_center_size(pos2(x, bar.bottom() + 11.0), vec2(10.0, 10.0));
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(x, bar.bottom() + 1.0), pos2(x + 5.0, body.top()), pos2(x - 5.0, body.top())],
            ink(on),
            Stroke::NONE,
        ));
        painter.rect_filled(body, 1.0, c32(*c, 1.0));
        painter.rect_stroke(body, 1.0, Stroke::new(1.0, ink(on)), StrokeKind::Outside);
    }
    for (i, (loc, o)) in opacity.iter().enumerate() {
        let x = x_of(*loc);
        let on = selected == Marker::Opacity(i);
        let body = Rect::from_center_size(pos2(x, bar.top() - 11.0), vec2(10.0, 10.0));
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(x, bar.top() - 1.0), pos2(x + 5.0, body.bottom()), pos2(x - 5.0, body.bottom())],
            ink(on),
            Stroke::NONE,
        ));
        let g = byte(1.0 - o);
        painter.rect_filled(body, 1.0, Color32::from_gray(g));
        painter.rect_stroke(body, 1.0, Stroke::new(1.0, ink(on)), StrokeKind::Outside);
    }

    // The selected stop's fields.
    let mut delete = false;
    ui.horizontal(|ui| match selected {
        Marker::Color(i) => {
            if let Some(&(loc, c)) = colors.get(i) {
                let mut pct = loc * 100.0;
                ui.label(egui::RichText::new(tl!("Location")).color(t.text_dim));
                if widgets::value_field(ui, &mut pct, 0.0..=100.0, "%", 64.0).changed() {
                    let u = (pct / 100.0).clamp(0.0, 1.0);
                    let at = sorted_index(colors.iter().map(|s| s.0), i, u);
                    let mut s = colors.remove(i);
                    s.0 = u;
                    colors.insert(at.min(colors.len()), s);
                    selected = Marker::Color(at);
                    changed = true;
                }
                let mut rgb = [byte(c[0]), byte(c[1]), byte(c[2])];
                if widgets::color_edit_button_srgb(ui, &mut rgb).changed()
                    && let Some(s) = colors.iter_mut().find(|s| s.0 == loc && s.1 == c)
                {
                    s.1 = rgb.map(|v| f32::from(v) / 255.0);
                    changed = true;
                }
                delete = colors.len() > MIN_STOPS && widgets::secondary_button(ui, tl!("Delete"), 0.0).clicked();
            }
        }
        Marker::Opacity(i) => {
            if let Some(&(loc, o)) = opacity.get(i) {
                let mut pct = o * 100.0;
                ui.label(egui::RichText::new(tl!("Opacity")).color(t.text_dim));
                if widgets::value_field(ui, &mut pct, 0.0..=100.0, "%", 64.0).changed()
                    && let Some(s) = opacity.get_mut(i)
                {
                    s.1 = (pct / 100.0).clamp(0.0, 1.0);
                    changed = true;
                }
                let mut at_pct = loc * 100.0;
                ui.label(egui::RichText::new(tl!("Location")).color(t.text_dim));
                if widgets::value_field(ui, &mut at_pct, 0.0..=100.0, "%", 64.0).changed() {
                    let u = (at_pct / 100.0).clamp(0.0, 1.0);
                    let at = sorted_index(opacity.iter().map(|s| s.0), i, u);
                    let mut s = opacity.remove(i);
                    s.0 = u;
                    opacity.insert(at.min(opacity.len()), s);
                    selected = Marker::Opacity(at);
                    changed = true;
                }
                delete = widgets::secondary_button(ui, tl!("Delete"), 0.0).clicked();
            }
        }
    });
    let key_delete = resp.hovered() && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace));
    if delete || key_delete {
        match selected {
            Marker::Color(i) if colors.len() > MIN_STOPS && i < colors.len() => {
                colors.remove(i);
                selected = Marker::Color(i.saturating_sub(1).min(colors.len() - 1));
                changed = true;
            }
            Marker::Opacity(i) if i < opacity.len() => {
                opacity.remove(i);
                // Fewer than two transparency stops means "fully opaque".
                if opacity.len() < 2 {
                    opacity.clear();
                }
                selected = Marker::Color(0);
                changed = true;
            }
            _ => {}
        }
    }
    ui.data_mut(|d| {
        d.insert_temp(select_key, selected);
        if let Some(m) = dragging {
            d.insert_temp(drag_key, m);
        } else {
            d.remove::<Marker>(drag_key);
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn colours_and_opacity_interpolate_between_stops() {
        let stops = [(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 0.5, 0.0])];
        assert_eq!(color_at(&stops, 0.5), [0.5, 0.25, 0.0]);
        assert_eq!(color_at(&stops, -1.0), [0.0; 3]);
        assert_eq!(color_at(&stops, 2.0), [1.0, 0.5, 0.0]);
        assert_eq!(opacity_at(&[], 0.3), 1.0);
        assert_eq!(opacity_at(&[(0.0, 1.0), (1.0, 0.0)], 0.25), 0.75);
    }

    #[test]
    fn clicking_below_the_ramp_adds_a_colour_stop_there() {
        use egui_kittest::{Harness, kittest::Queryable};
        let state: (Vec<ColorStop>, Vec<OpacityStop>, bool) = (vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])], Vec::new(), false);
        let mut h = Harness::builder().with_size(vec2(300.0, 160.0)).build_ui_state(
            |ui, s: &mut (Vec<ColorStop>, Vec<OpacityStop>, bool)| {
                let (c, o) = (&mut s.0, &mut s.1);
                s.2 |= show(ui, egui::Id::new("t"), c, o);
            },
            state,
        );
        crate::PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
        h.run_steps(2);
        h.get_by_label("Gradient stops").click();
        h.run_steps(3);
        let (colors, opacity, changed) = h.state();
        assert!(*changed);
        assert_eq!(colors.len(), 3, "{colors:?}");
        assert!((colors[1].0 - 0.5).abs() < 0.05, "added near the middle: {colors:?}");
        assert!((colors[1].1[0] - 0.5).abs() < 0.05, "takes the colour already there");
        assert!(opacity.is_empty(), "no transparency stops were touched");
    }

    #[test]
    fn a_moved_stop_lands_among_its_neighbours() {
        let locs = [0.0, 0.4, 0.8];
        assert_eq!(sorted_index(locs.iter().copied(), 0, 0.5), 1);
        assert_eq!(sorted_index(locs.iter().copied(), 0, 0.9), 2);
        assert_eq!(sorted_index(locs.iter().copied(), 2, 0.1), 1);
    }
}
