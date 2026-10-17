//! Autodesk Animator FLI/FLC (`.fli`, `.flc`): the first frame of the animation, as an
//! 8-bit colour-mapped picture. Reads the colour-table chunks (COLOR_256, COLOR_64), BYTE_RUN,
//! COPY and BLACK; later frames, which hold only deltas, are not decoded. Read-only.

use crate::error::CodecError;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::Limits;

const F: Format = Format::Fli;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}
fn le16(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 2).map(|s| usize::from(u16::from_le_bytes([s[0], s[1]])))
}
fn le32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

pub(crate) fn is_fli(b: &[u8]) -> bool {
    b.len() >= 128 && matches!(le16(b, 4), Some(0xaf11 | 0xaf12)) && le16(b, 12) == Some(8)
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !is_fli(b) {
        return Err(bad("not an FLI/FLC file"));
    }
    let (w, h) = (le16(b, 8).unwrap_or(0), le16(b, 10).unwrap_or(0));
    if w == 0 || h == 0 {
        return Err(bad("empty image"));
    }
    limits.check(w as u32, h as u32, ChannelLayout::Rgb, SampleType::U8)?;
    let mut pos = 128;
    // The first frame holding picture data (skipping a leading prefix chunk).
    let (size, magic, nchunks) = (le32(b, pos).unwrap_or(0), le16(b, pos + 4).unwrap_or(0), le16(b, pos + 6).unwrap_or(0));
    if magic == 0xf100 {
        pos += size;
    }
    let (fsize, fmagic, nchunks) = if magic == 0xf100 { (le32(b, pos).unwrap_or(0), le16(b, pos + 4).unwrap_or(0), le16(b, pos + 6).unwrap_or(0)) } else { (size, magic, nchunks) };
    if fmagic != 0xf1fa {
        return Err(bad("bad frame header"));
    }
    let end = pos.saturating_add(fsize).min(b.len());
    let mut p = pos + 16;
    let mut palette = [[0u8; 3]; 256];
    let mut pixels = vec![0u8; w * h];
    for _ in 0..nchunks {
        let (csize, ctype) = (le32(b, p).ok_or_else(|| bad("truncated chunk"))?, le16(b, p + 4).ok_or_else(|| bad("truncated chunk"))?);
        if csize < 6 || p + csize > end.max(p + 6) {
            return Err(bad("bad chunk size"));
        }
        let d = &b[p + 6..(p + csize).min(b.len())];
        match ctype {
            4 | 11 => {
                let scale: u8 = if ctype == 11 { 4 } else { 1 };
                let packets = le16(d, 0).unwrap_or(0);
                let (mut at, mut idx) = (2, 0usize);
                for _ in 0..packets {
                    let (skip, count) = (usize::from(*d.get(at).ok_or_else(|| bad("truncated palette"))?), usize::from(*d.get(at + 1).ok_or_else(|| bad("truncated palette"))?));
                    at += 2;
                    idx += skip;
                    let count = if count == 0 { 256 } else { count };
                    for _ in 0..count {
                        let c = d.get(at..at + 3).ok_or_else(|| bad("truncated palette"))?;
                        if idx < 256 {
                            let up = |v: u8| (u16::from(v) * u16::from(scale)).min(255) as u8;
                            palette[idx] = [up(c[0]), up(c[1]), up(c[2])];
                        }
                        idx += 1;
                        at += 3;
                    }
                }
            }
            13 => pixels.fill(0),
            15 => {
                let mut at = 0;
                for y in 0..h {
                    at += 1; // packet count: unreliable, decode by width instead
                    let mut x = 0;
                    while x < w {
                        let c = *d.get(at).ok_or_else(|| bad("truncated BRUN data"))? as i8;
                        at += 1;
                        if c < 0 {
                            let n = (-i32::from(c)) as usize;
                            let src = d.get(at..at + n).ok_or_else(|| bad("truncated BRUN data"))?;
                            let n = n.min(w - x);
                            pixels[y * w + x..y * w + x + n].copy_from_slice(&src[..n]);
                            at += (-i32::from(c)) as usize;
                            x += n;
                        } else if c > 0 {
                            let n = (c as usize).min(w - x);
                            pixels[y * w + x..y * w + x + n].fill(*d.get(at).ok_or_else(|| bad("truncated BRUN data"))?);
                            at += 1;
                            x += n;
                        } else {
                            return Err(bad("zero-length run"));
                        }
                    }
                }
            }
            16 => {
                let src = d.get(..w * h).ok_or_else(|| bad("truncated COPY data"))?;
                pixels.copy_from_slice(src);
            }
            _ => {}
        }
        p += csize;
    }
    let used_gray = pixels.iter().all(|v| palette[usize::from(*v)][0] == palette[usize::from(*v)][1] && palette[usize::from(*v)][1] == palette[usize::from(*v)][2]);
    if used_gray {
        return Image::from_u8(w as u32, h as u32, ChannelLayout::Gray, pixels.iter().map(|v| palette[usize::from(*v)][0]).collect());
    }
    Image::from_u8(w as u32, h as u32, ChannelLayout::Rgb, pixels.iter().flat_map(|v| palette[usize::from(*v)]).collect())
}
