//! The procedure database: GIMP-compatible `gimp-*` names mapped onto PhotoCraft commands.
//!
//! Every procedure returns a list of its results (`(car (gimp-image-new 64 64 RGB))`), as Script-Fu
//! does. Image and drawable numbers are small integers handed out per interpreter; they map to a
//! document and a layer of the session.

use std::cell::RefCell;
use std::rc::Rc;

use photocraft_engine::Session;
use photocraft_scheme::{Error, Host, Interp, Result, Value};
use serde_json::{Value as Json, json};

use crate::registry::{ParamKind, Registered, parse_register};

/// Layer modes by Script-Fu constant, in PhotoCraft's names.
const MODES: &[(&str, &str)] = &[
    ("NORMAL-MODE", "Normal"),
    ("DISSOLVE-MODE", "Dissolve"),
    ("MULTIPLY-MODE", "Multiply"),
    ("SCREEN-MODE", "Screen"),
    ("OVERLAY-MODE", "Overlay"),
    ("DIFFERENCE-MODE", "Difference"),
    ("ADDITION-MODE", "Linear Dodge"),
    ("SUBTRACT-MODE", "Subtract"),
    ("DARKEN-ONLY-MODE", "Darken"),
    ("LIGHTEN-ONLY-MODE", "Lighten"),
    ("HUE-MODE", "Hue"),
    ("SATURATION-MODE", "Saturation"),
    ("COLOR-MODE", "Color"),
    ("VALUE-MODE", "Luminosity"),
    ("DIVIDE-MODE", "Divide"),
    ("DODGE-MODE", "Color Dodge"),
    ("BURN-MODE", "Color Burn"),
    ("HARDLIGHT-MODE", "Hard Light"),
    ("SOFTLIGHT-MODE", "Soft Light"),
];

/// Simple enumerations: (name, value).
const ENUMS: &[(&str, i64)] = &[
    ("RGB", 0),
    ("GRAY", 1),
    ("INDEXED", 2),
    ("RGB-IMAGE", 0),
    ("RGBA-IMAGE", 1),
    ("GRAY-IMAGE", 2),
    ("GRAYA-IMAGE", 3),
    ("FOREGROUND-FILL", 0),
    ("BACKGROUND-FILL", 1),
    ("WHITE-FILL", 2),
    ("TRANSPARENT-FILL", 3),
    ("PATTERN-FILL", 4),
    ("CHANNEL-OP-ADD", 0),
    ("CHANNEL-OP-SUBTRACT", 1),
    ("CHANNEL-OP-REPLACE", 2),
    ("CHANNEL-OP-INTERSECT", 3),
    ("RUN-INTERACTIVE", 0),
    ("RUN-NONINTERACTIVE", 1),
    ("RUN-WITH-LAST-VALS", 2),
    ("CLIP-TO-IMAGE", 0),
    ("TRUE-1", 1),
];

#[derive(Default)]
pub struct Shared {
    pub session: Option<Session>,
    pub output: String,
    pub registered: Vec<Registered>,
    /// `images[n - 1]` is the document id of image number `n`.
    images: Vec<u64>,
    /// `drawables[n - 1]` is (document id, layer id) of drawable number `n`.
    drawables: Vec<(u64, u64)>,
    foreground: Option<String>,
    background: Option<String>,
}

pub struct Pdb(pub Rc<RefCell<Shared>>);

fn bad(name: &str, why: impl std::fmt::Display) -> Error {
    Error::msg(format!("{name}: {why}"))
}

fn ok(values: impl IntoIterator<Item = Value, IntoIter: DoubleEndedIterator>) -> Option<Result<Value>> {
    Some(Ok(Value::list(values)))
}

fn done() -> Option<Result<Value>> {
    ok([Value::Bool(true)])
}

fn int(name: &str, args: &[Value], i: usize) -> Result<i64> {
    args.get(i).and_then(Value::as_int).ok_or_else(|| bad(name, format!("argument {} must be an integer", i + 1)))
}

fn float(name: &str, args: &[Value], i: usize) -> Result<f64> {
    args.get(i).and_then(Value::as_f64).ok_or_else(|| bad(name, format!("argument {} must be a number", i + 1)))
}

fn string(name: &str, args: &[Value], i: usize) -> Result<String> {
    args.get(i).and_then(Value::as_string).ok_or_else(|| bad(name, format!("argument {} must be a string", i + 1)))
}

