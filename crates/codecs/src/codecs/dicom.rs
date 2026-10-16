//! DICOM (`.dcm`): the pixel data of a single image or the first frame of a multi-frame one.
//!
//! Reads uncompressed pixel data in implicit or explicit little-endian transfer syntaxes, RLE
//! lossless, and encapsulated JPEG baseline and JPEG 2000 (through those codecs). Photometric
//! interpretations MONOCHROME1/2, RGB, YBR_FULL and PALETTE COLOR; 8 to 16 stored bits, signed
//! or not. Rescale slope/intercept and the first window centre/width are applied when present;
//! a signed or rescaled image without a window is stretched over its range. Anything without
//! those (a plain 8 or 16-bit image) is read as stored.
//!
//! Writes an uncompressed explicit-VR little-endian Secondary Capture image (gray or RGB, 8 or
//! 16 bits). Patient and study fields are left empty.

use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Dicom;
const IMPLICIT_LE: &str = "1.2.840.10008.1.2";
const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";
const EXPLICIT_BE: &str = "1.2.840.10008.1.2.2";
const RLE: &str = "1.2.840.10008.1.2.5";

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

pub(crate) fn is_dicom(b: &[u8]) -> bool {
    b.get(128..132) == Some(b"DICM")
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[derive(Default)]
struct Attrs {
    samples: u16,
    photometric: String,
    planar: u16,
    rows: u32,
    cols: u32,
    allocated: u16,
    stored: u16,
    signed: bool,
    slope: f64,
    intercept: f64,
    window: Option<(f64, f64)>,
    palette_desc: Vec<u16>,
    palette: [Vec<u8>; 3],
    transfer: String,
    /// Pixel data as one buffer (native) or its fragments (encapsulated).
    native: Option<(usize, usize)>,
    fragments: Vec<(usize, usize)>,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string()
}

fn ds_values(b: &[u8]) -> Vec<f64> {
    text(b).split('\\').filter_map(|s| s.trim().parse().ok()).collect()
}

/// Skips the items of a sequence of undefined length, starting after its header; returns the
/// offset after its delimiter.
fn skip_undefined(b: &[u8], mut pos: usize, explicit: bool, depth: u32) -> Result<usize, CodecError> {
    if depth > 16 {
        return Err(bad("sequences nested too deeply"));
    }
    loop {
        let (g, e, len) = (le16(b, pos).ok_or_else(|| bad("truncated sequence"))?, le16(b, pos + 2).ok_or_else(|| bad("truncated sequence"))?, le32(b, pos + 4).ok_or_else(|| bad("truncated sequence"))?);
        pos += 8;
        match (g, e) {
            (0xfffe, 0xe0dd) => return Ok(pos),
            (0xfffe, 0xe000) if len == 0xffff_ffff => pos = skip_item_elements(b, pos, explicit, depth)?,
            (0xfffe, 0xe000) => pos = pos.checked_add(len as usize).filter(|p| *p <= b.len()).ok_or_else(|| bad("item past the end"))?,
            _ => return Err(bad("unexpected tag inside a sequence")),
        }
    }
}

/// Elements of an undefined-length item up to its item delimiter.
fn skip_item_elements(b: &[u8], mut pos: usize, explicit: bool, depth: u32) -> Result<usize, CodecError> {
    loop {
        let (g, e) = (le16(b, pos).ok_or_else(|| bad("truncated item"))?, le16(b, pos + 2).ok_or_else(|| bad("truncated item"))?);
        if (g, e) == (0xfffe, 0xe00d) {
            return Ok(pos + 8);
        }
        let (_, len, header) = element_header(b, pos, explicit)?;
        pos += header;
        if len == 0xffff_ffff {
            pos = skip_undefined(b, pos, explicit, depth + 1)?;
        } else {
            pos = pos.checked_add(len as usize).filter(|p| *p <= b.len()).ok_or_else(|| bad("element past the end"))?;
        }
    }
}

/// (VR, length, header size) of the element at `pos`.
fn element_header(b: &[u8], pos: usize, explicit: bool) -> Result<([u8; 2], u32, usize), CodecError> {
    let t = || bad("truncated element header");
    if !explicit {
        return Ok(([0, 0], le32(b, pos + 4).ok_or_else(t)?, 8));
    }
    let vr = [*b.get(pos + 4).ok_or_else(t)?, *b.get(pos + 5).ok_or_else(t)?];
    if matches!(&vr, b"OB" | b"OD" | b"OF" | b"OL" | b"OV" | b"OW" | b"SQ" | b"UN" | b"UT" | b"UC" | b"UR") {
        Ok((vr, le32(b, pos + 8).ok_or_else(t)?, 12))
    } else {
        Ok((vr, u32::from(le16(b, pos + 6).ok_or_else(t)?), 8))
    }
}

fn parse(b: &[u8]) -> Result<Attrs, CodecError> {
    let mut a = Attrs { samples: 1, slope: 1.0, ..Default::default() };
    let (mut pos, mut transfer_known) = (132usize, false);
    let mut explicit = true;
    while pos + 8 <= b.len() {
        let (g, e) = (le16(b, pos).unwrap_or(0), le16(b, pos + 2).unwrap_or(0));
        // The dataset starts after group 0002, in the transfer syntax the meta header names.
        if g != 2 && !transfer_known {
            transfer_known = true;
            match a.transfer.as_str() {
                "" | EXPLICIT_LE => explicit = true,
                IMPLICIT_LE => explicit = false,
                EXPLICIT_BE => return Err(CodecError::unsupported(F, "big-endian DICOM")),
                _ => explicit = true,
            }
        }
        let this_explicit = if g == 2 { true } else { explicit };
        let (_, len, header) = element_header(b, pos, this_explicit)?;
        let body = pos + header;
        if (g, e) == (0x7fe0, 0x0010) {
            if len == 0xffff_ffff {
                // Encapsulated: a basic offset table item, then the fragments.
                let mut p = body;
                let mut first = true;
                loop {
                    let (ig, ie, il) = (le16(b, p).ok_or_else(|| bad("truncated pixel data"))?, le16(b, p + 2).ok_or_else(|| bad("truncated pixel data"))?, le32(b, p + 4).ok_or_else(|| bad("truncated pixel data"))? as usize);
                    p += 8;
                    if (ig, ie) == (0xfffe, 0xe0dd) {
                        break;
                    }
                    if (ig, ie) != (0xfffe, 0xe000) || p.checked_add(il).is_none_or(|end| end > b.len()) {
                        return Err(bad("bad encapsulated pixel data"));
                    }
                    if !first {
                        a.fragments.push((p, il));
                    }
                    first = false;
                    p += il;
                }
            } else {
                a.native = Some((body, (len as usize).min(b.len().saturating_sub(body))));
            }
            break;
        }
        if len == 0xffff_ffff {
            pos = skip_undefined(b, body, this_explicit, 0)?;
            continue;
        }
        let end = body.checked_add(len as usize).filter(|p| *p <= b.len()).ok_or_else(|| bad("element past the end"))?;
        let v = &b[body..end];
        match (g, e) {
            (0x0002, 0x0010) => a.transfer = text(v),
            (0x0028, 0x0002) => a.samples = le16(v, 0).unwrap_or(1),
            (0x0028, 0x0004) => a.photometric = text(v),
            (0x0028, 0x0006) => a.planar = le16(v, 0).unwrap_or(0),
            (0x0028, 0x0010) => a.rows = u32::from(le16(v, 0).unwrap_or(0)),
            (0x0028, 0x0011) => a.cols = u32::from(le16(v, 0).unwrap_or(0)),
            (0x0028, 0x0100) => a.allocated = le16(v, 0).unwrap_or(0),
            (0x0028, 0x0101) => a.stored = le16(v, 0).unwrap_or(0),
            (0x0028, 0x0103) => a.signed = le16(v, 0).unwrap_or(0) == 1,
            (0x0028, 0x1050) => {
                a.window = ds_values(v).first().map(|c| (*c, 0.0));
            }
            (0x0028, 0x1051) => {
                if let (Some((c, _)), Some(w)) = (a.window, ds_values(v).first()) {
                    a.window = Some((c, *w));
                }
            }
            (0x0028, 0x1052) => a.intercept = ds_values(v).first().copied().unwrap_or(0.0),
            (0x0028, 0x1053) => a.slope = ds_values(v).first().copied().unwrap_or(1.0),
            (0x0028, 0x1101) => a.palette_desc = v.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect(),
            (0x0028, 0x1201) => a.palette[0] = v.to_vec(),
            (0x0028, 0x1202) => a.palette[1] = v.to_vec(),
            (0x0028, 0x1203) => a.palette[2] = v.to_vec(),
            _ => {}
        }
        pos = end;
    }
    if a.window.is_some_and(|(_, w)| w <= 0.0) {
        a.window = None;
    }
    Ok(a)
}

/// PackBits-style segment decode used by DICOM RLE.
fn unpack_segment(data: &[u8], want: usize) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0;
    while out.len() < want && i < data.len() {
        let n = data[i] as i8;
        i += 1;
        if n >= 0 {
            let c = n as usize + 1;
            out.extend_from_slice(data.get(i..i + c).ok_or_else(|| bad("truncated RLE segment"))?);
            i += c;
        } else if n != -128 {
            let c = 1 - n as isize;
            out.resize(out.len() + c as usize, *data.get(i).ok_or_else(|| bad("truncated RLE segment"))?);
            i += 1;
        }
    }
    if out.len() < want {
        return Err(bad("RLE segment is too short"));
    }
    out.truncate(want);
    Ok(out)
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !is_dicom(b) {
        return Err(bad("missing DICM prefix"));
    }
    let a = parse(b)?;
    let (w, h) = (a.cols, a.rows);
    let compressed = !a.fragments.is_empty();
    if a.native.is_none() && !compressed {
        return Err(bad("no pixel data"));
    }
    if a.transfer.starts_with("1.2.840.10008.1.2.4.") && compressed {
        // Encapsulated JPEG baseline or JPEG 2000: hand the joined fragments to that codec.
        let joined: Vec<u8> = a.fragments.iter().flat_map(|(p, l)| b[*p..*p + *l].iter().copied()).collect();
        let tail = a.transfer.rsplit('.').next().unwrap_or("");
        return match tail {
            "50" | "51" => super::jpeg::decode(&joined, limits),
            "90" | "91" | "92" | "93" => super::jp2::decode(&joined, limits),
            _ => Err(CodecError::unsupported(F, format!("DICOM transfer syntax {}", a.transfer))),
        };
    }
    if !matches!(a.allocated, 8 | 16) || a.stored == 0 || a.stored > a.allocated || !matches!(a.samples, 1 | 3) {
        return Err(CodecError::unsupported(F, "unsupported DICOM sample layout"));
    }
    limits.check(w, h, ChannelLayout::Rgb, SampleType::U16)?;
    let (wu, hu, spp) = (w as usize, h as usize, usize::from(a.samples));
    let bytes = usize::from(a.allocated) / 8;
    let frame = wu * hu * spp * bytes;

    // One frame as bytes in sample order (interleaved).
    let data: Vec<u8> = if compressed {
        if a.transfer != RLE {
            return Err(CodecError::unsupported(F, format!("DICOM transfer syntax {}", a.transfer)));
        }
        let joined: Vec<u8> = a.fragments.iter().flat_map(|(p, l)| b[*p..*p + *l].iter().copied()).collect();
        let nseg = le32(&joined, 0).unwrap_or(0) as usize;
        if nseg == 0 || nseg > 15 || nseg != spp * bytes {
            return Err(bad("bad RLE segment table"));
        }
        let offs: Vec<usize> = (0..nseg).map(|i| le32(&joined, 4 + i * 4).unwrap_or(0) as usize).collect();
        let mut out = vec![0u8; frame];
        for (i, off) in offs.iter().enumerate() {
            let end = offs.get(i + 1).copied().unwrap_or(joined.len());
            let seg = unpack_segment(joined.get(*off..end).ok_or_else(|| bad("RLE segment outside the data"))?, wu * hu)?;
            // Segments run channel by channel, most significant byte first (little-endian data).
            let (ch, byte) = (i / bytes, bytes - 1 - i % bytes);
            for (p, v) in seg.iter().enumerate() {
                out[(p * spp + ch) * bytes + byte] = *v;
            }
        }
        out
    } else {
        let (pos, len) = a.native.unwrap_or((0, 0));
        let raw = &b[pos..pos + len];
        if raw.len() < frame {
            return Err(bad("truncated pixel data"));
        }
        let mut f = raw[..frame].to_vec();
        if a.planar == 1 && spp == 3 {
            let plane = wu * hu * bytes;
            let src = f.clone();
            for p in 0..wu * hu {
                for c in 0..3 {
                    f[(p * 3 + c) * bytes..(p * 3 + c + 1) * bytes].copy_from_slice(&src[c * plane + p * bytes..c * plane + (p + 1) * bytes]);
                }
            }
        }
        f
    };

    let value = |i: usize| -> i64 {
        let raw = if bytes == 1 { i64::from(data[i]) } else { i64::from(u16::from_le_bytes([data[i * 2], data[i * 2 + 1]])) };
        let masked = raw & ((1i64 << a.stored) - 1);
        if a.signed && masked >= 1i64 << (a.stored - 1) { masked - (1i64 << a.stored) } else { masked }
    };
    let photometric = a.photometric.as_str();
    if spp == 3 {
        let max = ((1u32 << a.stored) - 1) as f64;
        let out_max = if bytes == 2 { 65535.0 } else { 255.0 };
        let mut out = Vec::with_capacity(wu * hu * 3);
        for p in 0..wu * hu {
            let s = [value(p * 3), value(p * 3 + 1), value(p * 3 + 2)];
            let rgb: [f64; 3] = if photometric.starts_with("YBR_FULL") {
                let (y, cb, cr) = (s[0] as f64, s[1] as f64 - (max + 1.0) / 2.0, s[2] as f64 - (max + 1.0) / 2.0);
                [y + 1.402 * cr, y - 0.344136 * cb - 0.714136 * cr, y + 1.772 * cb]
            } else {
                [s[0] as f64, s[1] as f64, s[2] as f64]
            };
            out.extend(rgb.iter().map(|v| (v.clamp(0.0, max) * out_max / max).round() as u16));
        }
        return if bytes == 2 { Image::from_u16(w, h, ChannelLayout::Rgb, &out) } else { Image::from_u8(w, h, ChannelLayout::Rgb, out.iter().map(|v| *v as u8).collect()) };
    }
    if photometric == "PALETTE COLOR" {
        let n = match a.palette_desc.first() {
            Some(0) => 65536,
            Some(n) => usize::from(*n),
            None => return Err(bad("palette color without a palette")),
        };
        let first = i64::from(a.palette_desc.get(1).copied().unwrap_or(0));
        let bits = a.palette_desc.get(2).copied().unwrap_or(16);
        let entry = |ch: usize, i: i64| -> u8 {
            let i = (i - first).clamp(0, n as i64 - 1) as usize;
            let d = &a.palette[ch];
            let v = if bits == 8 { d.get(i).copied().map(u16::from).unwrap_or(0) } else { le16(d, i * 2).unwrap_or(0) };
            if bits == 8 { v as u8 } else { (v >> 8) as u8 }
        };
        let mut out = Vec::with_capacity(wu * hu * 3);
        for p in 0..wu * hu {
            let v = value(p);
            out.extend([entry(0, v), entry(1, v), entry(2, v)]);
        }
        return Image::from_u8(w, h, ChannelLayout::Rgb, out);
    }
    // Grayscale: window, rescale, or as stored.
    let plain = !a.signed && a.slope == 1.0 && a.intercept == 0.0 && a.window.is_none();
    let invert = photometric == "MONOCHROME1";
    let wide = a.allocated == 16;
    if plain {
        let max = ((1u32 << a.stored) - 1) as f64;
        let out_max = if wide { 65535.0 } else { 255.0 };
        let samples: Vec<u16> = (0..wu * hu).map(|p| {
            let v = (value(p) as f64 * out_max / max).round().clamp(0.0, out_max) as u16;
            if invert { out_max as u16 - v } else { v }
        }).collect();
        return if wide { Image::from_u16(w, h, ChannelLayout::Gray, &samples) } else { Image::from_u8(w, h, ChannelLayout::Gray, samples.iter().map(|v| *v as u8).collect()) };
    }
    let real: Vec<f64> = (0..wu * hu).map(|p| value(p) as f64 * a.slope + a.intercept).collect();
    let (lo, hi) = match a.window {
        Some((c, w)) => (c - 0.5 - (w - 1.0) / 2.0, c - 0.5 + (w - 1.0) / 2.0),
        None => real.iter().fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v))),
    };
    let span = (hi - lo).max(f64::EPSILON);
    let samples: Vec<u16> = real.iter().map(|v| {
        let t = ((v - lo) / span).clamp(0.0, 1.0);
        let t = if invert { 1.0 - t } else { t };
        (t * 65535.0).round() as u16
    }).collect();
    Image::from_u16(w, h, ChannelLayout::Gray, &samples)
}

