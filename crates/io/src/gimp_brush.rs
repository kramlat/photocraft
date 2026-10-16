//! GIMP brush files: `.gbr` (one brush tip) and `.gih` (an image hose: a text header, then
//! several `.gbr` tips back to back).
//!
//! A `.gbr` is a big-endian header (header size, version 1 or 2, width, height, bytes per
//! pixel) followed, in version 2, by the magic `GIMP` and a spacing in percent, then a
//! NUL-terminated name up to the header size, then 8-bit interleaved pixels. One byte per pixel
//! is a paint mask (255 = full paint); four bytes are RGBA, of which only the alpha shapes the
//! tip (the tip is a mask; colour brushes paint in the foreground colour).

/// Largest side accepted for one tip (GIMP's own limit is 10000).
const MAX_SIDE: u32 = 4096;
/// Most cells read from one `.gih`.
const MAX_CELLS: usize = 1024;

/// One brush tip as a paint-amount map.
#[derive(Debug, Clone, PartialEq)]
pub struct GimpTip {
    pub name: String,
    /// Spacing as a fraction of the tip size (GIMP stores a percent).
    pub spacing: f32,
    pub width: usize,
    pub height: usize,
    /// Row-major paint amounts, 0 = none, 1 = full.
    pub paint: Vec<f32>,
}

/// A parsed brush file: a `.gbr` has one tip, a `.gih` has one per cell.
#[derive(Debug, Clone, PartialEq)]
pub struct GimpBrushFile {
    pub name: String,
    pub tips: Vec<GimpTip>,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?).and_then(|s| <[u8; 4]>::try_from(s).ok()).map(u32::from_be_bytes)
}

/// Parses the `.gbr` at the start of `b`; returns the tip and the bytes it occupied.
fn parse_gbr_at(b: &[u8]) -> Result<(GimpTip, usize), String> {
    let bad = |what: &str| format!("not a readable GIMP brush: {what}");
    let (Some(header), Some(version), Some(w), Some(h), Some(bpp)) = (be32(b, 0), be32(b, 4), be32(b, 8), be32(b, 12), be32(b, 16)) else {
        return Err(bad("truncated header"));
    };
    let header = header as usize;
    if !(1..=2).contains(&version) {
        return Err(bad(&format!("unsupported version {version}")));
    }
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(bad("bad size"));
    }
    if bpp != 1 && bpp != 4 {
        return Err(bad("unsupported bytes per pixel"));
    }
    let fixed = if version == 1 { 20 } else { 28 };
    if header < fixed {
        return Err(bad("header smaller than its fixed part"));
    }
    let mut spacing = 0.25;
    if version == 2 {
        if b.get(20..24) != Some(b"GIMP") {
            return Err(bad("missing GIMP magic"));
        }
        spacing = be32(b, 24).ok_or_else(|| bad("truncated header"))?.clamp(1, 1000) as f32 / 100.0;
    }
    let name = b.get(fixed..header).ok_or_else(|| bad("truncated name"))?;
    let name = String::from_utf8_lossy(name.split(|c| *c == 0).next().unwrap_or_default()).trim().to_string();
    let len = (w as usize) * (h as usize) * (bpp as usize);
    let end = header.checked_add(len).ok_or_else(|| bad("bad size"))?;
    let px = b.get(header..end).ok_or_else(|| bad("truncated pixels"))?;
    let paint = if bpp == 1 { px.iter().map(|v| f32::from(*v) / 255.0).collect() } else { px.chunks_exact(4).map(|c| f32::from(c[3]) / 255.0).collect() };
    Ok((GimpTip { name, spacing, width: w as usize, height: h as usize, paint }, end))
}

/// Reads a `.gbr`.
pub fn read_gbr(bytes: &[u8]) -> Result<GimpBrushFile, String> {
    let (tip, _) = parse_gbr_at(bytes)?;
    Ok(GimpBrushFile { name: tip.name.clone(), tips: vec![tip] })
}

