//! Sony PlayStation TIM textures (`.tim`): magic `10 00 00 00`, a flags word (pixel mode in the
//! low two bits: 0 = 4-bit, 1 = 8-bit, 2 = 16-bit, 3 = 24-bit; bit 3 = has a colour table), the
//! optional CLUT block, then the image block. Colours are 15-bit (`R` in the low five bits).
//! Reads the first palette of an indexed image. Read-only.

use crate::error::CodecError;
use crate::format::Format;
use crate::image::{ChannelLayout, Image};
use crate::options::Limits;

const F: Format = Format::Tim;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}
fn le16(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 2).map(|s| usize::from(u16::from_le_bytes([s[0], s[1]])))
}
fn le32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

pub(crate) fn is_tim(b: &[u8]) -> bool {
    b.len() >= 12 && b[..4] == [0x10, 0, 0, 0] && b[4] & 0xf4 == 0 && b[5..8] == [0, 0, 0] && (b[4] & 3 != 2 || b[4] & 8 == 0) && (b[4] & 3 >= 2 || b[4] & 8 != 0)
}

/// 15-bit PlayStation colour to 8-bit RGB.
fn colour(v: usize) -> [u8; 3] {
    let c = |x: usize| ((x & 31) * 255 / 31) as u8;
    [c(v), c(v >> 5), c(v >> 10)]
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !is_tim(b) {
        return Err(bad("not a TIM file"));
    }
    let (mode, has_clut) = (b[4] & 3, b[4] & 8 != 0);
    let mut pos = 8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    if has_clut {
        let size = le32(b, pos).ok_or_else(|| bad("truncated CLUT"))?;
        let (colors, cluts) = (le16(b, pos + 8).ok_or_else(|| bad("truncated CLUT"))?, le16(b, pos + 10).ok_or_else(|| bad("truncated CLUT"))?);
        if size < 12 || colors == 0 || cluts == 0 || colors > 256 {
            return Err(bad("bad CLUT"));
        }
        let data = b.get(pos + 12..pos + size).ok_or_else(|| bad("truncated CLUT"))?;
        // The first table only.
        palette = (0..colors).map(|i| le16(data, i * 2).map(colour).ok_or_else(|| bad("truncated CLUT"))).collect::<Result<_, _>>()?;
        pos += size;
    }
    let size = le32(b, pos).ok_or_else(|| bad("truncated image block"))?;
    let (words, h) = (le16(b, pos + 8).ok_or_else(|| bad("truncated image block"))?, le16(b, pos + 10).ok_or_else(|| bad("truncated image block"))?);
    let data = b.get(pos + 12..pos.saturating_add(size).min(b.len())).ok_or_else(|| bad("truncated image block"))?;
    let w = match mode {
        0 => words * 4,
        1 => words * 2,
        2 => words,
        _ => words * 2 / 3,
    };
    if w == 0 || h == 0 {
        return Err(bad("empty image"));
    }
    limits.check(w as u32, h as u32, ChannelLayout::Rgb, crate::SampleType::U8)?;
    let row = words * 2;
    if data.len() < row * h {
        return Err(bad("truncated pixel data"));
    }
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let r = &data[y * row..(y + 1) * row];
        for x in 0..w {
            let rgb = match mode {
                0 => palette.get(usize::from(r[x / 2] >> (4 * (x % 2)) & 15)).copied().unwrap_or([0; 3]),
                1 => palette.get(usize::from(r[x])).copied().unwrap_or([0; 3]),
                2 => colour(usize::from(u16::from_le_bytes([r[x * 2], r[x * 2 + 1]]))),
                _ => [r[x * 3], r[x * 3 + 1], r[x * 3 + 2]],
            };
            out.extend(rgb);
        }
    }
    Image::from_u8(w as u32, h as u32, ChannelLayout::Rgb, out)
}
