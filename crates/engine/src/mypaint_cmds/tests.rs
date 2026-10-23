use super::*;

fn session(w: u64, h: u64, depth: u64, mode: &str) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": w, "height": h, "depth": depth, "mode": mode, "background": "white"})).unwrap();
    s
}

fn px(s: &Session, x: i32, y: i32) -> Vec<f32> {
    let d = s.active().unwrap();
    let id = d.active_layer.unwrap();
    d.doc.layer(id).unwrap().surface().unwrap().read_region(Rect::new(x, y, x + 1, y + 1))
}

fn stroke(extra: Value) -> Value {
    let pts: Vec<Value> = (0..30).map(|i| json!([10.0 + i as f64 * 3.0, 20.0, 0.8, 0, 0, 0, i as f64 * 10.0])).collect();
    let mut p = json!({"points": pts, "color": "#ff0000", "seed": 5});
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    p
}

#[test]
fn the_library_lists_the_embedded_mypaint_set_in_groups() {
    let mut s = Session::new();
    let v = s.execute("mypaint.brushes.list", json!({})).unwrap();
    let groups = v["groups"].as_array().unwrap();
    assert!(groups.iter().any(|g| g["name"] == "Classic"), "{v}");
    let total: usize = groups.iter().map(|g| g["brushes"].as_array().unwrap().len()).sum();
    assert!(total >= 190, "{total}");
    assert_eq!(v["current"], "classic/brush");
    let r = s.execute("mypaint.brushes.select", json!({"brush": "Pencil"})).unwrap();
    assert!(r["brush"].as_str().unwrap().ends_with("pencil"), "{r}");
    assert!(s.execute("mypaint.brushes.select", json!({"brush": "no such brush"})).is_err());
}

#[test]
fn a_stroke_paints_in_the_chosen_colour_and_is_one_history_step() {
    let mut s = session(120, 40, 8, "rgb");
    s.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil", "size": 8}))).unwrap();
    let p = px(&s, 50, 20);
    assert!(p[0] > 0.9 && p[1] < 0.5 && p[2] < 0.5, "{p:?}");
    assert!(px(&s, 50, 5).iter().all(|v| *v > 0.99), "outside the stroke stays white");
    s.execute("history.undo", json!({})).unwrap();
    assert!(px(&s, 50, 20).iter().all(|v| *v > 0.99));
}

#[test]
fn the_same_params_paint_the_same_pixels_and_the_seed_changes_random_brushes() {
    let run = |seed: u64, brush: &str| {
        let mut s = session(120, 40, 8, "rgb");
        s.execute("paint.mypaint", stroke(json!({"brush": brush, "size": 14, "seed": seed}))).unwrap();
        s.active().unwrap().doc.layers[0].surface().unwrap().read_region(Rect::new(0, 0, 120, 40))
    };
    assert_eq!(run(1, "classic/pencil"), run(1, "classic/pencil"));
    // A brush that scatters its dabs (offset by random) paints differently for another seed.
    assert_ne!(run(1, "experimental/fur"), run(2, "experimental/fur"));
}

#[test]
fn size_opacity_erase_and_the_selection_are_honoured() {
    let mut s = session(120, 40, 8, "rgb");
    s.execute("paint.mypaint", stroke(json!({"brush": "classic/pen", "size": 4}))).unwrap();
    let thin = px(&s, 60, 26)[1];
    s.execute("history.undo", json!({})).unwrap();
    s.execute("paint.mypaint", stroke(json!({"brush": "classic/pen", "size": 30}))).unwrap();
    assert!(px(&s, 60, 26)[1] < thin, "a bigger brush reaches farther from the line");
    s.execute("history.undo", json!({})).unwrap();
    // Selection: nothing lands outside it.
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 60, "height": 40})).unwrap();
    s.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil", "size": 8}))).unwrap();
    assert!(px(&s, 30, 20)[1] < 0.6, "{:?}", px(&s, 30, 20));
    assert!(px(&s, 90, 20).iter().all(|v| *v > 0.99), "outside the selection is untouched");
    s.execute("select.deselect", json!({})).unwrap();
    // The eraser option takes paint away (on a layer that can lose it: a Background cannot).
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil", "size": 8, "color": "#ff0000"}))).unwrap();
    let painted = px(&s, 50, 20)[3];
    assert!(painted > 0.2, "{painted}");
    s.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil", "size": 8, "erase": true}))).unwrap();
    assert!(px(&s, 50, 20)[3] < painted - 0.05, "{:?}", px(&s, 50, 20));
}

