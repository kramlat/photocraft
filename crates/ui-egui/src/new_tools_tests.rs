//! The Ink, MyPaint Brush and Art History Brush tools and the History panel's source column.

use egui::Modifiers;
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, tool_event};
use crate::state::{HistorySource, Tool};

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.run("file.new", json!({"width": 160, "height": 80, "background": "transparent"})).unwrap();
    app.run("tools.setColors", json!({"foreground": "#000000", "background": "#ffffff"})).unwrap();
    app
}

fn layer(app: &PhotocraftApp) -> photocraft_engine::doc::Layer {
    let st = app.session.active().unwrap();
    st.doc.layer(st.active_layer.unwrap()).unwrap().clone()
}

fn alpha(app: &PhotocraftApp, x: i32, y: i32) -> f32 {
    layer(app).surface().unwrap().rgba(x, y)[3]
}

fn drag(app: &mut PhotocraftApp, pts: &[(f64, f64)]) {
    let (x, y) = pts[0];
    tool_event(app, ToolEvent::Down { x, y, pressure: 1.0 }, Modifiers::NONE);
    for &(x, y) in &pts[1..] {
        tool_event(app, ToolEvent::Move { x, y, pressure: 1.0 }, Modifiers::NONE);
    }
    let &(x, y) = pts.last().unwrap();
    tool_event(app, ToolEvent::Up { x, y }, Modifiers::NONE);
}

fn last_command(app: &PhotocraftApp) -> String {
    app.session.journal.last().cloned().unwrap().0
}

#[test]
fn the_new_tools_live_in_their_photoshop_flyout_groups() {
    // Brush group (B): Ink and MyPaint Brush after the Photoshop tools; History group (Y).
    for t in [Tool::Ink, Tool::MyPaintBrush] {
        assert_eq!(t.key(), 'B', "{t:?}");
        assert!(t.is_brushlike());
    }
    assert_eq!(Tool::ArtHistoryBrush.key(), 'Y');
    assert_eq!(Tool::from_name("Ink Tool"), Some(Tool::Ink));
    assert_eq!(Tool::from_name("myPaintBrush"), Some(Tool::MyPaintBrush));
    assert_eq!(Tool::from_name("art history brush"), Some(Tool::ArtHistoryBrush));
    for t in [Tool::Ink, Tool::MyPaintBrush, Tool::ArtHistoryBrush] {
        assert!(crate::icons::exists(crate::icons::tool_icon(t)), "{t:?}");
    }
}

#[test]
fn ink_draws_with_the_session_brush_through_the_live_path() {
    let mut app = app();
    app.run("tools.setBrush", json!({"brush": {"size": 14, "smoothing": {"amount": 0.0}}})).unwrap();
    app.ui.tool = Tool::Ink;
    crate::paint_mouse::sync_tool_brush(&mut app);
    let (x, y) = (10.0, 40.0);
    tool_event(&mut app, ToolEvent::Down { x, y, pressure: 1.0 }, Modifiers::NONE);
    tool_event(&mut app, ToolEvent::Move { x: 60.0, y: 41.0, pressure: 1.0 }, Modifiers::NONE);
    assert!(app.live_stroke.is_some(), "Ink strokes live");
    tool_event(&mut app, ToolEvent::Up { x: 120.0, y: 40.0 }, Modifiers::NONE);
    assert_eq!(last_command(&app), "paint.stroke");
    assert!(alpha(&app, 60, 40) > 0.5);
    assert_eq!(app.session.active().unwrap().history.past_len(), 1);
}

#[test]
fn a_slow_ink_stroke_is_thicker_than_a_quick_one() {
    // The speed control: the same line drawn with times 8 ms and 1 ms apart.
    let width = |ms: f64| {
        let mut s = photocraft_engine::Session::new();
        s.execute("file.new", json!({"width": 200, "height": 60, "background": "transparent"})).unwrap();
        let pts: Vec<serde_json::Value> = (0..60).map(|i| json!([10.0 + i as f64 * 3.0, 30.0, 1.0, 0, 0, 0, i as f64 * ms])).collect();
        let brush = json!({"hardness": 1.0, "roundness": 1.0, "shapeDynamics": {"enabled": true, "size": {"control": "speed", "minimum": 0.1}}});
        s.execute("paint.stroke", json!({"points": pts, "brush": brush, "size": 16, "color": "#000000", "seed": 1})).unwrap();
        let st = s.active().unwrap();
        let surf = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().clone();
        (0..60).map(|y| surf.rgba(100, y)[3]).sum::<f32>()
    };
    let (slow, quick) = (width(40.0), width(1.0));
    assert!(slow > quick + 3.0, "slow {slow} quick {quick}");
}

