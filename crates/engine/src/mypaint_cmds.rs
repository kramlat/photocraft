//! MyPaint brushes: the brush library (`mypaint.brushes.*`) and the stroke command
//! (`paint.mypaint`). The brushes are `.myb` files (version 3, what MyPaint 2.x writes); the
//! engine is [`photocraft_paint::mypaint`], a port of libmypaint. The default set of MyPaint
//! (CC0) is embedded; further brushes come from the folders MyPaint itself uses, from
//! `~/.local/share/photocraft/mypaint-brushes`, and from `mypaint.brushes.import`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use photocraft_doc::{Document, LayerId};
use photocraft_geom::Rect;
use photocraft_paint::mypaint::tables::set;
use photocraft_paint::mypaint::{Brush, Event, TileSurface};
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

mod embedded;
#[cfg(not(target_arch = "wasm32"))]
mod embedded_previews;
pub mod preview;

pub use preview::Preview;

#[derive(Clone, Debug)]
enum Source {
    Embedded(&'static str),
    File(PathBuf),
    Inline(String),
}

/// Where a brush's thumbnail comes from; none means the engine draws one.
#[derive(Clone, Debug)]
enum PreviewSrc {
    None,
    #[cfg(not(target_arch = "wasm32"))]
    Embedded(&'static [u8]),
    File(PathBuf),
}

#[derive(Clone, Debug)]
struct Entry {
    id: String,
    name: String,
    group: String,
    /// The brush's own size (diameter, px).
    size: f32,
    source: Source,
    preview: PreviewSrc,
}

/// The brushes available and the one picked. Filled on first use.
#[derive(Default)]
pub struct Library {
    entries: Vec<Entry>,
    scanned: bool,
    current: Option<String>,
    /// Where imported brushes are kept; `None` is the default (see [`default_user_dir`]).
    pub user_dir: Option<PathBuf>,
    previews: std::collections::HashMap<String, Option<Arc<Preview>>>,
}

fn pretty(stem: &str) -> String {
    stem.replace(['_', '+'], " ").trim().to_string()
}

fn capitalised(folder: &str) -> String {
    let mut c = folder.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
}

/// The brush's size from its text (`radius_logarithmic` is the log of the radius).
fn default_size(text: &str) -> f32 {
    Brush::from_json(text).map_or(10.0, |b| 2.0 * b.base_value(set::RADIUS_LOGARITHMIC).exp())
}

#[cfg(not(target_arch = "wasm32"))]
fn default_user_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("photocraft/mypaint-brushes"))
}

#[cfg(not(target_arch = "wasm32"))]
fn search_roots(user: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = std::env::var_os("PHOTOCRAFT_MYPAINT_BRUSHES") {
        roots.push(PathBuf::from(p));
    }
    // The folders MyPaint reads: its user brushes, then the packs of the system data directories.
    let home_data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    if let Some(d) = &home_data {
        roots.push(d.join("mypaint/brushes"));
    }
    if let Some(d) = user {
        roots.push(d);
    }
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in dirs.split(':').filter(|d| !d.is_empty()) {
        roots.push(Path::new(d).join("mypaint-data/2.0/brushes"));
        roots.push(Path::new(d).join("mypaint-data/1.0/brushes"));
    }
    roots
}

/// The `.myb` files one folder down from `root` (`root/group/brush.myb`) and in `root` itself.
#[cfg(not(target_arch = "wasm32"))]
fn scan(root: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    dirs.sort();
    for p in dirs {
        if p.is_dir() {
            let folder = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let Ok(files) = std::fs::read_dir(&p) else { continue };
            let mut files: Vec<PathBuf> = files.filter_map(|e| e.ok()).map(|e| e.path()).filter(|f| f.extension().is_some_and(|x| x == "myb")).collect();
            files.sort();
            for f in files.into_iter().take(2000) {
                let stem = f.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
                out.push((format!("{folder}/{stem}"), capitalised(&folder), f));
            }
        } else if p.extension().is_some_and(|x| x == "myb") {
            let stem = p.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
            out.push((format!("user/{stem}"), "User".into(), p));
        }
    }
    out
}