#[test]
fn smudge_brushes_drag_existing_colour_along() {
    let mut s = session(120, 40, 8, "rgb");
    let blue: Vec<Value> = (0..30).map(|i| json!([i as f64 * 1.5, 20.0, 0.5, 0, 0, 0, i as f64 * 10.0])).collect();
    s.execute("paint.mypaint", json!({"points": blue, "brush": "classic/brush", "size": 24, "color": "#0000ff", "seed": 1})).unwrap();
    assert!(px(&s, 20, 20)[2] > 0.9 && px(&s, 20, 20)[0] < 0.5, "the blue stroke: {:?}", px(&s, 20, 20));
    assert!(px(&s, 90, 20).iter().all(|v| *v > 0.99), "nothing painted there yet");
    // Drag from the blue towards the right with a smudge brush.
    let pts: Vec<Value> = (0..40).map(|i| json!([30.0 + i as f64 * 2.0, 20.0, 0.9, 0, 0, 0, i as f64 * 10.0])).collect();
    s.execute("paint.mypaint", json!({"points": pts, "brush": "classic/smudge", "size": 20, "seed": 2})).unwrap();
    let far = px(&s, 72, 20);
    assert!(far[0] < 0.93 || far[1] < 0.93, "some blue was carried to the right: {far:?}");
}

#[test]
fn grayscale_16_bit_and_mask_targets_work_and_cmyk_is_refused() {
    let mut g = session(80, 30, 16, "grayscale");
    g.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil", "size": 8, "color": "#000000"}))).unwrap();
    assert!(px(&g, 40, 20)[0] < 0.2, "{:?}", px(&g, 40, 20));
    let mut m = session(80, 30, 8, "rgb");
    m.execute("layer.layerMask.revealAll", json!({})).unwrap();
    let r = m.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil", "size": 8, "color": "#000000", "target": "mask"})));
    assert!(r.is_ok(), "{r:?}");
    let mut c = session(80, 30, 8, "cmyk");
    assert!(c.execute("paint.mypaint", stroke(json!({"brush": "classic/pencil"}))).is_err());
}

#[test]
fn bad_input_is_an_error_not_a_crash() {
    let mut s = session(60, 30, 8, "rgb");
    assert!(s.execute("paint.mypaint", json!({"points": []})).is_err());
    assert!(s.execute("paint.mypaint", json!({"points": [[1, 2]], "brush": "nope"})).is_err());
    assert!(s.execute("paint.mypaint", json!({"points": [[1, 2]], "size": 99999})).is_err());
    assert!(s.execute("paint.mypaint", json!({"points": [[f64::NAN, 2.0]]})).is_err());
    assert!(s.execute("paint.mypaint", json!({"points": [[1, 2]], "myb": "{not json"})).is_err());
    assert!(s.execute("paint.mypaint", json!({"points": [[1, 2]], "myb": "{\"version\": 2, \"settings\": {}}"})).is_err());
    // Huge off-canvas coordinates and 0-radius sizes are survivable.
    s.execute("paint.mypaint", json!({"points": [[-1.0e6, 5.0, 1.0, 0], [1.0e6, 5.0, 1.0, 10]], "size": 1})).unwrap();
}

