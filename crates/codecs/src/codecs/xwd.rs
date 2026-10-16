#![allow(clippy::chunks_exact_to_as_chunks)]
//! X Window Dump (`.xwd`, version 7): a header of 25 big-endian u32 fields, the window name, the
//! colour map (12-byte `XColor` entries), then the pixels in the dump's own byte and bit order.
//!
//! Reads 1-bit bitmaps and 8-bit colour-mapped or gray pixmaps, and 16/24/32-bit direct-colour
//! pixmaps through their channel masks. Writes 32-bit TrueColor with the usual 0x00RRGGBB masks.

use super::legacy::{be32, bit_at};
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Xwd;
const FIXED: usize = 100;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

pub(crate) fn looks_like_xwd(b: &[u8]) -> bool {
    let (Some(size), Some(ver), Some(fmt), Some(depth), Some(w), Some(h), Some(bpp)) = (be32(b, 0), be32(b, 4), be32(b, 8), be32(b, 12), be32(b, 16), be32(b, 20), be32(b, 44))
    else {
        return false;
    };
    ver == 7 && (FIXED as u32..=65536).contains(&size) && fmt <= 2 && (1..=32).contains(&depth) && w > 0 && h > 0 && matches!(bpp, 1 | 2 | 4 | 8 | 16 | 24 | 32)
}

/// Value of the `mask` field in `pixel`, scaled to 0..=255.
fn channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let v = (pixel & mask) >> shift;
    let max = (1u64 << bits) - 1;
    ((u64::from(v) * 255 + max / 2) / max) as u8
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !looks_like_xwd(b) {
        return Err(bad("invalid XWD header"));
    }
    let f = |i: usize| be32(b, i * 4).ok_or_else(|| bad("truncated header"));
    let (header_size, format, depth, w, h) = (f(0)? as usize, f(2)?, f(3)?, f(4)?, f(5)?);
    let (msb_bytes, bit_order, bpp, bpl, class) = (f(7)? == 1, f(9)?, f(11)? as usize, f(12)? as usize, f(13)?);
    let (rmask, gmask, bmask, ncolors) = (f(14)?, f(15)?, f(16)?, f(19)? as usize);
    limits.check(w, h, ChannelLayout::Rgb, SampleType::U8)?;
    let (wu, hu) = (w as usize, h as usize);
    if bpl < (wu * bpp).div_ceil(8) {
        return Err(bad("bytes per line too small"));
    }
    let colours_at = header_size;
    let pixels_at = ncolors.checked_mul(12).and_then(|n| n.checked_add(colours_at)).ok_or_else(|| bad("bad colour count"))?;
    let map = b.get(colours_at..pixels_at).ok_or_else(|| bad("truncated colour map"))?;
    let need = bpl.checked_mul(hu).ok_or_else(|| bad("image size overflow"))?;
    let rows = b.get(pixels_at..pixels_at.saturating_add(need)).ok_or_else(|| bad("truncated pixel data"))?;
    let lookup = |pixel: u32| -> [u8; 3] {
        for e in map.chunks_exact(12) {
            if u32::from_be_bytes([e[0], e[1], e[2], e[3]]) == pixel {
                return [e[4], e[6], e[8]];
            }
        }
        [0, 0, 0]
    };
    let mut out = Vec::with_capacity(wu * hu * 3);
    let mut all_gray = true;
    for y in 0..hu {
        let row = &rows[y * bpl..(y + 1) * bpl];
        for x in 0..wu {
            let rgb = match bpp {
                1 => {
                    let on = bit_at(rows, bpl, x, y, bit_order == 1);
                    if ncolors >= 2 {
                        lookup(u32::from(on))
                    } else if on {
                        [255; 3]
                    } else {
                        [0; 3]
                    }
                }
                2 | 4 | 8 => {
                    let idx = if bpp == 8 {
                        u32::from(row[x])
                    } else {
                        let per = 8 / bpp;
                        let slot = if msb_bytes { per - 1 - x % per } else { x % per };
                        u32::from(row[x / per] >> (slot * bpp)) & ((1 << bpp) - 1)
                    };
                    if ncolors > 0 {
                        lookup(idx)
                    } else {
                        [(idx * 255 / ((1 << bpp) - 1)) as u8; 3]
                    }
                }
                _ => {
                    let n = bpp / 8;
                    let px = &row[x * n..(x + 1) * n];
                    let pixel = if msb_bytes { px.iter().fold(0u32, |a, v| a << 8 | u32::from(*v)) } else { px.iter().rev().fold(0u32, |a, v| a << 8 | u32::from(*v)) };
                    if class >= 4 && rmask | gmask | bmask != 0 {
                        [channel(pixel, rmask), channel(pixel, gmask), channel(pixel, bmask)]
                    } else if ncolors > 0 {
                        lookup(pixel)
                    } else {
                        [(pixel & 0xff) as u8; 3]
                    }
                }
            };
            all_gray &= rgb[0] == rgb[1] && rgb[1] == rgb[2];
            out.extend_from_slice(&rgb);
        }
    }
    let _ = (format, depth);
    if all_gray {
        let gray: Vec<u8> = out.chunks_exact(3).map(|p| p[0]).collect();
        return Image::from_u8(w, h, ChannelLayout::Gray, gray);
    }
    Image::from_u8(w, h, ChannelLayout::Rgb, out)
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width(), img.height());
    let name = b"photocraft\0";
    let header_size = (FIXED + name.len()) as u32;
    let fields: [u32; 25] = [header_size, 7, 2, 24, w, h, 0, 1, 32, 1, 32, 32, w * 4, 4, 0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 8, 256, 0, w, h, 0, 0, 0];
    let mut out = Vec::with_capacity(header_size as usize + (w * h * 4) as usize);
    for v in fields {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(name);
    for p in img.data().chunks_exact(3) {
        out.extend_from_slice(&[0, p[0], p[1], p[2]]);
    }
    Ok(out)
}
