//! GIMP (`.xcf`) read and write, from the public XCF file-format description.
//!
//! Read (versions 0–27 as GIMP 2.x and 3.x write them): RGB, grayscale and indexed images at 8-bit,
//! 16-bit integer, half and 32-bit float precision (linear-light files are converted to the
//! encoded values the document uses), layers with position, opacity, visibility, locks and blend
//! mode, layer groups, layer masks, guides, resolution and the embedded colour profile. Saved
//! channels, paths, text and vector data, and non-destructive filters are skipped with a warning
//! (a text layer opens as its pixels).
//!
//! Write: version 11, 8-bit, RLE-compressed tiles; RGB and grayscale documents with their layers,
//! groups, masks, blend modes, opacity, visibility, locks, guides, resolution and colour profile.

use std::sync::Arc;

use photocraft_color::{BlendMode, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerContent, LayerMask};
use photocraft_geom::{Rect, Size};
use photocraft_raster::{Interrupt, Surface};

use crate::{ExportResult, ImportResult, IoError};

type Result<T> = std::result::Result<T, IoError>;

/// The text every XCF starts with.
const MAGIC: &[u8] = b"gimp xcf ";
/// The file version written.
const WRITE_VERSION: u32 = 11;
const TILE: u32 = 64;
/// Canvas side limit (GIMP's own is 524288; PhotoCraft's PSB limit is smaller).
const MAX_SIDE: u32 = 300_000;
/// Decoded layer data across the file is capped, as for the other layered formats.
const MAX_PIXEL_BYTES: usize = 1 << 30;
/// Layers and groups in one document.
const MAX_NODES: usize = 8192;
const MAX_PROPS: usize = 4096;

fn invalid(message: impl Into<String>) -> IoError {
    IoError::Xcf(message.into())
}