/// A brush group: its name and its `(id, name, own size px)` brushes.
pub type BrushGroup = (String, Vec<(String, String, f32)>);

impl Library {
    /// Loads the brushes on first use; a no-op after that.
    pub fn ensure(&mut self) {
        self.ensure_loaded();
    }

    /// The brushes by group: `(group, [(id, name, own size px)])`, in panel order.
    pub fn groups(&mut self) -> Vec<BrushGroup> {
        self.ensure_loaded();
        let mut out: Vec<BrushGroup> = Vec::new();
        for e in &self.entries {
            match out.iter_mut().find(|(g, _)| *g == e.group) {
                Some((_, v)) => v.push((e.id.clone(), e.name.clone(), e.size)),
                None => out.push((e.group.clone(), vec![(e.id.clone(), e.name.clone(), e.size)])),
            }
        }
        out
    }

    /// The selected brush's own size, if the library is loaded.
    pub fn current_size(&self) -> Option<f32> {
        let id = self.current.as_ref()?;
        self.entries.iter().find(|e| &e.id == id).map(|e| e.size)
    }

    /// The selected brush: `(id, name, own size px)`.
    pub fn current(&mut self) -> Option<(String, String, f32)> {
        self.ensure_loaded();
        let id = self.current.clone()?;
        self.entries.iter().find(|e| e.id == id).map(|e| (e.id.clone(), e.name.clone(), e.size))
    }

    fn ensure_loaded(&mut self) {
        if self.scanned {
            return;
        }
        self.scanned = true;
        for (id, group, text) in embedded::BRUSHES {
            let stem = id.rsplit('/').next().unwrap_or(id);
            #[cfg(not(target_arch = "wasm32"))]
            let preview = embedded_previews::PREVIEWS.iter().find(|(k, _)| k == id).map_or(PreviewSrc::None, |(_, b)| PreviewSrc::Embedded(b));
            #[cfg(target_arch = "wasm32")]
            let preview = PreviewSrc::None;
            self.entries.push(Entry { id: (*id).to_string(), name: pretty(stem), group: (*group).to_string(), size: default_size(text), source: Source::Embedded(text), preview });
        }
        #[cfg(not(target_arch = "wasm32"))]
        for root in search_roots(self.user_dir.clone().or_else(default_user_dir)) {
            for (id, group, path) in scan(&root) {
                // A file on disk replaces the embedded brush of the same name (MyPaint's own
                // pack is the same set), and an earlier folder wins over a later one.
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if text.len() > 1 << 20 || Brush::from_json(&text).is_err() {
                    continue;
                }
                let stem = id.rsplit('/').next().unwrap_or(&id).to_string();
                let png = path.with_file_name(format!("{stem}_prev.png"));
                let preview = if png.is_file() { PreviewSrc::File(png) } else { PreviewSrc::None };
                let entry = Entry { id: id.clone(), name: pretty(&stem), group, size: default_size(&text), source: Source::File(path), preview };
                match self.entries.iter_mut().find(|e| e.id == id) {
                    Some(e) if matches!(e.source, Source::Embedded(_)) => *e = entry,
                    Some(_) => {}
                    None => self.entries.push(entry),
                }
            }
        }
        if self.current.is_none() {
            self.current = self.entries.iter().find(|e| e.id == "classic/brush").or(self.entries.first()).map(|e| e.id.clone());
        }
    }

