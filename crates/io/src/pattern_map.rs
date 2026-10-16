//! Patterns ⇄ PSD: the global `Patt`/`Pat2`/`Pat3` blocks and `.pat` files.
//!
//! Import parses the blocks into [`Document::patterns`] and keeps the raw blocks. Export reuses
//! the raw blocks verbatim while they still decode to the document's patterns (byte-exact round
//! trips); otherwise they are replaced by one block written from the document.

use std::sync::Arc;

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Pattern};
use photocraft_geom::Rect;
use photocraft_psd::patterns::{PatternBudget, PsdPattern, block_key, mode_channels, parse_pattern_block_with, write_pattern_block};
use photocraft_raster::Surface;

use crate::pixels::{deinterleave, interleave, max_sample, psd_depth, sample_for_depth, zero_sample};

/// Global block keys holding patterns.
pub const PATTERN_KEYS: [[u8; 4]; 3] = [*b"Patt", *b"Pat2", *b"Pat3"];

fn psd_mode(m: ColorMode) -> u32 {
    match m {
        ColorMode::Grayscale => 1,
        ColorMode::Cmyk => 4,
        ColorMode::Lab => 9,
        _ => 3,
    }
}

/// A PSD pattern as a document pattern (indexed patterns become RGB through their palette).
pub fn from_psd(p: &PsdPattern) -> Option<Pattern> {
    let (w, h) = (p.width as usize, p.height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    // 1-bit patterns: expand to 8-bit grey.
    let (channels, depth): (Vec<Vec<u8>>, u16) = if p.depth == 1 {
        let row = w.div_ceil(8);
        let expand = |plane: &Vec<u8>| -> Vec<u8> {
            (0..w * h).map(|i| if plane.get((i / w) * row + (i % w) / 8).is_some_and(|b| b & (0x80 >> (i % w % 8)) != 0) { 0 } else { 255 }).collect()
        };
        (p.channels.iter().map(expand).collect(), 8)
    } else {
        (p.channels.clone(), p.depth)
    };
    let (mode, channels) = match p.mode {
        1 | 8 => (ColorMode::Grayscale, channels),
        2 => {
            let pal = p.palette.as_deref()?;
            let idx = channels.first()?;
            let ch: Vec<Vec<u8>> = (0..3).map(|c| idx.iter().map(|i| pal.get(usize::from(*i) * 3 + c).copied().unwrap_or(0)).collect()).collect();
            (ColorMode::Rgb, ch)
        }
        4 => (ColorMode::Cmyk, channels),
        9 => (ColorMode::Lab, channels),
        3 => (ColorMode::Rgb, channels),
        _ => (ColorMode::Grayscale, channels.into_iter().take(1).collect()),
    };
    let s = sample_for_depth(depth);
    let fmt = PixelFormat::new(mode, s, p.alpha.is_some());
    let cc = mode_channels(psd_mode(mode));
    if channels.len() < cc {
        return None;
    }
    let mut refs: Vec<Option<&[u8]>> = channels.iter().take(cc).map(|c| Some(c.as_slice())).collect();
    let mut fill = vec![zero_sample(s); cc];
    let mut invert = vec![mode == ColorMode::Cmyk; cc];
    if let Some(a) = &p.alpha {
        refs.push(Some(a.as_slice()));
        fill.push(max_sample(s));
        invert.push(false);
    }
    let mut bytes = interleave(&refs, &fill, w * h, s, &invert);
    if mode == ColorMode::Lab && s == SampleType::U16 {
        crate::pixels::lab16_chroma(&mut bytes, refs.len(), true);
    }
    let surface = Surface::from_interleaved(fmt, Rect::new(0, 0, w as i32, h as i32), &bytes);
    Some(Pattern { id: p.id.clone(), name: p.name.clone(), width: p.width, height: p.height, surface })
}

/// A document pattern as a PSD pattern (its own depth and colour model; other models as RGB).
pub fn to_psd(p: &Pattern) -> PsdPattern {
    let f = p.surface.format();
    let mode = match f.mode {
        ColorMode::Grayscale | ColorMode::Rgb | ColorMode::Cmyk | ColorMode::Lab => f.mode,
        _ => ColorMode::Rgb,
    };
    let fmt = PixelFormat::new(mode, f.sample, f.alpha);
    let surf = if fmt == f { p.surface.clone() } else { p.surface.convert(fmt) };
    let mut bytes = surf.to_interleaved(p.rect());
    let ch = fmt.channels();
    if mode == ColorMode::Lab && fmt.sample == SampleType::U16 {
        crate::pixels::lab16_chroma(&mut bytes, ch, false);
    }
    let mut invert = vec![mode == ColorMode::Cmyk; ch];
    if fmt.alpha {
        invert[ch - 1] = false;
    }
    let mut planes = deinterleave(&bytes, ch, fmt.sample, &invert);
    let alpha = fmt.alpha.then(|| planes.pop()).flatten();
    PsdPattern {
        mode: psd_mode(mode),
        width: p.width,
        height: p.height,
        name: p.name.clone(),
        id: p.id.clone(),
        palette: None,
        depth: psd_depth(fmt.sample),
        channels: planes,
        alpha,
    }
}

/// Patterns decoded from the document's raw pattern blocks (malformed blocks are skipped). All
/// blocks share one memory budget, so repeating a block can't multiply it.
pub fn from_global_blocks(doc: &Document) -> Vec<Pattern> {
    let mut budget = PatternBudget::default();
    doc.metadata
        .psd_global_blocks
        .iter()
        .filter(|(_, k, _)| PATTERN_KEYS.contains(k))
        .filter_map(|(_, _, d)| parse_pattern_block_with(d, &mut budget).ok())
        .flatten()
        .filter_map(|p| from_psd(&p))
        .collect()
}

/// Global blocks to write: the raw ones while they still match [`Document::patterns`], else the
/// raw pattern blocks replaced by one written from the document (at the first one's position).
pub fn export_global_blocks(doc: &Document) -> Vec<photocraft_doc::PsdGlobalBlock> {
    let raw = &doc.metadata.psd_global_blocks;
    if from_global_blocks(doc) == doc.patterns {
        return raw.clone();
    }
    let key = block_key(psd_depth(doc.depth));
    let fresh = write_pattern_block(&doc.patterns.iter().map(to_psd).collect::<Vec<_>>()).unwrap_or_default();
    let mut out = Vec::with_capacity(raw.len() + 1);
    let mut placed = false;
    for b in raw {
        if PATTERN_KEYS.contains(&b.1) {
            if !placed {
                out.push((*b"8BIM", key, Arc::new(fresh.clone())));
                placed = true;
            }
        } else {
            out.push(b.clone());
        }
    }
    if !placed && !doc.patterns.is_empty() {
        out.push((*b"8BIM", key, Arc::new(fresh)));
    }
    out
}

/// Largest side of a GIMP pattern tile read here.
const GPAT_MAX_SIDE: u32 = 16_384;

/// Reads a GIMP `.pat` file (`GPAT`): big-endian header size, version, width, height, bytes per
/// pixel (1 grey, 2 grey + alpha, 3 RGB, 4 RGBA), the magic, a NUL-terminated name, then
/// 8-bit interleaved pixels. One pattern per file.
fn read_gimp_pat(bytes: &[u8]) -> Result<Vec<Pattern>, String> {
    let bad = |what: &str| format!("not a readable GIMP .pat file: {what}");
    let word = |i: usize| bytes.get(i * 4..i * 4 + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes);
    let (Some(header), Some(version), Some(w), Some(h), Some(bpp)) = (word(0), word(1), word(2), word(3), word(4)) else {
        return Err(bad("truncated header"));
    };
    if version != 1 {
        return Err(bad(&format!("unsupported version {version}")));
    }
    if w == 0 || h == 0 || w > GPAT_MAX_SIDE || h > GPAT_MAX_SIDE {
        return Err(bad("bad size"));
    }
    let (mode, alpha) = match bpp {
        1 => (ColorMode::Grayscale, false),
        2 => (ColorMode::Grayscale, true),
        3 => (ColorMode::Rgb, false),
        4 => (ColorMode::Rgb, true),
        _ => return Err(bad("unsupported bytes per pixel")),
    };
    let header = header as usize;
    let name = bytes.get(24..header).ok_or_else(|| bad("truncated name"))?;
    let name = String::from_utf8_lossy(name.split(|b| *b == 0).next().unwrap_or_default()).trim().to_string();
    let len = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(bpp as usize)).ok_or_else(|| bad("bad size"))?;
    let pixels = header.checked_add(len).and_then(|end| bytes.get(header..end)).ok_or_else(|| bad("truncated pixels"))?;
    let surface = Surface::from_interleaved(PixelFormat::new(mode, SampleType::U8, alpha), Rect::new(0, 0, w as i32, h as i32), pixels);
    let name = if name.is_empty() { "Pattern".to_string() } else { name };
    Ok(vec![Pattern::new(name, surface, w, h)])
}