/// `true` for an XCF file (by its magic).
pub(crate) fn is_xcf(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

// ---------------------------------------------------------------------------------------------
// Property and mode numbers

const PROP_END: u32 = 0;
const PROP_COLORMAP: u32 = 1;
const PROP_SELECTION: u32 = 4;
const PROP_OPACITY: u32 = 6;
const PROP_MODE: u32 = 7;
const PROP_VISIBLE: u32 = 8;
const PROP_LINKED: u32 = 9;
const PROP_LOCK_ALPHA: u32 = 10;
const PROP_APPLY_MASK: u32 = 11;
const PROP_EDIT_MASK: u32 = 12;
const PROP_SHOW_MASK: u32 = 13;
const PROP_OFFSETS: u32 = 15;
const PROP_COMPRESSION: u32 = 17;
const PROP_GUIDES: u32 = 18;
const PROP_RESOLUTION: u32 = 19;
const PROP_PARASITES: u32 = 21;
const PROP_PATHS: u32 = 23;
const PROP_VECTORS: u32 = 25;
const PROP_TEXT_LAYER_FLAGS: u32 = 26;
const PROP_LOCK_CONTENT: u32 = 28;
const PROP_GROUP_ITEM: u32 = 29;
const PROP_ITEM_PATH: u32 = 30;
const PROP_GROUP_ITEM_FLAGS: u32 = 31;
const PROP_LOCK_POSITION: u32 = 32;
const PROP_FLOAT_OPACITY: u32 = 33;
const PROP_COMPOSITE_SPACE: u32 = 36;
const PROP_BLEND_SPACE: u32 = 37;
const PROP_VECTOR_LAYER: u32 = 47;
const PROP_LINK_LAYER: u32 = 48;

/// GIMP layer modes by file number, as PhotoCraft blend modes; `true` = only an approximation.
fn mode_from_xcf(n: u32) -> Option<(BlendMode, bool)> {
    use BlendMode as B;
    Some(match n {
        0 | 28 => (B::Normal, false),
        1 => (B::Dissolve, false),
        3 | 30 => (B::Multiply, false),
        4 | 31 => (B::Screen, false),
        5 | 23 => (B::Overlay, false),
        6 | 32 => (B::Difference, false),
        7 | 33 => (B::LinearDodge, false),
        8 | 34 => (B::Subtract, false),
        9 | 35 => (B::Darken, false),
        10 | 36 => (B::Lighten, false),
        11 | 37 | 24 => (B::Hue, n == 24),
        12 | 38 | 25 => (B::Saturation, n == 25),
        13 | 39 | 26 => (B::Color, n == 26),
        14 | 40 => (B::Luminosity, true),
        27 | 56 => (B::Luminosity, true),
        15 | 41 => (B::Divide, false),
        16 | 42 => (B::ColorDodge, false),
        17 | 43 => (B::ColorBurn, false),
        18 | 44 => (B::HardLight, false),
        19 | 45 => (B::SoftLight, true),
        48 => (B::VividLight, false),
        49 => (B::PinLight, false),
        50 => (B::LinearLight, false),
        51 => (B::HardMix, false),
        52 => (B::Exclusion, false),
        53 => (B::LinearBurn, false),
        54 => (B::DarkerColor, true),
        55 => (B::LighterColor, true),
        61 => (B::PassThrough, false),
        _ => return None,
    })
}

/// The GIMP mode number written for `mode`, if it has a counterpart.
fn mode_to_xcf(mode: BlendMode) -> Option<u32> {
    use BlendMode as B;
    Some(match mode {
        B::Normal => 28,
        B::Dissolve => 1,
        B::Multiply => 30,
        B::Screen => 31,
        B::Overlay => 23,
        B::Difference => 32,
        B::LinearDodge => 33,
        B::Subtract => 34,
        B::Darken => 35,
        B::Lighten => 36,
        B::Hue => 37,
        B::Saturation => 38,
        B::Color => 39,
        B::Luminosity => 40,
        B::Divide => 41,
        B::ColorDodge => 42,
        B::ColorBurn => 43,
        B::HardLight => 44,
        B::SoftLight => 45,
        B::VividLight => 48,
        B::PinLight => 49,
        B::LinearLight => 50,
        B::HardMix => 51,
        B::Exclusion => 52,
        B::LinearBurn => 53,
        B::DarkerColor => 54,
        B::LighterColor => 55,
        B::PassThrough => 61,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Reading

/// Warnings gathered during import, each distinct message once; per-layer ones are grouped.
#[derive(Default)]
struct Warnings {
    general: Vec<String>,
    per_layer: Vec<(String, Vec<String>)>,
}

impl Warnings {
    fn note(&mut self, message: &str) {
        if !self.general.iter().any(|m| m == message) {
            self.general.push(message.to_string());
        }
    }
    fn layer(&mut self, layer: &str, message: &str) {
        match self.per_layer.iter_mut().find(|(m, _)| m == message) {
            Some((_, layers)) => {
                if !layers.iter().any(|l| l == layer) {
                    layers.push(layer.to_string());
                }
            }
            None => self.per_layer.push((message.to_string(), vec![layer.to_string()])),
        }
    }
    fn finish(self) -> Vec<String> {
        let mut out = self.general;
        for (message, layers) in self.per_layer {
            let shown: Vec<String> = layers.iter().take(5).map(|l| format!("\"{l}\"")).collect();
            let more = layers.len().saturating_sub(5);
            let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
            out.push(format!("{} ({}{tail})", message, shown.join(", ")));
        }
        out
    }
}

/// A bounds-checked cursor over the file.
#[derive(Clone)]
struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
    /// Bytes per file offset (4 before version 11, else 8).
    off: usize,
}

impl<'a> Cur<'a> {
    fn at(&self, pos: usize) -> Cur<'a> {
        Cur { b: self.b, pos, off: self.off }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| invalid("truncated file"))?;
        let s = self.b.get(self.pos..end).ok_or_else(|| invalid("truncated file"))?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    /// A file offset (position of other data).
    fn offset(&mut self) -> Result<usize> {
        let v = if self.off == 8 {
            let s = self.take(8)?;
            u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
        } else {
            u64::from(self.u32()?)
        };
        usize::try_from(v).map_err(|_| invalid("offset out of range"))
    }
    /// A string: length (counting its NUL), then the bytes.
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        if n == 0 {
            return Ok(String::new());
        }
        if n > 1 << 20 {
            return Err(invalid("string too long"));
        }
        let s = self.take(n)?;
        let s = s.strip_suffix(&[0]).unwrap_or(s);
        Ok(String::from_utf8_lossy(s).into_owned())
    }
}

/// Image precision as stored: bytes per component, float or integer, and whether the values are
/// linear light.
#[derive(Clone, Copy)]
struct Precision {
    bytes: usize,
    float: bool,
    linear: bool,
}

fn precision_from(version: u32, p: u32) -> Result<Precision> {
    let make = |bytes, float, linear| Precision { bytes, float, linear };
    if version == 4 {
        return match p {
            0 => Ok(make(1, false, false)),
            1 => Ok(make(2, false, false)),
            2 => Ok(make(4, false, true)),
            3 => Ok(make(2, true, true)),
            4 => Ok(make(4, true, true)),
            _ => Err(invalid(format!("invalid precision {p} for version 4"))),
        };
    }
    // Versions 5–6 knew only the linear / non-linear pairs; 7 added perceptual (treated as encoded).
    let (class, kind) = (p / 100, p % 100);
    let linear = kind == 0;
    match class {
        1 => Ok(make(1, false, linear)),
        2 => Ok(make(2, false, linear)),
        3 => Ok(make(4, false, linear)),
        5 => Ok(make(2, true, linear)),
        6 => Ok(make(4, true, linear)),
        7 => Ok(make(8, true, linear)),
        _ => Err(invalid(format!("unsupported image precision {p}"))),
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f32::from(h & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + frac / 1024.0) * 2f32.powi(e - 15),
    }
}

fn linear_to_encoded(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// One component at `bytes` (big-endian for version 12+, the writer's native order before).
fn component(prec: Precision, big_endian: bool, s: &[u8]) -> f32 {
    let mut a = [0u8; 8];
    let n = prec.bytes.min(8);
    a[..n].copy_from_slice(s.get(..n).unwrap_or(&[0u8; 8][..n]));
    if !big_endian {
        a[..n].reverse();
    }
    // `a[..n]` is now big-endian.
    match (prec.bytes, prec.float) {
        (1, _) => f32::from(a[0]) / 255.0,
        (2, false) => f32::from(u16::from_be_bytes([a[0], a[1]])) / 65535.0,
        (2, true) => half_to_f32(u16::from_be_bytes([a[0], a[1]])),
        (4, false) => (f64::from(u32::from_be_bytes([a[0], a[1], a[2], a[3]])) / f64::from(u32::MAX)) as f32,
        (4, true) => f32::from_bits(u32::from_be_bytes([a[0], a[1], a[2], a[3]])),
        (8, _) => f64::from_be_bytes(a) as f32,
        _ => 0.0,
    }
}

/// Run-length decodes one tile: the file stores each byte of the pixel as its own plane.
fn rle_tile(data: &[u8], pixels: usize, bpp: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; pixels.checked_mul(bpp)?];
    let mut p = 0usize;
    for plane in 0..bpp {
        let mut size = pixels;
        let mut at = plane;
        while size > 0 {
            let val = *data.get(p)?;
            p += 1;
            if val >= 128 {
                // A literal run: 256 − val bytes, or a 16-bit count when val == 128.
                let mut len = 256 - usize::from(val);
                if len == 128 {
                    len = (usize::from(*data.get(p)?) << 8) | usize::from(*data.get(p + 1)?);
                    p += 2;
                }
                if len > size {
                    return None;
                }
                let run = data.get(p..p.checked_add(len)?)?;
                p += len;
                for b in run {
                    *out.get_mut(at)? = *b;
                    at += bpp;
                }
                size -= len;
            } else {
                // A repeat: val + 1 copies of the next byte, or a 16-bit count when val == 127.
                let mut len = usize::from(val) + 1;
                if len == 128 {
                    len = (usize::from(*data.get(p)?) << 8) | usize::from(*data.get(p + 1)?);
                    p += 2;
                }
                if len > size {
                    return None;
                }
                let b = *data.get(p)?;
                p += 1;
                for _ in 0..len {
                    *out.get_mut(at)? = b;
                    at += bpp;
                }
                size -= len;
            }
        }
    }
    Some(out)
}

/// One decoded drawable: `w × h` pixels of `comps` f32 components.
struct Pixels {
    w: u32,
    h: u32,
    comps: usize,
    data: Vec<f32>,
}

struct Props {
    kind_group: bool,
    kind_text: bool,
    kind_vector: bool,
    opacity: f32,
    mode: u32,
    visible: bool,
    lock_alpha: bool,
    lock_content: bool,
    lock_position: bool,
    offset: (i32, i32),
    apply_mask: bool,
    expanded: bool,
    path: Option<Vec<usize>>,
    selection: bool,
    /// Blends or composites in linear light (GIMP's `RGB_LINEAR`).
    linear_light: bool,
    parasites: Vec<(String, Vec<u8>)>,
}

impl Default for Props {
    fn default() -> Self {
        Props {
            kind_group: false,
            kind_text: false,
            kind_vector: false,
            opacity: 1.0,
            mode: 28,
            visible: true,
            lock_alpha: false,
            lock_content: false,
            lock_position: false,
            offset: (0, 0),
            apply_mask: true,
            expanded: false,
            path: None,
            selection: false,
            linear_light: false,
            parasites: Vec::new(),
        }
    }
}

struct Ctx<'a, 'c> {
    c: Cur<'a>,
    version: u32,
    prec: Precision,
    colormap: Vec<[u8; 3]>,
    compression: u8,
    budget: usize,
    ctl: &'c Interrupt<'c>,
    warnings: Warnings,
}

fn parse_parasites(payload: &[u8], off: usize) -> Vec<(String, Vec<u8>)> {
    let mut c = Cur { b: payload, pos: 0, off };
    let mut out = Vec::new();
    while c.pos < payload.len() {
        let parsed = (|| -> Result<(String, Vec<u8>)> {
            let name = c.string()?;
            let _flags = c.u32()?;
            let size = c.u32()? as usize;
            Ok((name, c.take(size)?.to_vec()))
        })();
        match parsed {
            Ok(p) => out.push(p),
            Err(_) => break,
        }
    }
    out
}

impl Ctx<'_, '_> {
    /// Reads a property list (up to PROP_END) into `props`; image-level properties the caller
    /// wants are reported through `image`.
    fn props(&mut self, image: Option<&mut ImageProps>) -> Result<Props> {
        let mut p = Props::default();
        let mut image = image;
        for _ in 0..MAX_PROPS {
            let kind = self.c.u32()?;
            let size = self.c.u32()? as usize;
            if kind == PROP_END {
                return Ok(p);
            }
            let payload = self.c.take(size)?;
            let mut v = Cur { b: payload, pos: 0, off: self.c.off };
            match kind {
                PROP_OPACITY => p.opacity = (v.u32()? as f32 / 255.0).clamp(0.0, 1.0),
                PROP_FLOAT_OPACITY => p.opacity = v.f32()?.clamp(0.0, 1.0),
                PROP_MODE => p.mode = v.u32()?,
                PROP_VISIBLE => p.visible = v.u32()? != 0,
                PROP_LOCK_ALPHA => p.lock_alpha = v.u32()? != 0,
                PROP_LOCK_CONTENT => p.lock_content = v.u32()? != 0,
                PROP_LOCK_POSITION => p.lock_position = v.u32()? != 0,
                PROP_OFFSETS => p.offset = (v.i32()?, v.i32()?),
                PROP_APPLY_MASK => p.apply_mask = v.u32()? != 0,
                PROP_GROUP_ITEM => p.kind_group = true,
                PROP_GROUP_ITEM_FLAGS => p.expanded = v.u32()? & 1 != 0,
                PROP_ITEM_PATH => {
                    let mut path = Vec::new();
                    while v.pos + 4 <= payload.len() && path.len() < 64 {
                        path.push(v.u32()? as usize);
                    }
                    p.path = Some(path);
                }
                PROP_TEXT_LAYER_FLAGS => p.kind_text = true,
                PROP_VECTOR_LAYER | PROP_LINK_LAYER => p.kind_vector = true,
                PROP_SELECTION => p.selection = true,
                // A negative value is "auto" together with what auto meant when the file was saved.
                PROP_BLEND_SPACE | PROP_COMPOSITE_SPACE => p.linear_light |= v.i32()?.unsigned_abs() == 1,
                PROP_PARASITES => p.parasites = parse_parasites(payload, self.c.off),
                PROP_COMPRESSION => {
                    if let Some(i) = image.as_deref_mut() {
                        i.compression = v.u8()?;
                    }
                }
                PROP_RESOLUTION => {
                    if let Some(i) = image.as_deref_mut() {
                        i.resolution = Some((v.f32()?, v.f32()?));
                    }
                }
                PROP_GUIDES => {
                    if let Some(i) = image.as_deref_mut() {
                        while v.pos + 5 <= payload.len() && i.guides.len() < 4096 {
                            let pos = v.i32()?;
                            let orientation = v.u8()?;
                            i.guides.push((pos, orientation));
                        }
                    }
                }
                PROP_COLORMAP => {
                    if let Some(i) = image.as_deref_mut() {
                        let n = (v.u32()? as usize).min(256);
                        for _ in 0..n {
                            let s = v.take(3)?;
                            i.colormap.push([s[0], s[1], s[2]]);
                        }
                    }
                }
                PROP_PATHS | PROP_VECTORS => {
                    if let Some(i) = image.as_deref_mut() {
                        i.has_paths = true;
                    }
                }
                _ => {}
            }
        }
        Err(invalid("a property list never ends"))
    }

    /// The pixels of the hierarchy at `offset` (`comps` components per pixel, `w × h`), as f32
    /// with colour components converted to encoded values.
    fn hierarchy(&mut self, offset: usize, w: u32, h: u32, comps: usize, color_comps: usize) -> Result<Pixels> {
        let mut c = self.c.at(offset);
        let (hw, hh, bpp) = (c.u32()?, c.u32()?, c.u32()? as usize);
        let comp_size = self.prec.bytes;
        if hw != w || hh != h || bpp != comps * comp_size {
            return Err(invalid("a layer's pixel data does not match its size and type"));
        }
        let cost = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(comps)).and_then(|n| n.checked_mul(4)).ok_or_else(|| invalid("layer size overflow"))?;
        self.budget = self.budget.checked_sub(cost).ok_or_else(|| invalid("layers exceed the 1 GiB pixel limit"))?;
        let mut data = vec![0.0f32; cost / 4];
        let level = c.offset()?;
        let mut l = self.c.at(level);
        let (lw, lh) = (l.u32()?, l.u32()?);
        if lw != w || lh != h {
            return Err(invalid("a layer's level size does not match"));
        }
        let cols = w.div_ceil(TILE) as usize;
        let rows = h.div_ceil(TILE) as usize;
        let max_len = (TILE as usize) * (TILE as usize) * bpp * 3 / 2;
        let big_endian = self.version >= 12;
        let mut offsets = Vec::with_capacity(cols * rows);
        for _ in 0..cols * rows {
            let o = l.offset()?;
            if o == 0 {
                return Err(invalid("not enough tiles in a level"));
            }
            offsets.push(o);
        }
        // The list ends with a zero offset; the tile lengths come from the offsets themselves.
        for (i, &start) in offsets.iter().enumerate() {
            if i % 16 == 0 {
                self.ctl.check().map_err(|_| IoError::Cancelled)?;
            }
            let (tx, ty) = ((i % cols) as u32 * TILE, (i / cols) as u32 * TILE);
            let (tw, th) = ((w - tx).min(TILE) as usize, (h - ty).min(TILE) as usize);
            let end = offsets.get(i + 1).copied().filter(|e| *e > start).unwrap_or_else(|| start.saturating_add(max_len)).min(start.saturating_add(max_len)).min(self.c.b.len());
            let raw = self.c.b.get(start..end).ok_or_else(|| invalid("a tile lies outside the file"))?;
            let pixels = tw * th;
            let tile: Vec<u8> = match self.compression {
                0 => raw.get(..pixels * bpp).ok_or_else(|| invalid("a tile is truncated"))?.to_vec(),
                1 => rle_tile(raw, pixels, bpp).ok_or_else(|| invalid("a tile's run-length data is damaged"))?,
                2 => {
                    use std::io::Read;
                    let mut out = Vec::with_capacity(pixels * bpp);
                    flate2::read::ZlibDecoder::new(raw).take((pixels * bpp) as u64).read_to_end(&mut out).map_err(|e| invalid(format!("a tile will not decompress: {e}")))?;
                    if out.len() != pixels * bpp {
                        return Err(invalid("a tile decompressed to the wrong size"));
                    }
                    out
                }
                other => return Err(invalid(format!("unsupported tile compression {other}"))),
            };
            for py in 0..th {
                for px in 0..tw {
                    let src = (py * tw + px) * bpp;
                    let dst = ((ty as usize + py) * w as usize + tx as usize + px) * comps;
                    for k in 0..comps {
                        let mut v = component(self.prec, big_endian, tile.get(src + k * comp_size..).unwrap_or(&[]));
                        if self.prec.linear && k < color_comps {
                            v = linear_to_encoded(v);
                        }
                        if let Some(slot) = data.get_mut(dst + k) {
                            *slot = v;
                        }
                    }
                }
            }
        }
        Ok(Pixels { w, h, comps, data })
    }

    /// A layer record at `offset`; returns the layer's pieces.
    fn layer(&mut self, offset: usize) -> Result<RawLayer> {
        let saved = self.c.pos;
        self.c.pos = offset;
        let r = self.layer_here();
        self.c.pos = saved;
        r
    }

    fn layer_here(&mut self) -> Result<RawLayer> {
        let (w, h, ty) = (self.c.u32()?, self.c.u32()?, self.c.u32()?);
        let name = self.c.string()?;
        let props = self.props(None)?;
        let hierarchy = self.c.offset()?;
        let mask = self.c.offset()?;
        let effects = if self.version >= 20 { self.c.offset()? } else { 0 };
        let (has_alpha, gray, indexed) = match ty {
            0 => (false, false, false),
            1 => (true, false, false),
            2 => (false, true, false),
            3 => (true, true, false),
            4 => (false, false, true),
            5 => (true, false, true),
            _ => return Err(invalid(format!("unknown layer type {ty}"))),
        };
        let mut pixels = None;
        let drawable = !props.kind_group;
        if drawable && w > 0 && h > 0 {
            if w > MAX_SIDE || h > MAX_SIDE {
                return Err(invalid("a layer is too large"));
            }
            let comps = if indexed || gray { 1 } else { 3 } + usize::from(has_alpha);
            let color_comps = comps - usize::from(has_alpha);
            if hierarchy != 0 {
                pixels = Some(self.hierarchy(hierarchy, w, h, comps, if indexed { 0 } else { color_comps })?);
            }
        }
        let mut mask_pixels = None;
        if mask != 0 {
            let saved = self.c.pos;
            self.c.pos = mask;
            let m = (|| -> Result<Option<Pixels>> {
                let (mw, mh) = (self.c.u32()?, self.c.u32()?);
                let _name = self.c.string()?;
                let _mask_props = self.props(None)?;
                let hier = self.c.offset()?;
                if mw == 0 || mh == 0 || mw > MAX_SIDE || mh > MAX_SIDE || hier == 0 {
                    return Ok(None);
                }
                // Mask values are never linearised: they are coverage, not colour.
                self.hierarchy(hier, mw, mh, 1, 0).map(Some)
            })();
            self.c.pos = saved;
            mask_pixels = m?;
        }
        Ok(RawLayer { name, w, h, gray, indexed, has_alpha, props, pixels, mask: mask_pixels, effects: effects != 0 })
    }
}