#[test]
fn the_mypaint_tool_draws_live_and_commits_what_it_showed() {
    let mut app = app();
    app.run("mypaint.brushes.select", json!({"brush": "classic/pencil"})).unwrap();
    app.ui.tool = Tool::MyPaintBrush;
    app.ui.tool_options.mypaint_size = 10.0;
    let (x, y) = (10.0, 40.0);
    tool_event(&mut app, ToolEvent::Down { x, y, pressure: 0.8 }, Modifiers::NONE);
    for i in 1..30 {
        tool_event(&mut app, ToolEvent::Move { x: 10.0 + f64::from(i) * 4.0, y: 40.0, pressure: 0.8 }, Modifiers::NONE);
    }
    assert!(app.live_stroke.is_some(), "the MyPaint tool strokes live");
    tool_event(&mut app, ToolEvent::Up { x: 126.0, y: 40.0 }, Modifiers::NONE);
    assert_eq!(last_command(&app), "paint.mypaint");
    assert!(alpha(&app, 60, 40) > 0.2, "{}", alpha(&app, 60, 40));
    assert!(alpha(&app, 60, 5) == 0.0);
    assert_eq!(app.session.active().unwrap().history.past_len(), 1);
    app.run("history.undo", json!({})).unwrap();
    assert_eq!(alpha(&app, 60, 40), 0.0);
}

#[test]
fn the_history_brush_paints_from_the_source_chosen_in_the_history_panel() {
    let mut app = app();
    app.run("tools.setBrush", json!({"brush": {"size": 10, "hardness": 1.0, "smoothing": {"amount": 0.0}}})).unwrap();
    app.ui.tool = Tool::Brush;
    crate::paint_mouse::sync_tool_brush(&mut app);
    drag(&mut app, &[(10.0, 20.0), (150.0, 20.0)]);
    app.run("tools.setColors", json!({"foreground": "#ff0000"})).unwrap();
    drag(&mut app, &[(10.0, 20.0), (150.0, 20.0)]);
    // State 1 is the black stroke. Chosen as the source, the History Brush restores black over the red.
    let doc = app.session.active().unwrap().doc.id.0;
    app.ui.history_source = Some((doc, HistorySource::State(1)));
    app.ui.tool = Tool::HistoryBrush;
    crate::paint_mouse::sync_tool_brush(&mut app);
    drag(&mut app, &[(40.0, 20.0), (80.0, 20.0)]);
    assert_eq!(last_command(&app), "paint.historyBrush");
    let p = layer(&app).surface().unwrap().rgba(60, 20);
    assert!(p[0] < 0.1, "black again: {p:?}");
    assert!(layer(&app).surface().unwrap().rgba(130, 20)[0] > 0.9, "red where the brush did not go");
    // A source picked in another document does not carry over.
    app.ui.history_source = Some((doc + 99, HistorySource::State(1)));
    assert_eq!(crate::retouch_ui::history_params(&app, Tool::HistoryBrush), json!({}));
}

#[test]
fn the_art_history_brush_restyles_the_source_through_its_stroke() {
    let mut app = app();
    app.run("tools.setBrush", json!({"brush": {"size": 30, "hardness": 1.0, "smoothing": {"amount": 0.0}}})).unwrap();
    app.ui.tool = Tool::Brush;
    crate::paint_mouse::sync_tool_brush(&mut app);
    app.run("tools.setColors", json!({"foreground": "#0000ff"})).unwrap();
    drag(&mut app, &[(10.0, 40.0), (150.0, 40.0)]);
    app.run("tools.setColors", json!({"foreground": "#ffffff"})).unwrap();
    drag(&mut app, &[(10.0, 40.0), (150.0, 40.0)]);
    let doc = app.session.active().unwrap().doc.id.0;
    app.ui.history_source = Some((doc, HistorySource::State(1)));
    app.ui.tool = Tool::ArtHistoryBrush;
    app.ui.tool_options.art_style = "looseMedium".into();
    app.ui.tool_options.art_fidelity = 100.0;
    app.ui.tool_options.art_area = 30.0;
    crate::paint_mouse::sync_tool_brush(&mut app);
    app.run("tools.setBrush", json!({"brush": {"size": 20, "hardness": 1.0}})).unwrap();
    drag(&mut app, &[(30.0, 40.0), (100.0, 40.0)]);
    assert_eq!(last_command(&app), "paint.artHistoryBrush");
    let p = layer(&app).surface().unwrap().rgba(60, 40);
    assert!(p[2] > 0.9 && p[0] < 0.3, "the blue strokes of state 1: {p:?}");
    let p = crate::retouch_ui::history_params(&app, Tool::ArtHistoryBrush);
    assert_eq!((p["style"].as_str(), p["state"].as_u64()), (Some("looseMedium"), Some(1)));
}