/// Reads a `.pat` pattern file: Photoshop's (`8BPT`) or GIMP's (`GPAT`).
pub fn read_pat(bytes: &[u8]) -> Result<Vec<Pattern>, String> {
    if bytes.get(20..24) == Some(b"GPAT") {
        return read_gimp_pat(bytes);
    }
    let ps = photocraft_psd::patterns::parse_pat_file(bytes).map_err(|e| format!("not a readable .pat file: {e}"))?;
    Ok(ps.iter().filter_map(from_psd).collect())
}

/// Writes patterns as a `.pat` file.
pub fn write_pat(patterns: &[Pattern]) -> Result<Vec<u8>, String> {
    photocraft_psd::patterns::write_pat_file(&patterns.iter().map(to_psd).collect::<Vec<_>>()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::SampleType;

    fn pattern(mode: ColorMode, s: SampleType, alpha: bool) -> Pattern {
        let fmt = PixelFormat::new(mode, s, alpha);
        let mut surf = Surface::new(fmt);
        let n = fmt.channels();
        for y in 0..3 {
            for x in 0..5 {
                let px: Vec<f32> = (0..n).map(|c| ((x * 3 + y * 7 + c as i32 * 5) % 11) as f32 / 10.0).collect();
                surf.write_pixel(x, y, &px);
            }
        }
        Pattern::new("P", surf, 5, 3)
    }

    #[test]
    fn patterns_round_trip_through_psd_at_every_depth_and_model() {
        for mode in [ColorMode::Rgb, ColorMode::Grayscale, ColorMode::Cmyk, ColorMode::Lab] {
            for s in [SampleType::U8, SampleType::U16, SampleType::F32] {
                for alpha in [false, true] {
                    let p = pattern(mode, s, alpha);
                    let back = from_psd(&to_psd(&p)).unwrap();
                    assert_eq!((&back.id, &back.name, back.width, back.surface.format()), (&p.id, &p.name, p.width, p.surface.format()));
                    // Exact except float CMYK, whose stored inversion (1 − v) rounds.
                    let (a, b) = (back.surface.read_region(p.rect()), p.surface.read_region(p.rect()));
                    assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6), "{mode:?} {s:?} {alpha}");
                    if !(mode == ColorMode::Cmyk && s == SampleType::F32) {
                        assert_eq!(back, p, "{mode:?} {s:?} {alpha}");
                    }
                }
            }
        }
    }

    #[test]
    fn export_reuses_raw_blocks_until_patterns_change() {
        let mut d = Document::new("x", photocraft_geom::Size { width: 4, height: 4 }, ColorMode::Rgb, SampleType::U8);
        let p = pattern(ColorMode::Rgb, SampleType::U8, false);
        let raw = write_pattern_block(&[to_psd(&p)]).unwrap();
        d.metadata.psd_global_blocks = vec![(*b"8BIM", *b"Txt2", Arc::new(vec![1])), (*b"8BIM", *b"Patt", Arc::new(raw.clone()))];
        d.patterns = from_global_blocks(&d);
        assert_eq!(d.patterns.len(), 1);
        assert_eq!(export_global_blocks(&d), d.metadata.psd_global_blocks);
        d.patterns.push(pattern(ColorMode::Grayscale, SampleType::U16, true));
        let out = export_global_blocks(&d);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].1, *b"Patt");
        let mut d2 = d.clone();
        d2.metadata.psd_global_blocks = out;
        assert_eq!(from_global_blocks(&d2), d.patterns);
        assert_eq!(read_pat(&write_pat(&d.patterns).unwrap()).unwrap(), d.patterns);
    }

    fn gpat(w: u32, h: u32, bpp: u32, name: &str, pixels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [24 + name.len() as u32 + 1, 1, w, h, bpp] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(b"GPAT");
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.extend_from_slice(pixels);
        out
    }

    #[test]
    fn gimp_pat_files_read() {
        let rgb = gpat(2, 1, 3, "Big Blue", &[10, 20, 30, 40, 50, 60]);
        let p = &read_pat(&rgb).unwrap()[0];
        assert_eq!((p.name.as_str(), p.width, p.height), ("Big Blue", 2, 1));
        assert_eq!(p.surface.format(), PixelFormat::new(ColorMode::Rgb, SampleType::U8, false));
        assert_eq!(p.surface.to_interleaved(p.rect()), [10, 20, 30, 40, 50, 60]);
        let ga = &read_pat(&gpat(1, 2, 2, "", &[1, 2, 3, 4])).unwrap()[0];
        assert_eq!(ga.name, "Pattern");
        assert_eq!(ga.surface.format(), PixelFormat::new(ColorMode::Grayscale, SampleType::U8, true));
    }

    #[test]
    fn hostile_gimp_pat_files_error() {
        let ok = gpat(2, 2, 3, "x", &[0; 12]);
        assert!(read_pat(&ok[..ok.len() - 1]).is_err(), "truncated pixels");
        assert!(read_pat(&ok[..22]).is_err(), "truncated header");
        assert!(read_pat(&gpat(0, 2, 3, "x", &[])).is_err());
        assert!(read_pat(&gpat(u32::MAX, u32::MAX, 4, "x", &[])).is_err());
        assert!(read_pat(&gpat(1, 1, 7, "x", &[0; 7])).is_err());
        let mut v2 = ok.clone();
        v2[7] = 2;
        assert!(read_pat(&v2).is_err());
        let mut short_header = ok;
        short_header[3] = 20;
        assert!(read_pat(&short_header).is_err(), "header smaller than its fixed part");
    }
}