struct RawLayer {
    name: String,
    w: u32,
    h: u32,
    gray: bool,
    indexed: bool,
    has_alpha: bool,
    props: Props,
    pixels: Option<Pixels>,
    mask: Option<Pixels>,
    effects: bool,
}

#[derive(Default)]
struct ImageProps {
    compression: u8,
    resolution: Option<(f32, f32)>,
    guides: Vec<(i32, u8)>,
    colormap: Vec<[u8; 3]>,
    has_paths: bool,
}

/// A tree of layers being assembled from the flat list and each layer's item path.
#[derive(Default)]
struct Slot {
    layer: Option<Layer>,
    kids: Vec<Slot>,
}

fn place(root: &mut Vec<Slot>, path: &[usize], layer: Layer) -> bool {
    let Some((last, parents)) = path.split_last() else { return false };
    let mut level = root;
    for &i in parents {
        if i >= MAX_NODES {
            return false;
        }
        if level.len() <= i {
            level.resize_with(i + 1, Slot::default);
        }
        let Some(slot) = level.get_mut(i) else { return false };
        level = &mut slot.kids;
    }
    if *last >= MAX_NODES {
        return false;
    }
    if level.len() <= *last {
        level.resize_with(*last + 1, Slot::default);
    }
    match level.get_mut(*last) {
        Some(slot) if slot.layer.is_none() => {
            slot.layer = Some(layer);
            true
        }
        _ => false,
    }
}