/// `(r g b)` with 0–255 components, `#rrggbb`, or a colour name list, as `#rrggbb`.
fn color(name: &str, v: Option<&Value>) -> Result<String> {
    let v = v.ok_or_else(|| bad(name, "missing colour"))?;
    if let Some(s) = v.as_string() {
        return if s.len() == 7 && s.starts_with('#') { Ok(s) } else { Err(bad(name, "colour strings must be #rrggbb")) };
    }
    let parts = v.to_vec()?;
    let c = |i: usize| parts.get(i).and_then(Value::as_f64).map(|x| x.round().clamp(0.0, 255.0) as u8).ok_or_else(|| bad(name, "colours are (r g b) lists"));
    Ok(format!("#{:02x}{:02x}{:02x}", c(0)?, c(1)?, c(2)?))
}

fn hex_to_list(h: &str) -> Value {
    let c = |i: usize| h.get(i..i + 2).and_then(|x| i64::from_str_radix(x, 16).ok()).unwrap_or(0);
    Value::list([Value::Int(c(1)), Value::Int(c(3)), Value::Int(c(5))])
}

impl Shared {
    fn session(&mut self, name: &str) -> Result<&mut Session> {
        self.session.as_mut().ok_or_else(|| bad(name, "no session"))
    }

    fn run(&mut self, name: &str, id: &str, params: Json) -> Result<Json> {
        self.session(name)?.execute(id, params).map_err(|e| bad(name, e))
    }

    /// Makes image `n` the active document and returns its document index.
    fn activate(&mut self, name: &str, n: i64) -> Result<usize> {
        let doc_id = usize::try_from(n - 1).ok().and_then(|i| self.images.get(i).copied()).ok_or_else(|| bad(name, format!("no image {n}")))?;
        let session = self.session(name)?;
        let index = session.documents().iter().position(|d| d.doc.id.0 == doc_id).ok_or_else(|| bad(name, format!("image {n} was closed")))?;
        session.execute("document.activate", json!({"document": index})).map_err(|e| bad(name, e))?;
        Ok(index)
    }

    fn image_number(&mut self, doc_id: u64) -> i64 {
        match self.images.iter().position(|d| *d == doc_id) {
            Some(i) => i as i64 + 1,
            None => {
                self.images.push(doc_id);
                self.images.len() as i64
            }
        }
    }

    fn drawable_number(&mut self, doc: u64, layer: u64) -> i64 {
        match self.drawables.iter().position(|d| *d == (doc, layer)) {
            Some(i) => i as i64 + 1,
            None => {
                self.drawables.push((doc, layer));
                self.drawables.len() as i64
            }
        }
    }

    fn active_doc_id(&mut self, name: &str) -> Result<u64> {
        let session = self.session(name)?;
        session.active().map(|s| s.doc.id.0).ok_or_else(|| bad(name, "no image is open"))
    }

    /// Selects drawable `n` (activating its image) and returns its layer id.
    fn select_drawable(&mut self, name: &str, n: i64) -> Result<u64> {
        let (doc, layer) = usize::try_from(n - 1).ok().and_then(|i| self.drawables.get(i).copied()).ok_or_else(|| bad(name, format!("no drawable {n}")))?;
        let image = self.image_number(doc);
        self.activate(name, image)?;
        self.run(name, "layer.select", json!({"layer": layer}))?;
        Ok(layer)
    }

    fn size(&mut self, name: &str) -> Result<(i64, i64)> {
        let info = self.run(name, "document.inspect", json!({}))?;
        let get = |k: &str| info.get(k).and_then(Json::as_i64).unwrap_or(0);
        Ok((get("width"), get("height")))
    }

    fn set_colors(&mut self, name: &str) -> Result<()> {
        let (fg, bg) = (self.foreground.clone(), self.background.clone());
        let mut p = json!({});
        if let Some(c) = fg {
            p["foreground"] = json!(c);
        }
        if let Some(c) = bg {
            p["background"] = json!(c);
        }
        self.run(name, "tools.setColors", p).map(|_| ())
    }