    /// The brush's thumbnail: the pack's own `_prev.png` when it has one, else a stroke drawn by the
    /// engine. Made once per brush; `None` for an unknown id or a brush that will not load.
    pub fn preview(&mut self, id: &str) -> Option<Arc<Preview>> {
        self.ensure_loaded();
        if let Some(p) = self.previews.get(id) {
            return p.clone();
        }
        let e = self.entries.iter().find(|e| e.id == id)?.clone();
        let made = match &e.preview {
            #[cfg(not(target_arch = "wasm32"))]
            PreviewSrc::Embedded(b) => preview::decode(b),
            PreviewSrc::File(p) => std::fs::read(p).ok().and_then(|b| preview::decode(&b)),
            PreviewSrc::None => None,
        }
        .or_else(|| self.text(&e).ok().and_then(|t| preview::render(&t)))
        .map(Arc::new);
        self.previews.insert(id.to_string(), made.clone());
        made
    }

    fn find(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == key).or_else(|| self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(key))).or_else(|| self.entries.iter().find(|e| e.id.eq_ignore_ascii_case(key)))
    }

    fn text(&self, e: &Entry) -> Result<String> {
        match &e.source {
            Source::Embedded(t) => Ok((*t).to_string()),
            Source::Inline(t) => Ok(t.clone()),
            Source::File(p) => std::fs::read_to_string(p).map_err(|err| EngineError::Other(format!("{}: {err}", p.display()))),
        }
    }
}

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    let lib = &mut s.mypaint;
    lib.ensure();
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for e in &lib.entries {
        let item = json!({"id": e.id, "name": e.name, "size": e.size});
        match groups.iter_mut().find(|(g, _)| *g == e.group) {
            Some((_, v)) => v.push(item),
            None => groups.push((e.group.clone(), vec![item])),
        }
    }
    Ok(json!({"groups": groups.into_iter().map(|(name, brushes)| json!({"name": name, "brushes": brushes})).collect::<Vec<_>>(), "current": lib.current}))
}

fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let key = p.get("brush").and_then(Value::as_str).ok_or_else(|| bad("mypaint.brushes.select", "missing `brush`"))?;
    let lib = &mut s.mypaint;
    lib.ensure();
    let e = lib.find(key).ok_or_else(|| bad("mypaint.brushes.select", format!("no MyPaint brush \"{key}\"")))?.clone();
    lib.current = Some(e.id.clone());
    Ok(json!({"brush": e.id, "name": e.name, "size": e.size}))
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mypaint.brushes.import";
    let (text, name) = match (p.get("path").and_then(Value::as_str), p.get("data").and_then(Value::as_str)) {
        (Some(path), _) => {
            let t = crate::file_cmds::read_file(path).map(|b| String::from_utf8_lossy(&b).into_owned())?;
            (t, Path::new(path).file_stem().and_then(|n| n.to_str()).unwrap_or("brush").to_string())
        }
        (None, Some(t)) => (t.to_string(), p.get("name").and_then(Value::as_str).unwrap_or("brush").to_string()),
        (None, None) => return Err(bad(CMD, "missing `path` (a .myb file) or `data`")),
    };
    if text.len() > 1 << 20 {
        return Err(bad(CMD, "the brush file is over 1 MB"));
    }
    Brush::from_json(&text).map_err(|e| bad(CMD, e))?;
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '+') { c } else { '_' }).collect();
    let id = format!("user/{clean}");
    #[allow(unused_mut)]
    let mut source = Source::Inline(text.clone());
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(dir) = s.mypaint.user_dir.clone().or_else(default_user_dir).map(|d| d.join("user")) {
        // Kept for next time; a folder that can't be written still gives a brush for this session.
        let file = dir.join(format!("{clean}.myb"));
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, &text).is_ok() {
            source = Source::File(file);
        }
    }
    let lib = &mut s.mypaint;
    lib.ensure();
    let entry = Entry { id: id.clone(), name: pretty(&clean), group: "User".into(), size: default_size(&text), source, preview: PreviewSrc::None };
    match lib.entries.iter_mut().find(|e| e.id == id) {
        Some(e) => *e = entry.clone(),
        None => lib.entries.push(entry.clone()),
    }
    lib.current = Some(id.clone());
    Ok(json!({"brush": id, "name": entry.name, "size": entry.size}))
}