/// The slots as layers, bottom to top (the file lists the topmost first).
fn assemble(slots: Vec<Slot>, warnings: &mut Warnings) -> Vec<Layer> {
    let mut out = Vec::new();
    for slot in slots.into_iter().rev() {
        let kids = assemble(slot.kids, warnings);
        match slot.layer {
            Some(mut layer) => {
                if let Some(c) = layer.children_mut() {
                    *c = kids;
                } else if !kids.is_empty() {
                    out.extend(kids);
                }
                out.push(layer);
            }
            None => {
                // A group's missing parent: keep the children rather than lose them.
                if !kids.is_empty() {
                    warnings.note("some layers had no parent group in the file and were moved to the top level");
                    out.extend(kids);
                }
            }
        }
    }
    out
}

pub(crate) fn import(name: &str, bytes: &[u8], ctl: &Interrupt<'_>) -> Result<ImportResult> {
    let mut c = Cur { b: bytes, pos: 0, off: 4 };
    let id = c.take(14)?;
    if !id.starts_with(MAGIC) {
        return Err(invalid("not an XCF file"));
    }
    let version = if &id[9..13] == b"file" {
        0
    } else if id[9] == b'v' && id[13] == 0 {
        std::str::from_utf8(&id[10..13]).ok().and_then(|s| s.parse::<u32>().ok()).ok_or_else(|| invalid("bad XCF version"))?
    } else {
        return Err(invalid("bad XCF version"));
    };
    if version > 40 {
        return Err(invalid(format!("XCF version {version} is newer than this version of PhotoCraft understands")));
    }
    c.off = if version >= 11 { 8 } else { 4 };
    let (w, h, base) = (c.u32()?, c.u32()?, c.u32()?);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(invalid(format!("image size {w}×{h} is outside 1 to {MAX_SIDE} pixels")));
    }
    if base > 2 {
        return Err(invalid("unknown image type"));
    }
    let prec = if version >= 4 { precision_from(version, c.u32()?)? } else { Precision { bytes: 1, float: false, linear: false } };
    let mut cx = Ctx { c, version, prec, colormap: Vec::new(), compression: 0, budget: MAX_PIXEL_BYTES, ctl, warnings: Warnings::default() };
    let mut image = ImageProps::default();
    let doc_props = cx.props(Some(&mut image))?;
    cx.compression = image.compression;
    cx.colormap = std::mem::take(&mut image.colormap);
    let mut layer_offsets = Vec::new();
    loop {
        let o = cx.c.offset()?;
        if o == 0 {
            break;
        }
        if layer_offsets.len() >= MAX_NODES {
            return Err(invalid("too many layers"));
        }
        layer_offsets.push(o);
    }
    let mut channel_count = 0usize;
    while cx.c.offset().unwrap_or(0) != 0 {
        channel_count += 1;
        if channel_count > MAX_NODES {
            break;
        }
    }
    ctl.progress(0.1);

    let (doc_mode, sample) = {
        let mode = if base == 1 { ColorMode::Grayscale } else { ColorMode::Rgb };
        let sample = match (prec.bytes, prec.float) {
            (1, _) => SampleType::U8,
            (2, false) => SampleType::U16,
            _ => SampleType::F32,
        };
        (mode, sample)
    };
    let fmt = PixelFormat::new(doc_mode, sample, true);
    let mut slots: Vec<Slot> = Vec::new();
    let mut top_level = 0usize;
    let total = layer_offsets.len().max(1);
    for (i, &offset) in layer_offsets.iter().enumerate() {
        ctl.check().map_err(|_| IoError::Cancelled)?;
        ctl.progress(0.1 + 0.8 * (i as f32) / (total as f32));
        let raw = cx.layer(offset)?;
        let layer = build_layer(raw, fmt, &cx.colormap, doc_mode, &mut cx.warnings)?;
        let (layer, path) = layer;
        // Top-level layers carry no item path: they stack in file order, topmost first.
        let path = match path.filter(|p| p.len() >= 2) {
            Some(p) => p,
            None => {
                top_level += 1;
                vec![top_level - 1]
            }
        };
        if !place(&mut slots, &path, layer) {
            return Err(invalid("two layers claim the same place in the layer tree"));
        }
    }
    let mut warnings = std::mem::take(&mut cx.warnings);
    let layers = assemble(slots, &mut warnings);

    let mut document = Document::new(name, Size::new(w, h), doc_mode, sample);
    document.layers = layers;
    if let Some((x, _y)) = image.resolution.filter(|(x, y)| x.is_finite() && y.is_finite() && *x > 0.0 && *x < 1.0e6) {
        document.resolution_dpi = x;
    }
    for (pos, orientation) in &image.guides {
        match orientation {
            1 => document.guides.horizontal.push(f64::from(*pos)),
            2 => document.guides.vertical.push(f64::from(*pos)),
            _ => {}
        }
    }
    if let Some((_, data)) = doc_props.parasites.iter().find(|(n, _)| n == "icc-profile") {
        document.icc_profile = Some(Arc::new(data.clone()));
    }
    if base == 2 {
        warnings.note("indexed colour opens as RGB");
    }
    if prec.linear {
        warnings.note("the image was stored in linear light; its values were converted to the encoded ones PhotoCraft uses");
    }
    if prec.bytes >= 4 && !prec.float || prec.bytes == 8 {
        warnings.note("32-bit integer and 64-bit precision open as 32-bit float");
    }
    if channel_count > 0 {
        warnings.note("saved channels and the selection are not imported");
    }
    if image.has_paths {
        warnings.note("paths are not imported");
    }
    if document.layers.is_empty() {
        warnings.note("the XCF file has no layers");
    }
    Ok(ImportResult { document, warnings: warnings.finish(), source_read_only: false, preview_only: false })
}

