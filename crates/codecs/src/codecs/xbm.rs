//! X BitMap (XBM) and Wireless Bitmap (WBMP): 1-bit images. Both read to 8-bit gray (black 0,
//! white 255) and write by thresholding at one half.
//!
//! XBM is C source: `#define name_width N`, `#define name_height N`, optional `_x_hot` /
//! `_y_hot`, then `static [unsigned] char name_bits[] = { 0x.., ... };`. Bits run least
//! significant first and 1 is black. The X10 form uses 16-bit `short` words.
//!
//! WBMP type 0 is a type byte (0), a fixed-header byte (0), width and height as multi-byte
//! integers (7 bits per byte, high bit = continues), then rows padded to whole bytes, most
//! significant bit first; 1 is white.

use super::legacy::{bit_at, pack_bits};
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image};
use crate::options::{EncodeOptions, Limits};
use crate::SampleType;

fn bad(f: Format, m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(f, m)
}

// ---- XBM -------------------------------------------------------------------------------------

/// A C source that defines `*_width`, `*_height` and `*_bits`.
pub(crate) fn looks_like_xbm(b: &[u8]) -> bool {
    let head = &b[..b.len().min(1024)];
    let Ok(s) = std::str::from_utf8(head).or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()])) else {
        return false;
    };
    let t = s.trim_start();
    (t.starts_with("#define") || t.starts_with("/*")) && s.contains("_width") && s.contains("_height") && (s.contains("_bits") || b.len() > head.len() || s.contains("{"))
}

fn define_value(text: &str, suffix: &str) -> Option<i64> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next()? == "#define").then_some(())?;
        let name = it.next()?;
        if !name.ends_with(suffix) {
            return None;
        }
        it.next()?.parse().ok()
    })
}

pub(crate) fn decode_xbm(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    let f = Format::Xbm;
    let text = String::from_utf8_lossy(b);
    let w = define_value(&text, "_width").filter(|v| *v > 0).ok_or_else(|| bad(f, "missing or bad _width"))?;
    let h = define_value(&text, "_height").filter(|v| *v > 0).ok_or_else(|| bad(f, "missing or bad _height"))?;
    let (w, h) = (u32::try_from(w).map_err(|_| bad(f, "width too large"))?, u32::try_from(h).map_err(|_| bad(f, "height too large"))?);
    limits.check(w, h, ChannelLayout::Gray, SampleType::U8)?;
    let open = text.find('{').ok_or_else(|| bad(f, "no data array"))?;
    let head = &text[..open];
    let wide = head.contains("short");
    let close = text[open..].find('}').map(|i| open + i).ok_or_else(|| bad(f, "unterminated data array"))?;
    let stride = if wide { (w as usize).div_ceil(16) * 2 } else { (w as usize).div_ceil(8) };
    let need = stride * h as usize;
    let mut data = Vec::with_capacity(need);
    for tok in text[open + 1..close].split(',') {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        let v = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => t.parse(),
        }
        .map_err(|_| bad(f, format!("bad value {t:?}")))?;
        if wide {
            // X10: each 16-bit word holds two bytes, low byte first.
            data.push(v as u8);
            data.push((v >> 8) as u8);
        } else {
            data.push(v as u8);
        }
        if data.len() > need {
            break;
        }
    }
    if data.len() < need {
        return Err(bad(f, "truncated bitmap data"));
    }
    let (wu, hu) = (w as usize, h as usize);
    let mut px = vec![255u8; wu * hu];
    for y in 0..hu {
        for x in 0..wu {
            if bit_at(&data, stride, x, y, false) {
                px[y * wu + x] = 0;
            }
        }
    }
    Image::from_u8(w, h, ChannelLayout::Gray, px)
}

pub(crate) fn encode_xbm(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rows = pack_bits(img.data(), w, h, false, |v| v < 128);
    let mut s = format!("#define image_width {w}\n#define image_height {h}\nstatic unsigned char image_bits[] = {{\n");
    for (i, chunk) in rows.chunks(12).enumerate() {
        s.push_str(if i == 0 { "  " } else { ",\n  " });
        s.push_str(&chunk.iter().map(|v| format!("0x{v:02x}")).collect::<Vec<_>>().join(", "));
    }
    s.push_str("\n};\n");
    Ok(s.into_bytes())
}

// ---- WBMP ------------------------------------------------------------------------------------

fn mbint(b: &[u8], pos: &mut usize) -> Option<u32> {
    let mut v: u32 = 0;
    for _ in 0..5 {
        let c = *b.get(*pos)?;
        *pos += 1;
        v = v.checked_mul(128)? | u32::from(c & 0x7f);
        if c & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// A type-0 WBMP is recognised by its exact length: it has no magic number.
pub(crate) fn looks_like_wbmp(b: &[u8]) -> bool {
    if b.len() < 5 || b[0] != 0 || b[1] != 0 {
        return false;
    }
    let mut pos = 2;
    let (Some(w), Some(h)) = (mbint(b, &mut pos), mbint(b, &mut pos)) else {
        return false;
    };
    w > 0 && h > 0 && (b.len() - pos) as u64 == u64::from(w.div_ceil(8)) * u64::from(h)
}

pub(crate) fn decode_wbmp(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    let f = Format::Wbmp;
    if b.len() < 2 || b[0] != 0 || b[1] != 0 {
        return Err(bad(f, "only type 0 WBMP with no extension headers is supported"));
    }
    let mut pos = 2;
    let (Some(w), Some(h)) = (mbint(b, &mut pos), mbint(b, &mut pos)) else {
        return Err(bad(f, "truncated header"));
    };
    limits.check(w, h, ChannelLayout::Gray, SampleType::U8)?;
    let (wu, hu) = (w as usize, h as usize);
    let stride = wu.div_ceil(8);
    let body = b.get(pos..).unwrap_or_default();
    if body.len() < stride * hu {
        return Err(bad(f, "truncated bitmap data"));
    }
    let mut px = vec![0u8; wu * hu];
    for y in 0..hu {
        for x in 0..wu {
            px[y * wu + x] = if bit_at(body, stride, x, y, true) { 255 } else { 0 };
        }
    }
    Image::from_u8(w, h, ChannelLayout::Gray, px)
}

pub(crate) fn encode_wbmp(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = vec![0u8, 0u8];
    for v in [w as u32, h as u32] {
        let mut groups = Vec::new();
        let mut n = v;
        loop {
            groups.push((n & 0x7f) as u8);
            n >>= 7;
            if n == 0 {
                break;
            }
        }
        for (i, g) in groups.iter().enumerate().rev() {
            out.push(if i == 0 { *g } else { *g | 0x80 });
        }
    }
    out.extend(pack_bits(img.data(), w, h, true, |v| v >= 128));
    Ok(out)
}
