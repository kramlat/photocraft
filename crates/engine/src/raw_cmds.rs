//! File › Open Photoshop Raw… (`file.openRaw`) and File › Save Photoshop Raw… (`file.saveRaw`):
//! headerless pixel data whose shape the user states. Also File › Open PDF… (`file.openPdf`,
//! `file.pdfInfo`): one PDF page rasterised at a stated resolution.

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

fn pdf_info(_: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path", "file.pdfInfo")?;
    let info = photocraft_io::pdf_import::info(&read_file(path)?).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"pages": info.pages.iter().map(|(w, h)| json!([w, h])).collect::<Vec<_>>()}))
}

fn open_pdf(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path", "file.openPdf")?.to_string();
    let page = int(p, "page", 1).max(1) as usize - 1;
    let dpi = crate::file_cmds::f64_param(p, "resolution").unwrap_or(f64::from(photocraft_io::pdf_import::DEFAULT_DPI)) as f32;
    let bytes = read_file(&path)?;
    let r = photocraft_io::pdf_import::import(&crate::file_cmds::stem(&path), &bytes, page, dpi).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    // A rasterised page is a new picture: Save would not write a PDF back.
    let (i, color) = s.open_document(r.document, None);
    Ok(json!({"document": i, "color": color, "warnings": r.warnings}))
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
        spec!("file.pdfInfo", "PDF Info", &[], None, r##"{"path":str (the PDF) → {pages:[[width,height] in points]}"##, native, pdf_info),
        spec!(
            "file.openPdf",
            "Open PDF…",
            &["File"],
            None,
            r##"{"path":str (the PDF),"page":1..100000=1,"resolution":1..2400=150 (pixels per inch)}"##,
            native,
            open_pdf
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

#[cfg(test)]
mod pdf_tests {
    use super::*;

    /// A one-page PDF (200 x 100 points) with a red rectangle on the left half.
    fn pdf() -> Vec<u8> {
        let content = "1 0 0 rg 0 0 100 100 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        out
    }

    #[test]
    fn a_pdf_page_opens_at_the_stated_resolution() {
        let dir = std::env::temp_dir().join(format!("pc-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("page.pdf");
        std::fs::write(&path, pdf()).unwrap();
        let path = path.to_str().unwrap();
        let mut s = Session::new();
        let info = s.execute("file.pdfInfo", json!({"path": path})).unwrap();
        assert_eq!(info["pages"], json!([[200.0, 100.0]]));
        s.execute("file.openPdf", json!({"path": path, "resolution": 72})).unwrap();
        let d = s.active().unwrap();
        assert_eq!((d.doc.size.width, d.doc.size.height, d.doc.resolution_dpi), (200, 100, 72.0));
        let img = photocraft_io::document_to_image(&d.doc, &mut Vec::new()).unwrap();
        let px = |x: usize| &img.data()[(50 * 200 + x) * img.layout().channels()..][..3];
        assert_eq!(px(20), [255, 0, 0], "the rectangle");
        assert_eq!(px(150), [255, 255, 255], "white paper");
        s.execute("file.close", json!({})).unwrap();
        s.execute("file.openPdf", json!({"path": path, "resolution": 144})).unwrap();
        assert_eq!(s.active().unwrap().doc.size.width, 400);
        assert!(s.execute("file.openPdf", json!({"path": path, "page": 2})).is_err());
        // The generic Open path reads it too.
        let r = photocraft_io::import("page.pdf", &std::fs::read(path).unwrap()).unwrap();
        assert_eq!(r.document.size.width, 417, "150 ppi: 200 pt is 416.7 pixels");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