/// One pointer sample of a MyPaint stroke, as every stroke command takes them: `[x, y, pressure,
/// tilt x°, tilt y°, rotation°, time ms]` (all but the first two optional).
fn parse_points(p: &Value, cmd: &str) -> Result<Vec<photocraft_paint::StrokePoint>> {
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `points`"))?;
    let mut out = Vec::with_capacity(arr.len());
    let mut t = 0.0f64;
    for v in arr {
        let a = v.as_array().ok_or_else(|| bad(cmd, "`points` are [x, y, pressure?, tiltX?, tiltY?, rotation?, timeMs?]"))?;
        let n = |i: usize| a.get(i).and_then(Value::as_f64);
        let (Some(x), Some(y)) = (n(0), n(1)) else { return Err(bad(cmd, "a point needs numeric x and y")) };
        // Without times the pointer is taken to report every 8 ms.
        t = n(6).unwrap_or(t + 8.0);
        let mut sp = photocraft_paint::StrokePoint::new(x, y, n(2).unwrap_or(1.0) as f32);
        sp.time = t;
        sp.tilt_x = n(3).unwrap_or(0.0) as f32;
        sp.tilt_y = n(4).unwrap_or(0.0) as f32;
        sp.rotation = n(5).unwrap_or(0.0) as f32;
        out.push(sp);
    }
    if out.is_empty() {
        return Err(bad(cmd, "`points` is empty"));
    }
    crate::brush_cmds::check_coords(&out, cmd)?;
    Ok(out)
}

fn colour(p: &Value, s: &Session) -> [f32; 3] {
    let c = crate::brush_cmds::color(p.get("color"), s.tools.foreground);
    [c[0], c[1], c[2]]
}

/// The brush for a stroke: the named (or current, or inline `myb`) brush with the size, opacity,
/// colour and eraser the params ask for.
fn make_brush(s: &Session, p: &Value, points: &[photocraft_paint::StrokePoint], cmd: &str) -> Result<Brush> {
    let text = match p.get("myb").and_then(Value::as_str) {
        Some(t) => t.to_string(),
        None => {
            let lib = &s.mypaint;
            if !lib.scanned {
                return Err(bad(cmd, "the MyPaint brush library is not loaded yet (run mypaint.brushes.list)"));
            }
            let key = p.get("brush").and_then(Value::as_str).map(str::to_string).or_else(|| lib.current.clone()).ok_or_else(|| bad(cmd, "no MyPaint brush"))?;
            let e = lib.find(&key).ok_or_else(|| bad(cmd, format!("no MyPaint brush \"{key}\"")))?.clone();
            lib.text(&e)?
        }
    };
    let mut b = Brush::from_json(&text).map_err(|e| bad(cmd, e))?;
    if let Some(size) = p.get("size").and_then(Value::as_f64) {
        if !(0.5..=2000.0).contains(&size) {
            return Err(bad(cmd, "size is between 0.5 and 2000 px"));
        }
        b.set_base_value(set::RADIUS_LOGARITHMIC, (size as f32 / 2.0).ln());
    }
    if let Some(o) = p.get("opacity").and_then(Value::as_f64) {
        b.set_base_value(set::OPAQUE, b.base_value(set::OPAQUE) * (o as f32).clamp(0.0, 1.0));
    }
    if let Some(h) = p.get("hardness").and_then(Value::as_f64) {
        b.set_base_value(set::HARDNESS, (h as f32).clamp(0.0, 1.0));
    }
    if p.get("erase").and_then(Value::as_bool) == Some(true) {
        b.set_base_value(set::ERASER, 1.0);
    }
    b.set_color(colour(p, s));
    let seed = match p.get("seed").and_then(Value::as_u64) {
        Some(v) => v,
        None => photocraft_paint::rng::seed_from_bytes(format!("{:?}", points.iter().map(|q| (q.x as i64, q.y as i64)).collect::<Vec<_>>()).as_bytes()),
    };
    b.seed((seed & 0x3fff_ffff) as i64);
    b.reset();
    Ok(b)
}