fn build_layer(raw: RawLayer, fmt: PixelFormat, colormap: &[[u8; 3]], doc_mode: ColorMode, warnings: &mut Warnings) -> Result<(Layer, Option<Vec<usize>>)> {
    let RawLayer { name, w, h, gray, indexed, has_alpha, props, pixels, mask, effects } = raw;
    let name = if name.trim().is_empty() { "Layer".to_string() } else { name };
    let mut layer = if props.kind_group {
        let mut g = Layer::group(name.clone(), Vec::new());
        if let LayerContent::Group(group) = &mut g.content {
            group.expanded = props.expanded;
        }
        g
    } else {
        let mut surface = Surface::new(fmt);
        if let Some(p) = &pixels {
            // Convert to the document's channel layout: RGBA or grey + alpha.
            let n = p.w as usize * p.h as usize;
            let out_comps = fmt.channels();
            let mut out = vec![0.0f32; n * out_comps];
            for i in 0..n {
                let src = &p.data[i * p.comps..(i + 1) * p.comps];
                let alpha = if has_alpha { src[p.comps - 1] } else { 1.0 };
                let dst = &mut out[i * out_comps..(i + 1) * out_comps];
                if indexed {
                    // Indexed values are palette indices; they were read as 0..=1 fractions of 255.
                    let idx = (src[0] * 255.0).round().clamp(0.0, 255.0) as usize;
                    let e = colormap.get(idx).copied().unwrap_or([0, 0, 0]);
                    dst[0] = f32::from(e[0]) / 255.0;
                    dst[1] = f32::from(e[1]) / 255.0;
                    dst[2] = f32::from(e[2]) / 255.0;
                    dst[3] = alpha;
                } else if gray && doc_mode == ColorMode::Grayscale {
                    dst[0] = src[0];
                    dst[1] = alpha;
                } else if gray {
                    dst[0] = src[0];
                    dst[1] = src[0];
                    dst[2] = src[0];
                    dst[3] = alpha;
                } else {
                    dst[0] = src[0];
                    dst[1] = src[1];
                    dst[2] = src[2];
                    dst[3] = alpha;
                }
            }
            let rect = Rect::new(props.offset.0, props.offset.1, props.offset.0.saturating_add(p.w as i32), props.offset.1.saturating_add(p.h as i32));
            surface.write_region(rect, &out);
            surface.prune();
        } else if w > 0 && h > 0 {
            warnings.layer(&name, "the layer's pixel data is missing; the layer opens empty");
        }
        Layer::new(name.clone(), LayerContent::Raster(surface))
    };
    layer.visible = props.visible;
    layer.opacity = props.opacity;
    layer.locks.transparency = props.lock_alpha;
    layer.locks.pixels = props.lock_content;
    layer.locks.position = props.lock_position;
    match mode_from_xcf(props.mode) {
        Some((mode, approx)) => {
            layer.blend = if props.kind_group && mode == BlendMode::Normal { BlendMode::Normal } else { mode };
            if approx {
                warnings.layer(&name, "its blend mode is only approximated");
            }
        }
        None => warnings.layer(&name, "its blend mode has no PhotoCraft equivalent and was set to Normal"),
    }
    if props.kind_text {
        warnings.layer(&name, "text layers open as pixels");
    }
    if props.kind_vector {
        warnings.layer(&name, "vector and link layers open as pixels");
    }
    if effects {
        warnings.layer(&name, "non-destructive filters are not applied");
    }
    if props.linear_light {
        warnings.layer(&name, "it blends in linear light, which PhotoCraft does not; the result can differ slightly");
    }
    if let Some(m) = mask {
        let mut surface = Surface::new(PixelFormat::GRAY8);
        let rect = Rect::new(props.offset.0, props.offset.1, props.offset.0.saturating_add(m.w as i32), props.offset.1.saturating_add(m.h as i32));
        surface.write_region(rect, &m.data);
        let mut lm = LayerMask::reveal_all();
        lm.surface = surface;
        lm.enabled = props.apply_mask;
        layer.mask = Some(lm);
    }
    Ok((layer, props.path))
}

