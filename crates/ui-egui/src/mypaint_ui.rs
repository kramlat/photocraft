//! The MyPaint Brush tool: its options bar (brush, size, opacity, Import…) and the params of its
//! strokes. The brushes are `.myb` files from MyPaint's own folders, the embedded default set, and
//! `mypaint.brushes.import`; the engine is `photocraft_engine::mypaint_cmds`.

use egui::Ui;
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::Drag;
use crate::theme::Tokens;

/// Options bar: Brush, Size, Opacity, Import….
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let label = |ui: &mut Ui, text: &str| {
        ui.label(egui::RichText::new(crate::i18n::t(text)).color(t.text_dim).size(12.0));
    };
    let groups = app.session.mypaint.groups();
    let current = app.session.mypaint.current();
    label(ui, "Brush:");
    let mut key = current.as_ref().map(|c| c.0.clone()).unwrap_or_default();
    let options: Vec<(String, String)> = groups.iter().flat_map(|(g, v)| v.iter().map(move |(id, name, _)| (id.clone(), format!("{g} › {name}")))).collect();
    let refs: Vec<(String, &str)> = options.iter().map(|(k, l)| (k.clone(), l.as_str())).collect();
    let before = key.clone();
    crate::widgets::dropdown(ui, "mypaint-brush", &mut key, &refs, 230.0);
    if key != before {
        let _ = app.run("mypaint.brushes.select", json!({"brush": key}));
        // A new brush starts at its own size.
        app.ui.tool_options.mypaint_size = 0.0;
    }
    let own = current.as_ref().map_or(10.0, |c| c.2);
    label(ui, "Size:");
    let mut size = if app.ui.tool_options.mypaint_size > 0.0 { app.ui.tool_options.mypaint_size } else { own };
    if crate::widgets::value_field(ui, &mut size, 0.5..=2000.0, "px", 66.0).changed() {
        app.ui.tool_options.mypaint_size = size;
    }
    label(ui, "Opacity:");
    crate::widgets::value_field(ui, &mut app.ui.tool_options.mypaint_opacity, 1.0..=100.0, "%", 58.0);
    if crate::widgets::secondary_button(ui, crate::i18n::t("Import…"), 0.0).clicked() {
        let _ = app.pick_file_bytes_filtered(&["myb"], |app, name, bytes| {
            let data = String::from_utf8_lossy(&bytes).into_owned();
            let stem = name.rsplit(['/', '\\']).next().unwrap_or("brush").trim_end_matches(".myb").to_string();
            app.run("mypaint.brushes.import", json!({"data": data, "name": stem})).map(|v| json!({"imported": v})).map_err(|e| e.to_string())
        });
    }
}

/// The params of a MyPaint stroke of drag `d` (the points are the stylus's, with times and tilt).
pub fn stroke_params(app: &PhotocraftApp, d: &Drag, seed: u64) -> Value {
    let o = &app.ui.tool_options;
    let mut p = json!({
        "points": app.stylus.stroke_points(&d.points),
        "opacity": f64::from(o.mypaint_opacity) / 100.0,
        "zoom": app.point_zoom(),
        "target": crate::canvas::paint_target(app),
        "seed": seed,
    });
    if o.mypaint_size > 0.0 {
        p["size"] = json!(o.mypaint_size);
    }
    p
}

/// The diameter (px) the MyPaint cursor outlines: the options bar's Size, or the brush's own.
pub fn cursor_size(app: &PhotocraftApp) -> f32 {
    let o = app.ui.tool_options.mypaint_size;
    if o > 0.0 {
        return o;
    }
    // The library is read-only here; a brush not loaded yet shows a plain 10 px.
    app.session.mypaint.current_size().unwrap_or(10.0)
}