fn event_for(prev_time: &mut Option<f64>, sp: &photocraft_paint::StrokePoint, zoom: f32) -> Event {
    let dtime = prev_time.map_or(0.01, |t| ((sp.time - t) / 1000.0).max(0.0001));
    *prev_time = Some(sp.time);
    Event {
        x: sp.x as f32,
        y: sp.y as f32,
        pressure: sp.pressure,
        xtilt: sp.tilt_x / 90.0,
        ytilt: sp.tilt_y / 90.0,
        dtime,
        viewzoom: zoom,
        viewrotation: 0.0,
        barrel_rotation: sp.rotation / 360.0,
    }
}

fn ensure_paintable(surf: &Surface, cmd: &str) -> Result<()> {
    use photocraft_color::ColorMode;
    match surf.format().mode {
        ColorMode::Rgb | ColorMode::Grayscale => Ok(()),
        m => Err(bad(cmd, format!("MyPaint brushes paint in RGB and Grayscale documents, not {m:?}"))),
    }
}

fn paint_mypaint(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "paint.mypaint";
    let points = parse_points(p, CMD)?;
    s.mypaint.ensure();
    let mut brush = make_brush(s, p, &points, CMD)?;
    let zoom = p.get("zoom").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.01, 100.0) as f32;
    let id: Option<LayerId> = if crate::channel_cmds::is_channel_target(p) {
        None
    } else {
        Some(match p.get("layer").and_then(Value::as_u64) {
            Some(i) => LayerId(i),
            None => s.active().and_then(|d| d.active_layer).ok_or(EngineError::Other("no active layer".into()))?,
        })
    };
    let dmg = s.edit("MyPaint Brush", |doc, _| {
        let pre = doc.clone();
        let bounds = doc.bounds();
        let (surf, lock) = crate::channel_cmds::target_surface(doc, id, p)?;
        ensure_paintable(surf, CMD)?;
        let mut tiles = TileSurface::new(surf.clone(), bounds);
        let mut prev = None;
        for sp in &points {
            brush.stroke_to(&mut tiles, event_for(&mut prev, sp, zoom), false);
        }
        let dirty = tiles.take_dirty();
        tiles.write_into(surf, dirty, pre.selection.as_ref(), lock);
        Ok(dirty)
    })?;
    if let Some(st) = s.active_mut() {
        st.last_damage = Some(dmg);
    }
    Ok(json!({"damage": [dmg.x0, dmg.y0, dmg.width(), dmg.height()]}))
}

fn has_pixel_layer(s: &Session) -> std::result::Result<(), String> {
    use photocraft_doc::LayerContent;
    if crate::channel_cmds::edits_channel(s) {
        return Ok(());
    }
    let d = s.active().ok_or("no document open")?;
    let l = d.active_layer.and_then(|id| d.doc.layer(id)).ok_or("no active layer")?;
    if matches!(l.content, LayerContent::Raster(_)) || l.mask.is_some() {
        Ok(())
    } else {
        Err(format!("active layer is {} {} layer, not a pixel layer", l.content.article(), l.content.kind_name()))
    }
}

/// A MyPaint stroke shown while it is drawn: the dabs so far on a copy of the active document,
/// painted exactly as `paint.mypaint` with the same params and points commits them.
pub struct LiveMyPaint {
    /// The active document with the stroke so far.
    pub doc: Arc<Document>,
    brush: Brush,
    tiles: TileSurface,
    id: Option<LayerId>,
    params: Value,
    pre_selection: Option<Surface>,
    lock: bool,
    prev_time: Option<f64>,
    zoom: f32,
    dirty: Rect,
    /// The seed the stroke's random numbers come from (the commit must reuse it).
    pub seed: u64,
}