// ---------------------------------------------------------------------------------------------
// Writing

struct Out {
    b: Vec<u8>,
}

impl Out {
    fn u8(&mut self, v: u8) {
        self.b.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }
    fn offset(&mut self, v: usize) {
        self.b.extend_from_slice(&(v as u64).to_be_bytes());
    }
    /// A placeholder offset, to be filled with [`Out::patch`].
    fn hole(&mut self) -> usize {
        let at = self.b.len();
        self.offset(0);
        at
    }
    fn patch(&mut self, at: usize, v: usize) {
        if let Some(s) = self.b.get_mut(at..at + 8) {
            s.copy_from_slice(&(v as u64).to_be_bytes());
        }
    }
    fn string(&mut self, s: &str) {
        let bytes: Vec<u8> = s.bytes().filter(|b| *b != 0).collect();
        self.u32(bytes.len() as u32 + 1);
        self.b.extend_from_slice(&bytes);
        self.b.push(0);
    }
    fn prop(&mut self, kind: u32, payload: &[u8]) {
        self.u32(kind);
        self.u32(payload.len() as u32);
        self.b.extend_from_slice(payload);
    }
    fn end_props(&mut self) {
        self.u32(PROP_END);
        self.u32(0);
    }
}

/// Run-length encodes one byte plane of a tile.
fn rle_plane(plane: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < plane.len() {
        let b = plane[i];
        let mut run = 1;
        while i + run < plane.len() && plane[i + run] == b && run < 65535 {
            run += 1;
        }
        if run >= 3 {
            if run <= 127 {
                out.push((run - 1) as u8);
            } else {
                out.push(127);
                out.extend_from_slice(&(run as u16).to_be_bytes());
            }
            out.push(b);
            i += run;
        } else {
            // A literal stretch up to the next run of three, at most 127 bytes (or a long one).
            let mut end = i + run;
            while end < plane.len() && end - i < 127 {
                if end + 2 < plane.len() && plane[end] == plane[end + 1] && plane[end] == plane[end + 2] {
                    break;
                }
                end += 1;
            }
            let len = end - i;
            out.push((256 - len) as u8);
            out.extend_from_slice(&plane[i..end]);
            i = end;
        }
    }
}

/// Writes a hierarchy (one level) of `w × h` pixels with `bpp` bytes each; returns its offset.
fn write_hierarchy(out: &mut Out, w: u32, h: u32, bpp: usize, pixels: &[u8]) -> usize {
    let start = out.b.len();
    out.u32(w);
    out.u32(h);
    out.u32(bpp as u32);
    let level_ptr = out.hole();
    out.offset(0);
    let level = out.b.len();
    out.patch(level_ptr, level);
    out.u32(w);
    out.u32(h);
    let cols = w.div_ceil(TILE) as usize;
    let rows = h.div_ceil(TILE) as usize;
    let table = out.b.len();
    for _ in 0..cols * rows {
        out.offset(0);
    }
    out.offset(0);
    for t in 0..cols * rows {
        let (tx, ty) = ((t % cols) as u32 * TILE, (t / cols) as u32 * TILE);
        let (tw, th) = ((w - tx).min(TILE) as usize, (h - ty).min(TILE) as usize);
        let mut tile = Vec::with_capacity(tw * th * bpp);
        for y in 0..th {
            let row = ((ty as usize + y) * w as usize + tx as usize) * bpp;
            tile.extend_from_slice(&pixels[row..row + tw * bpp]);
        }
        let at = out.b.len();
        out.patch(table + t * 8, at);
        for plane in 0..bpp {
            let bytes: Vec<u8> = tile.iter().skip(plane).step_by(bpp).copied().collect();
            rle_plane(&bytes, &mut out.b);
        }
    }
    start
}

fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

struct Writer<'a> {
    doc: &'a Document,
    out: Out,
    warnings: Warnings,
    gray: bool,
    /// Layer record offsets still to be filled in the pointer table.
    records: Vec<(usize, Prepared)>,
    dry: bool,
}

/// A layer ready to write: its geometry, pixels and properties.
struct Prepared {
    name: String,
    rect: Rect,
    group: bool,
    rgba: Vec<[f32; 4]>,
    mask: Option<Vec<f32>>,
    props: Vec<(u32, Vec<u8>)>,
}

impl Writer<'_> {
    fn pixels(&mut self, layer: &Layer) -> (Rect, Vec<[f32; 4]>) {
        let canvas = self.doc.bounds();
        let (rect, mut buf) = match &layer.content {
            LayerContent::Raster(s) => {
                let r = s.content_bounds();
                (r, photocraft_compose::surface_to_buffer(s, r).px)
            }
            _ => {
                let mut solo = layer.clone();
                solo.visible = true;
                solo.opacity = 1.0;
                solo.fill_opacity = 1.0;
                solo.blend = BlendMode::Normal;
                solo.clipped = false;
                solo.mask = None;
                solo.effects = Default::default();
                let r = photocraft_compose::composite_bounds(&solo, canvas).unwrap_or(canvas).intersect(&canvas);
                (r, photocraft_compose::render_layer(&solo, r).px)
            }
        };
        if layer.fill_opacity < 1.0 {
            for p in &mut buf {
                p[3] *= layer.fill_opacity.clamp(0.0, 1.0);
            }
        }
        if rect.is_empty() { (Rect::new(0, 0, 1, 1), vec![[0.0; 4]]) } else { (rect, buf) }
    }

    fn warn(&mut self, layer: &Layer) {
        if !matches!(layer.content, LayerContent::Raster(_) | LayerContent::Group(_)) {
            self.warnings.layer(&layer.name, &format!("{} layers are saved to XCF as pixels", layer.content.kind_name().to_lowercase()));
        }
        if !layer.effects.items.is_empty() {
            self.warnings.layer(&layer.name, "layer styles are not saved to XCF");
        }
        if layer.clipped {
            self.warnings.layer(&layer.name, "clipping masks are not saved to XCF");
        }
        if layer.vector_mask.is_some() {
            self.warnings.layer(&layer.name, "vector masks are not saved to XCF");
        }
        if mode_to_xcf(layer.blend).is_none() {
            self.warnings.layer(&layer.name, "its blend mode has no GIMP equivalent and was saved as Normal");
        }
        if layer.mask.as_ref().is_some_and(|m| m.enabled && m.feather > 0.0) {
            self.warnings.layer(&layer.name, "layer mask feathering is not saved to XCF");
        }
    }

    fn collect(&mut self, layers: &[Layer], parent: &[u32], depth: usize) -> Result<()> {
        if depth > photocraft_doc::MAX_GROUP_DEPTH {
            return Err(IoError::Unsupported(format!("layer groups nested deeper than {}", photocraft_doc::MAX_GROUP_DEPTH)));
        }
        // The file lists the topmost layer first, each group before its children.
        for (index, layer) in layers.iter().rev().enumerate() {
            if matches!(layer.content, LayerContent::Adjustment(_)) {
                self.warnings.layer(&layer.name, "adjustment layers cannot be saved to XCF and were left out");
                continue;
            }
            self.warn(layer);
            let mut path = parent.to_vec();
            path.push(index as u32);
            let mut props: Vec<(u32, Vec<u8>)> = Vec::new();
            // A group's marker comes first: GIMP replaces the layer when it reads it, and keeps
            // only the properties read before it from the old layer.
            let group = matches!(layer.content, LayerContent::Group(_));
            if group {
                props.push((PROP_GROUP_ITEM, Vec::new()));
                if let LayerContent::Group(g) = &layer.content {
                    props.push((PROP_GROUP_ITEM_FLAGS, u32::from(g.expanded).to_be_bytes().to_vec()));
                }
            }
            let opacity = layer.opacity.clamp(0.0, 1.0);
            props.push((PROP_OPACITY, u32::from(quantize(opacity)).to_be_bytes().to_vec()));
            props.push((PROP_FLOAT_OPACITY, opacity.to_bits().to_be_bytes().to_vec()));
            props.push((PROP_MODE, mode_to_xcf(layer.blend).unwrap_or(28).to_be_bytes().to_vec()));
            props.push((PROP_VISIBLE, u32::from(layer.visible).to_be_bytes().to_vec()));
            props.push((PROP_LINKED, 0u32.to_be_bytes().to_vec()));
            props.push((PROP_LOCK_ALPHA, u32::from(layer.locks.transparency).to_be_bytes().to_vec()));
            props.push((PROP_LOCK_CONTENT, u32::from(layer.locks.pixels || layer.locks.all).to_be_bytes().to_vec()));
            props.push((PROP_LOCK_POSITION, u32::from(layer.locks.position || layer.locks.all).to_be_bytes().to_vec()));
            // Photoshop-style blending: on the encoded values, as the document is composited here.
            // Blend and composite on the encoded values, as every other editor does and as this
            // document is composited. Without these GIMP mixes new-style modes in linear light.
            props.push((PROP_COMPOSITE_SPACE, 2u32.to_be_bytes().to_vec()));
            props.push((PROP_BLEND_SPACE, 2u32.to_be_bytes().to_vec()));
            props.push((PROP_APPLY_MASK, u32::from(layer.mask.as_ref().is_some_and(|m| m.enabled)).to_be_bytes().to_vec()));
            props.push((PROP_EDIT_MASK, 0u32.to_be_bytes().to_vec()));
            props.push((PROP_SHOW_MASK, 0u32.to_be_bytes().to_vec()));
            // Only layers inside a group carry their path; top-level ones stack in file order.
            if path.len() >= 2 {
                props.push((PROP_ITEM_PATH, path.iter().flat_map(|p| p.to_be_bytes()).collect()));
            }
            let (rect, rgba) = if group || self.dry {
                (Rect::new(0, 0, 1, 1), vec![[0.0; 4]])
            } else {
                self.pixels(layer)
            };
            props.push((PROP_OFFSETS, [rect.x0.to_be_bytes(), rect.y0.to_be_bytes()].concat()));
            let mask = if self.dry || group {
                None
            } else {
                layer.mask.as_ref().filter(|m| m.enabled).map(|m| {
                    let mut values = Vec::new();
                    m.values_into(rect, &mut values);
                    values
                })
            };
            if group && layer.mask.as_ref().is_some_and(|m| m.enabled) {
                self.warnings.layer(&layer.name, "group masks are not saved to XCF");
            }
            self.records.push((0, Prepared { name: layer.name.clone(), rect, group, rgba, mask, props }));
            if let LayerContent::Group(g) = &layer.content {
                self.collect(&g.children, &path, depth + 1)?;
            }
        }
        Ok(())
    }

    fn write(&mut self) -> Result<()> {
        let doc = self.doc;
        let o = &mut self.out;
        o.b.extend_from_slice(MAGIC);
        o.b.extend_from_slice(format!("v{WRITE_VERSION:03}").as_bytes());
        o.u8(0);
        o.u32(doc.size.width);
        o.u32(doc.size.height);
        o.u32(if self.gray { 1 } else { 0 });
        o.u32(150); // 8-bit, non-linear
        o.prop(PROP_COMPRESSION, &[1]);
        let dpi = doc.resolution_dpi.clamp(1.0, 1.0e6);
        o.prop(PROP_RESOLUTION, &[dpi.to_bits().to_be_bytes(), dpi.to_bits().to_be_bytes()].concat());
        let mut guides = Vec::new();
        for g in &doc.guides.horizontal {
            guides.extend((g.round() as i32).to_be_bytes());
            guides.push(1);
        }
        for g in &doc.guides.vertical {
            guides.extend((g.round() as i32).to_be_bytes());
            guides.push(2);
        }
        if !guides.is_empty() {
            o.prop(PROP_GUIDES, &guides);
        }
        if let Some(icc) = doc.icc_profile.as_ref().filter(|p| !p.is_empty()) {
            let mut par = Out { b: Vec::new() };
            par.string("icc-profile");
            par.u32(1);
            par.u32(icc.len() as u32);
            par.b.extend_from_slice(icc);
            o.prop(PROP_PARASITES, &par.b);
        }
        o.end_props();
        let table = o.b.len();
        for _ in 0..self.records.len() {
            o.offset(0);
        }
        o.offset(0);
        o.offset(0); // no channels
        // Layer records, then hierarchies and masks.
        let records = std::mem::take(&mut self.records);
        let mut holes = Vec::new();
        for (i, (_, p)) in records.iter().enumerate() {
            let at = o.b.len();
            o.patch(table + i * 8, at);
            let (w, h) = (p.rect.width(), p.rect.height());
            o.u32(w);
            o.u32(h);
            o.u32(if self.gray { 3 } else { 1 });
            o.string(&p.name);
            for (kind, payload) in &p.props {
                o.prop(*kind, payload);
            }
            o.end_props();
            let hier = o.hole();
            let mask = o.hole();
            holes.push((hier, mask));
        }
        for ((_, p), (hier, mask)) in records.iter().zip(holes) {
            let (w, h) = (p.rect.width(), p.rect.height());
            let mut bytes = Vec::with_capacity(p.rgba.len() * 4);
            for px in &p.rgba {
                if self.gray {
                    bytes.push(quantize(px[0]));
                    bytes.push(quantize(px[3]));
                } else {
                    bytes.extend([quantize(px[0]), quantize(px[1]), quantize(px[2]), quantize(px[3])]);
                }
            }
            let bpp = if self.gray { 2 } else { 4 };
            let at = write_hierarchy(&mut self.out, w, h, bpp, &bytes);
            self.out.patch(hier, at);
            if let Some(values) = &p.mask {
                let at = self.out.b.len();
                self.out.patch(mask, at);
                self.out.u32(w);
                self.out.u32(h);
                self.out.string(&p.name);
                self.out.end_props();
                let hp = self.out.hole();
                let m: Vec<u8> = values.iter().map(|v| quantize(*v)).collect();
                let at = write_hierarchy(&mut self.out, w, h, 1, &m);
                self.out.patch(hp, at);
            }
            let _ = p.group;
        }
        Ok(())
    }
}

