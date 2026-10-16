use photocraft_engine::Session;
use serde_json::json;

use super::*;

fn run(src: &str) -> (Session, Result<Outcome, String>) {
    let mut s = Session::new();
    let r = PythonFu::new().run(&mut s, src);
    (s, r)
}

#[test]
fn plain_python_runs_and_prints() {
    let (_, r) = run("x = [i * i for i in range(4)]\nprint('squares', x)\nx");
    let out = r.unwrap();
    assert_eq!(out.output, "squares [0, 1, 4, 9]\n");
    assert_eq!(out.value, "[0, 1, 4, 9]");
}

#[test]
fn a_gimpfu_style_script_builds_an_image() {
    let (mut s, r) = run(
        r#"
from gimpfu import *

def make_it(width, height):
    img = pdb.gimp_image_new(width, height, RGB)
    layer = pdb.gimp_layer_new(img, width, height, RGBA_IMAGE, "Blue box", 100, NORMAL_MODE)
    pdb.gimp_image_insert_layer(img, layer, None, 0)
    pdb.gimp_context_set_foreground((0, 0, 255))
    pdb.gimp_image_select_rectangle(img, CHANNEL_OP_REPLACE, 2, 2, 6, 6)
    pdb.gimp_drawable_fill(layer, FOREGROUND_FILL)
    pdb.gimp_selection_none(img)
    return pdb.gimp_image_width(img)

make_it(16, 12)
"#,
    );
    assert_eq!(r.unwrap().value, "16");
    let names: Vec<String> = s.execute("document.inspect", json!({})).unwrap()["layers"].as_array().unwrap().iter().filter_map(|l| l["name"].as_str().map(str::to_string)).collect();
    assert!(names.contains(&"Blue box".to_string()), "{names:?}");
}

#[test]
fn registered_scripts_are_listed_and_run() {
    let mut s = Session::new();
    let mut py = PythonFu::new();
    py.run(
        &mut s,
        r#"
from gimpfu import *

def hello(name, times):
    gimp.message(name * times)

register("python-fu-hello", "Says hello", "help", "me", "me", "2026",
         "<Image>/Filters/Render/Hello...", "*",
         [(PF_STRING, "name", "Name", "hi"), (PF_INT, "times", "Times", 2)], [], hello)
main()
"#,
    )
    .unwrap();
    let list = py.registered();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].name.as_str(), list[0].label.as_str(), list[0].menu.as_deref()), ("python-fu-hello", "Hello...", Some("<Image>/Filters/Render")));
    assert_eq!(list[0].params.len(), 2);
    let out = py.run_registered(&mut s, "python-fu-hello", vec![Value::string("ab"), Value::Int(2)]).unwrap();
    assert_eq!(out.output, "abab\n");
}

#[test]
fn errors_come_back_as_text_and_documents_survive() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 4, "height": 4})).unwrap();
    let mut py = PythonFu::new();
    for src in ["1/0", "pdb.gimp_image_width(99)", "undefined_name", "def broken(:", "pdb.no_such_procedure()", "import os"] {
        let e = py.run(&mut s, src).unwrap_err();
        assert!(!e.is_empty(), "{src}");
    }
    assert_eq!(s.documents().len(), 1);
    assert!(py.run(&mut s, "pdb.gimp_image_list()").is_ok());
}

#[test]
fn definitions_persist_between_runs() {
    let mut s = Session::new();
    let mut py = PythonFu::new();
    py.run(&mut s, "counter = 41").unwrap();
    assert_eq!(py.run(&mut s, "counter + 1").unwrap().value, "42");
}

#[test]
fn scripts_cannot_open_files_or_import_modules() {
    let mut s = Session::new();
    let mut py = PythonFu::new();
    for src in ["open('/etc/passwd').read()", "__import__('os').listdir('/')", "import os", "import posix", "import _io", "import subprocess", "import socket", "import ctypes", "exec('1')", "eval('1+1')", "input()"] {
        let r = py.run(&mut s, src);
        assert!(r.is_err(), "{src} -> {r:?}");
    }
    // The language itself and the procedure database still work afterwards.
    assert_eq!(py.run(&mut s, "sum(range(5))").unwrap().value, "10");
    assert!(py.run(&mut s, "pdb.gimp_image_list()").is_ok());
}