    pub(crate) fn call(&mut self, name: &str, a: &[Value]) -> Option<Result<Value>> {
        match self.call_inner(name, a) {
            Ok(v) => v,
            Err(e) => Some(Err(e)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn call_inner(&mut self, name: &str, a: &[Value]) -> Result<Option<Result<Value>>> {
        Ok(match name {
            // ---- images
            "gimp-image-new" => {
                let (w, h, t) = (int(name, a, 0)?, int(name, a, 1)?, int(name, a, 2)?);
                let mode = if t == 1 { "gray" } else { "rgb" };
                let r = self.run(name, "file.new", json!({"width": w, "height": h, "mode": mode, "background": "transparent"}))?;
                let _ = r;
                let id = self.active_doc_id(name)?;
                ok([Value::Int(self.image_number(id))])
            }
            "gimp-image-list" => {
                let ids: Vec<u64> = self.session(name)?.documents().iter().map(|d| d.doc.id.0).collect();
                let numbers: Vec<Value> = ids.into_iter().map(|id| Value::Int(self.image_number(id))).collect();
                ok([Value::Int(numbers.len() as i64), Value::vector(numbers)])
            }
            "gimp-image-width" | "gimp-image-height" => {
                self.activate(name, int(name, a, 0)?)?;
                let (w, h) = self.size(name)?;
                ok([Value::Int(if name.ends_with("width") { w } else { h })])
            }
            "gimp-image-scale" => {
                self.activate(name, int(name, a, 0)?)?;
                self.run(name, "image.imageSize", json!({"width": int(name, a, 1)?, "height": int(name, a, 2)?}))?;
                done()
            }
            "gimp-image-resize" => {
                self.activate(name, int(name, a, 0)?)?;
                self.run(name, "image.canvasSize", json!({"width": int(name, a, 1)?, "height": int(name, a, 2)?, "anchor": "topLeft"}))?;
                done()
            }
            "gimp-image-flatten" => {
                self.activate(name, int(name, a, 0)?)?;
                self.run(name, "layer.flattenImage", json!({}))?;
                let doc = self.active_doc_id(name)?;
                let info = self.run(name, "document.inspect", json!({}))?;
                let layer = info.get("activeLayer").and_then(Json::as_u64).unwrap_or(0);
                ok([Value::Int(self.drawable_number(doc, layer))])
            }
            "gimp-image-delete" => {
                self.activate(name, int(name, a, 0)?)?;
                self.run(name, "file.close", json!({"discard": true}))?;
                done()
            }
            "gimp-image-get-active-layer" | "gimp-image-get-active-drawable" => {
                self.activate(name, int(name, a, 0)?)?;
                let doc = self.active_doc_id(name)?;
                let info = self.run(name, "document.inspect", json!({}))?;
                let layer = info.get("activeLayer").and_then(Json::as_u64).ok_or_else(|| bad(name, "the image has no layer"))?;
                ok([Value::Int(self.drawable_number(doc, layer))])
            }
            "gimp-image-insert-layer" | "gimp-image-add-layer" | "gimp-image-undo-group-start" | "gimp-image-undo-group-end" | "gimp-displays-flush" => done(),
            "gimp-display-new" => ok([Value::Int(1)]),
            // ---- layers and drawables
            "gimp-layer-new" => {
                // (image width height type name opacity mode): the layer is created in place.
                let image = int(name, a, 0)?;
                self.activate(name, image)?;
                let mode = a.get(6).and_then(Value::as_int).and_then(|m| usize::try_from(m).ok()).and_then(|m| MODES.get(m)).map_or("Normal", |(_, n)| n);
                let layer = self.run(
                    name,
                    "layer.new.layer",
                    json!({"name": string(name, a, 4)?, "opacity": (float(name, a, 5)? / 100.0).clamp(0.0, 1.0), "mode": mode}),
                )?;
                let id = layer.get("layer").and_then(Json::as_u64).ok_or_else(|| bad(name, "the layer was not created"))?;
                let doc = self.active_doc_id(name)?;
                ok([Value::Int(self.drawable_number(doc, id))])
            }
            "gimp-layer-set-opacity" => {
                self.select_drawable(name, int(name, a, 0)?)?;
                self.run(name, "layer.setProps", json!({"opacity": (float(name, a, 1)? / 100.0).clamp(0.0, 1.0)}))?;
                done()
            }
            "gimp-item-set-name" | "gimp-drawable-set-name" => {
                self.select_drawable(name, int(name, a, 0)?)?;
                self.run(name, "layer.renameLayer", json!({"name": string(name, a, 1)?}))?;
                done()
            }
            "gimp-drawable-fill" | "gimp-edit-fill" => {
                self.select_drawable(name, int(name, a, 0)?)?;
                let contents = match int(name, a, 1)? {
                    0 => "foreground",
                    1 => "background",
                    2 => "white",
                    3 => return Ok(Some(self.run(name, "edit.clear", json!({})).map(|_| Value::list([Value::Bool(true)])))),
                    other => return Err(bad(name, format!("unsupported fill type {other}"))),
                };
                self.run(name, "edit.fill", json!({"contents": contents}))?;
                done()
            }
            // ---- selection
            "gimp-selection-all" => {
                self.activate(name, int(name, a, 0)?)?;
                self.run(name, "select.all", json!({}))?;
                done()
            }
            "gimp-selection-none" => {
                self.activate(name, int(name, a, 0)?)?;
                self.run(name, "select.deselect", json!({}))?;
                done()
            }
            "gimp-image-select-rectangle" | "gimp-image-select-ellipse" => {
                self.activate(name, int(name, a, 0)?)?;
                let mode = ["add", "subtract", "replace", "intersect"].get(usize::try_from(int(name, a, 1)?).unwrap_or(2)).copied().unwrap_or("replace");
                self.run(
                    name,
                    "select.rect",
                    json!({"x": float(name, a, 2)?.round(), "y": float(name, a, 3)?.round(), "width": float(name, a, 4)?.round(), "height": float(name, a, 5)?.round(),
                           "mode": mode, "ellipse": name.ends_with("ellipse")}),
                )?;
                done()
            }
            // ---- context
            "gimp-context-set-foreground" => {
                self.foreground = Some(color(name, a.first())?);
                self.set_colors(name)?;
                done()
            }
            "gimp-context-set-background" => {
                self.background = Some(color(name, a.first())?);
                self.set_colors(name)?;
                done()
            }
            "gimp-context-get-foreground" => ok([hex_to_list(self.foreground.as_deref().unwrap_or("#000000"))]),
            "gimp-context-get-background" => ok([hex_to_list(self.background.as_deref().unwrap_or("#ffffff"))]),
            "gimp-message" => {
                self.output.push_str(&string(name, a, 0)?);
                self.output.push('\n');
                done()
            }
            // ---- filters (plug-in-*: run-mode image drawable …)
            "plug-in-gauss" | "plug-in-gauss-iir" | "plug-in-gauss-rle" | "plug-in-gauss-iir2" => {
                self.select_drawable(name, int(name, a, 2)?)?;
                let radius = (float(name, a, 3)?.max(float(name, a, 4)?)).clamp(0.1, 1000.0);
                self.run(name, "filter.blur.gaussianBlur", json!({"radius": radius}))?;
                done()
            }
            "plug-in-edge" => {
                self.select_drawable(name, int(name, a, 2)?)?;
                let algorithm = ["sobel", "prewitt", "gradient", "roberts", "differential", "laplace"]
                    .get(usize::try_from(int(name, a, 5)?).unwrap_or(0))
                    .copied()
                    .unwrap_or("sobel");
                self.run(name, "filter.stylize.edgeDetection", json!({"algorithm": algorithm, "amount": float(name, a, 3)?.clamp(0.1, 10.0)}))?;
                done()
            }
            "plug-in-pixelize" => {
                self.select_drawable(name, int(name, a, 2)?)?;
                self.run(name, "filter.pixelate.mosaic", json!({"cellSize": float(name, a, 3)?.clamp(2.0, 200.0)}))?;
                done()
            }
            "plug-in-vinvert" => {
                self.select_drawable(name, int(name, a, 2)?)?;
                self.run(name, "filter.other.invertBrightness", json!({}))?;
                done()
            }
            // ---- registration
            "script-fu-register" => {
                let r = parse_register(a).map_err(Error::msg)?;
                self.registered.retain(|x| x.name != r.name);
                self.registered.push(r);
                done()
            }
            "script-fu-menu-register" => {
                let (script, menu) = (string(name, a, 0)?, string(name, a, 1)?);
                if let Some(r) = self.registered.iter_mut().find(|r| r.name == script) {
                    r.menu = Some(menu);
                }
                done()
            }
            _ => None,
        })
    }
}

/// The value of the Script-Fu / Python-Fu constant `name` (`RGB`, `NORMAL-MODE`, `SF-STRING`).
pub fn constant(name: &str) -> Option<i64> {
    if let Some((_, v)) = ENUMS.iter().find(|(n, _)| *n == name) {
        return Some(*v);
    }
    if let Some(i) = MODES.iter().position(|(n, _)| *n == name) {
        return Some(i as i64);
    }
    ParamKind::NAMES.iter().position(|(n, _)| *n == name).map(|i| i as i64)
}

/// Every constant name, for languages that bind them up front.
pub fn constant_names() -> Vec<&'static str> {
    ENUMS.iter().map(|(n, _)| *n).chain(MODES.iter().map(|(n, _)| *n)).chain(ParamKind::NAMES.iter().map(|(n, _)| *n)).collect()
}

impl Host for Pdb {
    fn constant(&mut self, name: &str) -> Option<Value> {
        constant(name).map(Value::Int)
    }

    fn call(&mut self, _interp: &mut Interp, name: &str, args: &[Value]) -> Option<Result<Value>> {
        // Anything that isn't a known `gimp-*` / `plug-in-*` procedure is reported as unbound by
        // the interpreter.
        self.0.borrow_mut().call(name, args)
    }
}