fn document_warnings(doc: &Document, warnings: &mut Warnings) {
    if doc.depth != SampleType::U8 {
        warnings.note("layers are saved as 8-bit");
    }
    if !doc.channels.is_empty() {
        warnings.note("alpha channels are not saved to XCF");
    }
    if !doc.paths.is_empty() {
        warnings.note("paths are not saved to XCF");
    }
    if !doc.metadata.text.is_empty() || doc.metadata.xmp.is_some() || doc.metadata.exif.is_some() {
        warnings.note("metadata (XMP, EXIF, text) is not saved to XCF");
    }
}

pub(crate) fn export(doc: &Document) -> Result<ExportResult> {
    if !matches!(doc.mode, ColorMode::Rgb | ColorMode::Grayscale) {
        return Err(IoError::Unsupported("XCF holds RGB or grayscale layers; convert the document to RGB or Grayscale first".into()));
    }
    let mut w = Writer { doc, out: Out { b: Vec::new() }, warnings: Warnings::default(), gray: doc.mode == ColorMode::Grayscale, records: Vec::new(), dry: false };
    document_warnings(doc, &mut w.warnings);
    w.collect(&doc.layers, &[], 1)?;
    w.write()?;
    Ok(ExportResult { bytes: w.out.b, warnings: w.warnings.finish() })
}

/// Everything saving `doc` to XCF would lose, without rendering or encoding anything.
pub fn losses(doc: &Document) -> Vec<String> {
    if !matches!(doc.mode, ColorMode::Rgb | ColorMode::Grayscale) {
        return vec!["XCF holds RGB or grayscale layers; convert the document to RGB or Grayscale first".into()];
    }
    let mut w = Writer { doc, out: Out { b: Vec::new() }, warnings: Warnings::default(), gray: false, records: Vec::new(), dry: true };
    document_warnings(doc, &mut w.warnings);
    if let Err(e) = w.collect(&doc.layers, &[], 1) {
        w.warnings.note(&e.to_string());
    }
    w.warnings.finish()
}

#[cfg(test)]
mod tests;