fn element(out: &mut Vec<u8>, g: u16, e: u16, vr: &[u8; 2], value: &[u8]) {
    let mut v = value.to_vec();
    if v.len() % 2 == 1 {
        v.push(if matches!(vr, b"UI") || matches!(vr, b"OB") { 0 } else { b' ' });
    }
    out.extend(g.to_le_bytes());
    out.extend(e.to_le_bytes());
    out.extend(vr);
    if matches!(vr, b"OB" | b"OW") {
        out.extend([0, 0]);
        out.extend((v.len() as u32).to_le_bytes());
    } else {
        out.extend((v.len() as u16).to_le_bytes());
    }
    out.extend(v);
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width(), img.height());
    if w > 65535 || h > 65535 {
        return Err(CodecError::encode(F, "DICOM is limited to 65535 pixels per side"));
    }
    let rgb = img.layout() == ChannelLayout::Rgb;
    let wide = img.sample_type() == SampleType::U16;
    let mut pixels = Vec::new();
    if wide {
        for v in img.to_u16_samples().unwrap_or_default() {
            pixels.extend(v.to_le_bytes());
        }
    } else {
        pixels.extend_from_slice(img.data());
    }
    const SC: &str = "1.2.840.10008.5.1.4.1.1.7";
    // A UID from the pixels: the same image always writes the same file.
    let uid = format!("2.25.{}", img.data().iter().fold(0x9e37_79b9_7f4a_7c15u64, |h, v| (h ^ u64::from(*v)).wrapping_mul(0x0100_0000_01b3)));
    let mut meta = Vec::new();
    element(&mut meta, 2, 1, b"OB", &[0, 1]);
    element(&mut meta, 2, 2, b"UI", SC.as_bytes());
    element(&mut meta, 2, 3, b"UI", uid.as_bytes());
    element(&mut meta, 2, 0x10, b"UI", EXPLICIT_LE.as_bytes());
    element(&mut meta, 2, 0x12, b"UI", b"1.2.826.0.1.3680043.9.7133.1");
    let mut out = vec![0u8; 128];
    out.extend(b"DICM");
    let mut group_len = Vec::new();
    element(&mut group_len, 2, 0, b"UL", &(meta.len() as u32).to_le_bytes());
    out.extend(group_len);
    out.extend(meta);
    element(&mut out, 8, 0x16, b"UI", SC.as_bytes());
    element(&mut out, 8, 0x18, b"UI", uid.as_bytes());
    element(&mut out, 8, 0x60, b"CS", b"OT");
    element(&mut out, 0x10, 0x10, b"PN", b"");
    element(&mut out, 0x28, 2, b"US", &(if rgb { 3u16 } else { 1 }).to_le_bytes());
    element(&mut out, 0x28, 4, b"CS", if rgb { b"RGB" } else { b"MONOCHROME2" });
    if rgb {
        element(&mut out, 0x28, 6, b"US", &0u16.to_le_bytes());
    }
    element(&mut out, 0x28, 0x10, b"US", &(h as u16).to_le_bytes());
    element(&mut out, 0x28, 0x11, b"US", &(w as u16).to_le_bytes());
    let bits: u16 = if wide { 16 } else { 8 };
    for (e, v) in [(0x100u16, bits), (0x101, bits), (0x102, bits - 1), (0x103, 0)] {
        element(&mut out, 0x28, e, b"US", &v.to_le_bytes());
    }
    element(&mut out, 0x7fe0, 0x10, if wide { b"OW" } else { b"OB" }, &pixels);
    Ok(out)
}
