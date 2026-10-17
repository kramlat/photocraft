//! File › Open As Photoshop Raw… (`file.openRaw`) and File › Save As Photoshop Raw…
//! (`file.saveRaw`): headerless pixel data whose shape the user states.

use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::file_cmds::{native, native_doc, read_file, str_param, write_file};
use crate::{EngineError, Result, Session};

fn int(p: &Value, key: &str, default: u64) -> u64 {
    p.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn mac(p: &Value) -> bool {
    p.get("byteOrder").and_then(Value::as_str) == Some("mac")
}

fn open_raw(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path", "file.openRaw")?.to_string();
    let params = photocraft_codecs::RawParams {
        width: int(p, "width", 1) as u32,
        height: int(p, "height", 1) as u32,
        channels: int(p, "channels", 3) as u8,
        depth: int(p, "depth", 8) as u8,
        big_endian: mac(p),
        interleaved: p.get("interleaved").and_then(Value::as_bool).unwrap_or(true),
        header: int(p, "header", 0) as usize,
    };
    let bytes = read_file(&path)?;
    let name = crate::file_cmds::stem(&path);
    let r = photocraft_io::import_raw(&name, &bytes, &params).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    // Not the document's save path: saving would overwrite the raw data with a PSD.
    let (i, color) = s.open_document(r.document, None);
    Ok(json!({"document": i, "color": color, "warnings": r.warnings}))
}

fn save_raw(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path", "file.saveRaw")?.to_string();
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let interleaved = p.get("interleaved").and_then(Value::as_bool).unwrap_or(true);
    let (bytes, warnings) = photocraft_io::export_raw(&d.doc, mac(p), interleaved).map_err(|e| EngineError::Other(e.to_string()))?;
    write_file(&path, &bytes)?;
    Ok(json!({"path": path, "bytes": bytes.len(), "warnings": warnings}))
}

pub fn specs() -> Vec<CommandSpec> {
    macro_rules! spec {
        ($id:expr, $label:expr, $menu:expr, $sc:expr, $params:expr, $enabled:expr, $run:expr) => {
            CommandSpec { id: $id, label: $label, menu: $menu, shortcut: $sc, params: $params, enabled: $enabled, journal: true, run: $run }
        };
    }
    vec![
        spec!(
            "file.openRaw",
            "Open Photoshop Raw…",
            &["File"],
            None,
            r##"{"path":str (the raw data file),"width":1..30000=1,"height":1..30000=1,"channels":1..4=3 (1 gray, 2 gray and alpha, 3 RGB, 4 RGBA),"depth":8|16=8,"byteOrder":"ibm|mac"="ibm" (16-bit: IBM PC is least significant byte first),"interleaved":bool=true,"header":0..1000000000=0 (bytes to skip)}"##,
            native,
            open_raw
        ),
        spec!(
            "file.saveRaw",
            "Save Photoshop Raw…",
            &["File"],
            None,
            r##"{"path":str,"byteOrder":"ibm|mac"="ibm","interleaved":bool=true}"##,
            native_doc,
            save_raw
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_round_trips_through_a_file_in_every_arrangement() {
        let dir = std::env::temp_dir().join(format!("pc-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 5, "height": 3, "background": "#336699"})).unwrap();
        s.execute("select.rect", json!({"x": 1, "y": 0, "width": 2, "height": 2})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff8000"})).unwrap();
        let want = s.active().unwrap().doc.layers.len();
        for (order, interleaved) in [("ibm", true), ("mac", true), ("ibm", false), ("mac", false)] {
            let path = dir.join(format!("a-{order}-{interleaved}.raw"));
            let path = path.to_str().unwrap();
            let r = s.execute("file.saveRaw", json!({"path": path, "byteOrder": order, "interleaved": interleaved})).unwrap();
            assert_eq!(r["bytes"], 5 * 3 * 3, "8-bit RGB");
            let before = s.active().unwrap().doc.clone();
            s.execute("file.openRaw", json!({"path": path, "width": 5, "height": 3, "channels": 3, "byteOrder": order, "interleaved": interleaved})).unwrap();
            let after = s.active().unwrap();
            assert_eq!((after.doc.size.width, after.doc.size.height, after.doc.layers.len()), (5, 3, want.min(1)));
            let a = photocraft_io::document_to_image(&before, &mut Vec::new()).unwrap();
            let b = photocraft_io::document_to_image(&after.doc, &mut Vec::new()).unwrap();
            assert_eq!(a.data(), b.data(), "{order} interleaved={interleaved}");
            s.execute("file.close", json!({})).unwrap();
        }
        // Too little data and a header offset.
        let path = dir.join("short.raw");
        std::fs::write(&path, [0u8; 10]).unwrap();
        let e = s.execute("file.openRaw", json!({"path": path.to_str().unwrap(), "width": 5, "height": 3})).unwrap_err();
        assert!(e.to_string().contains("needs"), "{e}");
        std::fs::write(&path, [9u8, 9, 9, 1, 2, 3]).unwrap();
        s.execute("file.openRaw", json!({"path": path.to_str().unwrap(), "width": 1, "height": 1, "header": 3})).unwrap();
        let img = photocraft_io::document_to_image(&s.active().unwrap().doc, &mut Vec::new()).unwrap();
        assert_eq!(&img.data()[..3], [1, 2, 3]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