impl LiveMyPaint {
    /// Starts a stroke from `p` (the params of `paint.mypaint`; its `points` are ignored, feed them
    /// with [`Self::push`]).
    pub fn begin(s: &Session, p: &Value) -> Result<Self> {
        const CMD: &str = "paint.mypaint";
        has_pixel_layer(s).map_err(EngineError::Other)?;
        let brush = make_brush(s, p, &[photocraft_paint::StrokePoint::new(0.0, 0.0, 1.0)], CMD)?;
        let st = s.active().ok_or(EngineError::NoDocument)?;
        let id = if crate::channel_cmds::is_channel_target(p) { None } else { st.active_layer };
        let mut doc = (*st.doc).clone();
        let bounds = doc.bounds();
        let pre_selection = doc.selection.clone();
        let (surf, lock) = crate::channel_cmds::target_surface(&mut doc, id, p)?;
        ensure_paintable(surf, CMD)?;
        let tiles = TileSurface::new(surf.clone(), bounds);
        let zoom = p.get("zoom").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.01, 100.0) as f32;
        Ok(LiveMyPaint { doc: Arc::new(doc), brush, tiles, id, params: p.clone(), pre_selection, lock, prev_time: None, zoom, dirty: Rect::EMPTY, seed: p.get("seed").and_then(Value::as_u64).unwrap_or(0) })
    }

    /// Paints more points; returns the rectangle that changed.
    pub fn push(&mut self, pts: &[photocraft_paint::StrokePoint]) -> Result<Rect> {
        crate::brush_cmds::check_coords(pts, "paint.mypaint")?;
        for sp in pts {
            let ev = event_for(&mut self.prev_time, sp, self.zoom);
            self.brush.stroke_to(&mut self.tiles, ev, false);
        }
        let d = self.tiles.take_dirty();
        if d.is_empty() {
            return Ok(d);
        }
        let doc = Arc::make_mut(&mut self.doc);
        let (surf, _) = crate::channel_cmds::target_surface(doc, self.id, &self.params)?;
        self.tiles.write_into(surf, d, self.pre_selection.as_ref(), self.lock);
        self.dirty = self.dirty.union(&d);
        Ok(d)
    }

    pub fn bounds(&self) -> Rect {
        self.dirty
    }
}

pub fn specs() -> Vec<CommandSpec> {
    macro_rules! spec {
        ($id:expr, $label:expr, $menu:expr, $params:expr, $enabled:expr, $run:expr) => {
            CommandSpec { id: $id, label: $label, menu: $menu, shortcut: None, params: $params, enabled: $enabled, journal: true, run: $run }
        };
    }
    vec![
        spec!("mypaint.brushes.list", "MyPaint Brushes", &[], r##"{} → {groups:[{name,brushes:[{id,name,size}]}],current}"##, crate::file_cmds::native, list),
        spec!("mypaint.brushes.select", "Select MyPaint Brush", &[], r##"{"brush":id|name}"##, crate::file_cmds::native, select),
        spec!(
            "mypaint.brushes.import",
            "Import MyPaint Brush…",
            &[],
            r##"{"path":".myb file"?,"data":".myb text"? (instead of a path),"name":str? (with data)}"##,
            crate::file_cmds::native,
            import
        ),
        spec!(
            "paint.mypaint",
            "MyPaint Brush",
            &[],
            r##"{"points":[[x,y,pressure?,tiltXdeg?,tiltYdeg?,rotationDeg?,timeMs?]…],"brush":id|name? (default: the selected one),"myb":".myb text"? (instead of a brush),"size":0.5..2000? (px; default the brush's own),"opacity":0..1?,"hardness":0..1?,"color":color? (default foreground),"erase":bool=false,"seed":0..1000000000?,"zoom":0.01..100=1,"layer":id?,"target":"pixels|mask|alpha:N|quickMask"?}"##,
            has_pixel_layer,
            paint_mypaint
        ),
    ]
}

#[cfg(test)]
mod tests;
