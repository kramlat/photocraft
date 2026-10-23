//! Window › MyPaint Brushes: the MyPaint brush library as a floating panel of thumbnails, in the
//! pack's groups, with a filter box and a size slider. A pack's own `<name>_prev.png` is the
//! thumbnail when there is one; otherwise the engine draws a stroke (`Library::preview`).
//! Clicking a brush selects it and switches to the MyPaint Brush tool.

use egui::{Color32, ColorImage, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2};
use serde_json::json;

use crate::PhotocraftApp;
use crate::state::Tool;
use crate::theme::Tokens;

const PANEL: &str = "mypaintBrushes";
const MIN_THUMB: f32 = 40.0;
const MAX_THUMB: f32 = 128.0;
const DEFAULT_THUMB: f32 = 64.0;
const GAP: f32 = 4.0;

fn texture(app: &mut PhotocraftApp, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
    let key = egui::Id::new(("mypaint-prev", id));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return Some(t);
    }
    let p = app.session.mypaint.preview(id)?;
    let img = ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.rgba);
    let tex = ctx.load_texture(format!("mypaint-prev-{id}"), img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    Some(tex)
}

/// The panel body (inside a floating window).
pub fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let size = *app.ui.presets_ui.sizes.get(PANEL).unwrap_or(&DEFAULT_THUMB);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.ui.presets_ui.mypaint_filter).hint_text(tl!("Search…")).desired_width(110.0));
        let mut s = size;
        ui.add_sized(vec2(70.0, 18.0), egui::Slider::new(&mut s, MIN_THUMB..=MAX_THUMB).show_value(false)).on_hover_text(tl!("Thumbnail size"));
        if (s - size).abs() > 0.01 {
            app.ui.presets_ui.sizes.insert(PANEL.into(), s);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::secondary_button(ui, tl!("Import…"), 0.0).clicked() {
                crate::mypaint_ui::import(app);
            }
        });
    });
    ui.add_space(2.0);
    let filter = app.ui.presets_ui.mypaint_filter.trim().to_lowercase();
    let groups = app.session.mypaint.groups();
    let current = app.session.mypaint.current().map(|c| c.0);
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("mypaint-scroll").max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        let mut shown = 0;
        for (gi, (group, brushes)) in groups.iter().enumerate() {
            let items: Vec<&(String, String, f32)> = brushes.iter().filter(|b| filter.is_empty() || b.1.to_lowercase().contains(&filter) || group.to_lowercase().contains(&filter)).collect();
            if items.is_empty() {
                continue;
            }
            shown += items.len();
            let toggled = app.ui.presets_ui.toggled.contains(&format!("{PANEL}/{group}"));
            // The first group starts open; a filter opens every group that has a match.
            let open = !filter.is_empty() || (gi == 0) != toggled;
            let (hr, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(hr, 2.0, t.hover.gamma_multiply(0.5));
            }
            let tri = Rect::from_center_size(pos2(hr.left() + 9.0, hr.center().y), vec2(12.0, 12.0));
            crate::icons::paint(ui, tri, if open { "chevron-down" } else { "chevron-right" }, 10.0, t.text_dim);
            ui.painter().text(pos2(hr.left() + 24.0, hr.center().y), egui::Align2::LEFT_CENTER, format!("{group} ({})", items.len()), egui::FontId::proportional(11.5), t.text);
            if resp.clicked() && filter.is_empty() {
                let k = format!("{PANEL}/{group}");
                if !app.ui.presets_ui.toggled.remove(&k) {
                    app.ui.presets_ui.toggled.insert(k);
                }
            }
            if !open {
                continue;
            }
            let w = ui.available_width();
            let cols = (((w - 8.0 + GAP) / (size + GAP)).floor() as usize).max(1);
            let rows = items.len().div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (size + GAP) + 2.0), Sense::hover());
            for (ii, (id, name, _)) in items.iter().map(|b| (&b.0, &b.1, b.2)).enumerate() {
                let r = Rect::from_min_size(area.min + vec2(8.0 + (ii % cols) as f32 * (size + GAP), (ii / cols) as f32 * (size + GAP)), vec2(size, size));
                let resp = ui.interact(r, ui.id().with((PANEL, gi, ii)), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
                if ui.is_rect_visible(r) {
                    ui.painter().rect_filled(r, 2.0, t.field);
                    if let Some(tex) = texture(app, &ctx, id) {
                        ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    let stroke = if current.as_deref() == Some(id.as_str()) {
                        Stroke::new(2.0, t.accent)
                    } else if resp.hovered() {
                        Stroke::new(1.0, t.text_dim)
                    } else {
                        Stroke::new(1.0, t.card_border)
                    };
                    ui.painter().rect_stroke(r, 2.0, stroke, egui::StrokeKind::Outside);
                }
                if resp.on_hover_text(name).clicked() {
                    picked = Some(id.clone());
                }
            }
        }
        if shown == 0 {
            ui.label(egui::RichText::new(tl!("No brushes match")).color(t.text_dim).size(11.5));
        }
    });
    if let Some(id) = picked {
        select(app, &id);
    }
}

/// Picks a brush for the MyPaint Brush tool and switches to it; the tool then paints at the
/// brush's own size.
pub fn select(app: &mut PhotocraftApp, id: &str) {
    if app.run("mypaint.brushes.select", json!({"brush": id})).is_ok() {
        app.ui.tool_options.mypaint_size = 0.0;
        app.ui.tool = Tool::MyPaintBrush;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> PhotocraftApp {
        PhotocraftApp::new(photocraft_engine::Session::new(), Default::default())
    }

    #[test]
    fn the_window_item_toggles_the_panel_and_the_first_group_shows_the_packs_previews() {
        let mut app = app();
        let ctx = egui::Context::default();
        PhotocraftApp::setup_context(&ctx, crate::theme::ThemeKind::ALL[0]);
        assert_eq!(crate::preset_panels::checked(&app, "window.panel.mypaintBrushes"), Some(false));
        crate::preset_panels::menu(&mut app, "window.panel.mypaintBrushes", &json!({})).unwrap().unwrap();
        assert!(app.ui.presets_ui.mypaint);
        assert_eq!(crate::preset_panels::checked(&app, "window.panel.mypaintBrushes"), Some(true));
        ctx.run_ui(egui::RawInput::default(), |ui| panel(&mut app, ui)).textures_delta.clear();
        let first = app.session.mypaint.groups()[0].1[0].0.clone();
        assert!(ctx.data(|d| d.get_temp::<TextureHandle>(egui::Id::new(("mypaint-prev", first.as_str())))).is_some(), "{first} has a thumbnail");
        crate::preset_panels::menu(&mut app, "window.panel.mypaintBrushes", &json!({})).unwrap().unwrap();
        assert!(!app.ui.presets_ui.mypaint);
    }

    #[test]
    fn picking_a_brush_selects_it_switches_tool_and_resets_the_size() {
        let mut app = app();
        app.session.mypaint.ensure();
        app.ui.tool = Tool::Brush;
        app.ui.tool_options.mypaint_size = 33.0;
        select(&mut app, "classic/pencil");
        assert_eq!(app.ui.tool, Tool::MyPaintBrush);
        assert_eq!(app.ui.tool_options.mypaint_size, 0.0);
        assert_eq!(app.session.mypaint.current().unwrap().0, "classic/pencil");
        select(&mut app, "no/such/brush");
        assert_eq!(app.session.mypaint.current().unwrap().0, "classic/pencil", "an unknown brush changes nothing");
    }
}