#[test]
fn importing_a_brush_adds_and_selects_it() {
    let mut s = Session::new();
    let dir = std::env::temp_dir().join(format!("pc-mypaint-{}", std::process::id()));
    s.mypaint.user_dir = Some(dir.clone());
    let text = r#"{"version": 3, "settings": {"radius_logarithmic": {"base_value": 1.0, "inputs": {}}, "opaque": {"base_value": 1.0, "inputs": {}}, "hardness": {"base_value": 1.0, "inputs": {}}}}"#;
    let r = s.execute("mypaint.brushes.import", json!({"data": text, "name": "Test Brush"})).unwrap();
    assert_eq!(r["brush"], "user/Test Brush");
    assert!((r["size"].as_f64().unwrap() - 2.0 * 1.0f64.exp()).abs() < 1e-4, "{r}");
    assert!(s.execute("mypaint.brushes.import", json!({"data": "{}"})).is_err());
    assert!(dir.join("user/Test Brush.myb").exists(), "kept for the next run");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_live_stroke_matches_the_committed_one() {
    let mut s = session(120, 40, 8, "rgb");
    let p = stroke(json!({"brush": "classic/pencil", "size": 8}));
    let pts: Vec<photocraft_paint::StrokePoint> = parse_points(&p, "t").unwrap();
    s.execute("mypaint.brushes.list", json!({})).unwrap();
    let mut live = LiveMyPaint::begin(&s, &p).unwrap();
    for chunk in pts.chunks(7) {
        live.push(chunk).unwrap();
    }
    let shown = live.doc.layers[0].surface().unwrap().read_region(Rect::new(0, 0, 120, 40));
    s.execute("paint.mypaint", p).unwrap();
    let committed = s.active().unwrap().doc.layers[0].surface().unwrap().read_region(Rect::new(0, 0, 120, 40));
    assert_eq!(shown, committed);
}

#[test]
fn every_embedded_brush_loads_and_paints_without_trouble() {
    let mut s = session(160, 80, 8, "rgb");
    let v = s.execute("mypaint.brushes.list", json!({})).unwrap();
    let ids: Vec<String> = v["groups"].as_array().unwrap().iter().flat_map(|g| g["brushes"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap().to_string())).collect();
    assert!(ids.len() >= 190);
    for id in ids {
        let pts: Vec<Value> = (0..24).map(|i| json!([10.0 + i as f64 * 6.0, 40.0 + 15.0 * (i as f64 / 4.0).sin(), 0.2 + 0.03 * i as f64, 10.0, -10.0, 0, i as f64 * 12.0])).collect();
        s.execute("paint.mypaint", json!({"points": pts, "brush": id, "size": 12, "seed": 3, "color": "#336699"})).unwrap_or_else(|e| panic!("{id}: {e}"));
        let d = s.active().unwrap();
        let data = d.doc.layers[0].surface().unwrap().read_region(Rect::new(0, 0, 160, 80));
        assert!(data.iter().all(|v| v.is_finite()), "{id}: non-finite pixels");
        s.execute("history.undo", json!({})).unwrap();
    }
}

#[test]
fn every_embedded_brush_has_the_packs_own_128_px_preview() {
    let mut lib = Library::default();
    lib.ensure();
    let ids: Vec<String> = lib.entries.iter().map(|e| e.id.clone()).collect();
    assert!(ids.len() >= 190);
    // The embedded table covers every embedded brush (a system pack found on disk may take over an entry, with its own files).
    for (id, _, _) in embedded::BRUSHES {
        let (_, bytes) = embedded_previews::PREVIEWS.iter().find(|(k, _)| k == id).unwrap_or_else(|| panic!("{id} has no embedded preview"));
        assert!(preview::decode(bytes).is_some(), "{id}");
    }
    for id in &ids {
        let p = lib.preview(id).unwrap();
        assert_eq!((p.width, p.height), (128, 128), "{id}");
        assert_eq!(p.rgba.len(), 128 * 128 * 4);
    }
}

#[test]
fn a_preview_is_made_once_and_unknown_brushes_have_none() {
    let mut lib = Library::default();
    let a = lib.preview("classic/pencil").unwrap();
    let b = lib.preview("classic/pencil").unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert!(lib.preview("no/such").is_none());
}

#[test]
fn the_engine_draws_a_preview_for_a_brush_without_one() {
    let mut inked = 0;
    let mut lib = Library::default();
    lib.ensure();
    let texts: Vec<String> = lib.entries.iter().map(|e| lib.text(e).unwrap()).collect();
    for text in &texts {
        let p = preview::render(text).expect("every shipped brush draws");
        assert_eq!((p.width, p.height), (128, 128));
        if p.rgba.chunks_exact(4).any(|px| px[..3].iter().any(|c| c.abs_diff(245) > 12)) {
            inked += 1;
        }
    }
    // Erasers and smudges have nothing to move on a blank page; every other brush marks it.
    assert!(inked * 10 >= texts.len() * 8, "{inked} of {} brushes marked the page", texts.len());
}

#[test]
fn a_preview_png_beside_a_brush_file_is_the_brushs_thumbnail() {
    let dir = std::env::temp_dir().join(format!("pc-mypaint-prev-{}", std::process::id()));
    let group = dir.join("mine");
    std::fs::create_dir_all(&group).unwrap();
    std::fs::write(group.join("dot.myb"), include_str!("../../../../assets/mypaint-brushes/classic/pencil.myb")).unwrap();
    std::fs::write(group.join("dot_prev.png"), include_bytes!("../../../../assets/mypaint-brushes/classic/brush_prev.png")).unwrap();
    std::fs::write(group.join("plain.myb"), include_str!("../../../../assets/mypaint-brushes/classic/pencil.myb")).unwrap();
    let mut lib = Library { user_dir: Some(dir.clone()), ..Library::default() };
    lib.ensure();
    let with = lib.entries.iter().find(|e| e.id == "mine/dot").unwrap();
    assert!(matches!(with.preview, PreviewSrc::File(_)));
    let without = lib.entries.iter().find(|e| e.id == "mine/plain").unwrap();
    assert!(matches!(without.preview, PreviewSrc::None));
    let a = lib.preview("mine/dot").unwrap();
    let want = preview::decode(include_bytes!("../../../../assets/mypaint-brushes/classic/brush_prev.png")).unwrap();
    assert_eq!(*a, want);
    assert_eq!((lib.preview("mine/plain").unwrap().width, 128), (128, 128));
    std::fs::remove_dir_all(&dir).ok();
}
