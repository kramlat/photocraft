use photocraft_engine::Session;
use serde_json::json;

use super::*;

fn run(src: &str) -> (Session, Result<Outcome, String>) {
    let mut s = Session::new();
    let r = ScriptFu::new().run(&mut s, src);
    (s, r)
}

#[test]
fn a_classic_script_builds_an_image_with_a_filled_layer() {
    let (mut s, r) = run(
        r#"
        (define (make-it width height)
          (let* ((img (car (gimp-image-new width height RGB)))
                 (layer (car (gimp-layer-new img width height RGBA-IMAGE "Red box" 100 NORMAL-MODE))))
            (gimp-image-insert-layer img layer 0 0)
            (gimp-context-set-foreground '(255 0 0))
            (gimp-image-select-rectangle img CHANNEL-OP-REPLACE 2 2 6 6)
            (gimp-drawable-fill layer FOREGROUND-FILL)
            (gimp-selection-none img)
            (gimp-message "done")
            (list (car (gimp-image-width img)) (car (gimp-image-height img)))))
        (make-it 16 12)"#,
    );
    let out = r.unwrap();
    assert_eq!(out.value, "(16 12)");
    assert_eq!(out.output, "done\n");
    let px = s.execute("document.pixel", json!({"x": 4, "y": 4})).unwrap();
    assert!(px.to_string().contains("255") || px.to_string().contains("1.0"), "{px}");
    let layers = s.execute("document.inspect", json!({})).unwrap()["layers"].as_array().unwrap().iter().filter_map(|l| l["name"].as_str().map(str::to_string)).collect::<Vec<_>>();
    assert!(layers.contains(&"Red box".to_string()), "{layers:?}");
}

#[test]
fn filters_run_on_the_chosen_drawable() {
    let mut s = Session::new();
    let mut sf = ScriptFu::new();
    sf.run(
        &mut s,
        r#"(define img (car (gimp-image-new 20 20 RGB)))
           (define l (car (gimp-layer-new img 20 20 RGBA-IMAGE "L" 100 NORMAL-MODE)))
           (gimp-context-set-foreground '(40 90 200))
           (gimp-image-select-rectangle img CHANNEL-OP-REPLACE 0 0 10 20)
           (gimp-drawable-fill l FOREGROUND-FILL)
           (gimp-selection-all img)"#,
    )
    .unwrap();
    let before = s.execute("document.pixel", json!({"x": 9, "y": 10})).unwrap();
    sf.run(&mut s, "(plug-in-edge RUN-NONINTERACTIVE img l 2.0 0 0)").unwrap();
    assert_ne!(s.execute("document.pixel", json!({"x": 9, "y": 10})).unwrap(), before);
    // The interpreter keeps its definitions between runs.
    assert_eq!(sf.run(&mut s, "(car (gimp-image-width img))").unwrap().value, "20");
}

#[test]
fn errors_come_back_and_the_session_survives() {
    for src in ["(gimp-image-width 99)", "(gimp-layer-new 1 2 3)", "(gimp-context-set-foreground 5)", "(car '())", "(gimp-no-such-thing)", "(plug-in-gauss 1 1 1)"] {
        let (s, r) = run(src);
        assert!(r.is_err(), "{src}");
        let _ = s.documents().len();
    }
    let (s, r) = run("(gimp-image-new 8 8 RGB) (car '())");
    assert!(r.is_err());
    assert_eq!(s.documents().len(), 1, "work done before the error stays");
}

#[test]
fn registered_scripts_are_listed_with_their_parameters() {
    let mut s = Session::new();
    let mut sf = ScriptFu::new();
    sf.run(
        &mut s,
        r#"(define (script-fu-hello text size) (gimp-message (string-append text "!")))
           (script-fu-register "script-fu-hello" "<Image>/Filters/Render/Hello…" "Says hello" "me" "me" "2026" ""
             SF-STRING "Text" "hi"
             SF-ADJUSTMENT "Size" '(10 1 100 1 10 0 0))
           "#,
    )
    .unwrap();
    let list = sf.registered();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].name.as_str(), list[0].label.as_str(), list[0].menu.as_deref()), ("script-fu-hello", "Hello…", Some("<Image>/Filters/Render")));
    assert_eq!(list[0].params.iter().map(|p| (p.kind, p.label.as_str())).collect::<Vec<_>>(), [(ParamKind::String, "Text"), (ParamKind::Adjustment, "Size")]);
    let out = sf.run_registered(&mut s, "script-fu-hello", vec![Value::string("hey"), Value::Int(3)]).unwrap();
    assert_eq!(out.output, "hey!\n");
}

#[test]
fn runaway_scripts_are_stopped() {
    let (_, r) = run("(define (spin) (spin)) (spin)");
    assert!(r.unwrap_err().contains("step budget"));
}

#[test]
fn documents_survive_a_script_that_fails_midway() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
    let mut sf = ScriptFu::new();
    assert!(sf.run(&mut s, "(gimp-image-new 4 4 RGB) (error \"stop\")").is_err());
    assert_eq!(s.documents().len(), 2, "the original document and the script's image");
    assert!(sf.run(&mut s, "(car (gimp-image-list))").is_ok());
}