/// Reads a `.gih`: the brush name, a line with the cell count (and, in newer files, the hose's
/// parameters after it), then that many `.gbr` tips.
pub fn read_gih(bytes: &[u8]) -> Result<GimpBrushFile, String> {
    let bad = |what: &str| format!("not a readable GIMP image hose: {what}");
    let mut pos = 0;
    let mut line = || -> Option<String> {
        let rest = bytes.get(pos..)?;
        let n = rest.iter().position(|c| *c == b'\n')?;
        pos += n + 1;
        Some(String::from_utf8_lossy(rest.get(..n)?).trim().to_string())
    };
    let name = line().ok_or_else(|| bad("missing name"))?;
    let params = line().ok_or_else(|| bad("missing cell count"))?;
    let cells: usize = params.split_whitespace().next().and_then(|t| t.parse().ok()).ok_or_else(|| bad("bad cell count"))?;
    if cells == 0 || cells > MAX_CELLS {
        return Err(bad("bad cell count"));
    }
    let mut tips = Vec::with_capacity(cells);
    for _ in 0..cells {
        let (tip, used) = parse_gbr_at(bytes.get(pos..).unwrap_or_default())?;
        pos += used;
        tips.push(tip);
    }
    Ok(GimpBrushFile { name, tips })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gbr(version: u32, w: u32, h: u32, bpp: u32, name: &str, spacing: u32, px: &[u8]) -> Vec<u8> {
        let fixed = if version == 1 { 20 } else { 28 };
        let mut out = Vec::new();
        for v in [fixed + name.len() as u32 + 1, version, w, h, bpp] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        if version == 2 {
            out.extend_from_slice(b"GIMP");
            out.extend_from_slice(&spacing.to_be_bytes());
        }
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.extend_from_slice(px);
        out
    }

    #[test]
    fn gbr_reads_masks_and_color_alpha() {
        let f = read_gbr(&gbr(2, 2, 1, 1, "Round", 10, &[255, 0])).unwrap();
        assert_eq!(f.name, "Round");
        assert_eq!((f.tips[0].width, f.tips[0].height, f.tips[0].spacing), (2, 1, 0.1));
        assert_eq!(f.tips[0].paint, [1.0, 0.0]);
        let c = read_gbr(&gbr(2, 1, 1, 4, "Rgba", 25, &[9, 9, 9, 255])).unwrap();
        assert_eq!(c.tips[0].paint, [1.0]);
        let v1 = read_gbr(&gbr(1, 1, 1, 1, "Old", 0, &[51])).unwrap();
        assert_eq!((v1.tips[0].spacing, v1.tips[0].paint[0]), (0.25, 0.2));
    }

    #[test]
    fn gih_reads_every_cell_with_or_without_parameters() {
        for params in ["2", "2 ncells:2 dim:1 placement:default"] {
            let mut b = format!("Hose\n{params}\n").into_bytes();
            b.extend(gbr(2, 1, 1, 1, "a", 25, &[255]));
            b.extend(gbr(2, 1, 1, 1, "b", 25, &[128]));
            let f = read_gih(&b).unwrap();
            assert_eq!((f.name.as_str(), f.tips.len()), ("Hose", 2));
            assert_eq!(f.tips[1].name, "b");
        }
    }

    #[test]
    fn hostile_brush_files_error() {
        let ok = gbr(2, 2, 2, 1, "x", 25, &[1; 4]);
        assert!(read_gbr(&ok[..ok.len() - 1]).is_err(), "truncated pixels");
        assert!(read_gbr(&ok[..19]).is_err(), "truncated header");
        assert!(read_gbr(&gbr(2, u32::MAX, u32::MAX, 4, "x", 25, &[])).is_err());
        assert!(read_gbr(&gbr(2, 1, 1, 2, "x", 25, &[0; 2])).is_err());
        assert!(read_gbr(&gbr(3, 1, 1, 1, "x", 25, &[0])).is_err());
        let mut small = ok.clone();
        small[3] = 10;
        assert!(read_gbr(&small).is_err(), "header smaller than its fixed part");
        let mut nomagic = ok;
        nomagic[20] = b'X';
        assert!(read_gbr(&nomagic).is_err());
        assert!(read_gih(b"Hose\n0\n").is_err());
        assert!(read_gih(b"Hose\n99999\n").is_err());
        assert!(read_gih(b"Hose\n3\n").is_err(), "cells missing");
        assert!(read_gih(b"no newline").is_err());
    }
}
