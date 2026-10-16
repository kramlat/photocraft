//! Sun Raster (`.ras`): a 32-byte big-endian header (magic 0x59A66A95, width, height, depth,
//! data length, type, colour-map type, colour-map length), an optional colour map (all reds, then
//! all greens, then all blues), then rows padded to 16 bits.
//!
//! Reads depths 1, 8, 24 and 32 for the old, standard, byte-encoded (RLE) and RGB types. Writes
//! uncompressed standard files: 8-bit gray with a ramp colour map, 24-bit BGR, 32-bit ABGR.

use super::legacy::{be32, bit_at};
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::SunRaster;
pub(crate) const MAGIC: [u8; 4] = [0x59, 0xa6, 0x6a, 0x95];

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

/// Sun's byte-encoded run-length scheme: `0x80 0x00` is a literal 0x80, `0x80 n v` is `n + 1`
/// copies of `v`, anything else is itself.
fn unrle(data: &[u8], want: usize) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0;
    while out.len() < want {
        let c = *data.get(i).ok_or_else(|| bad("truncated RLE data"))?;
        i += 1;
        if c != 0x80 {
            out.push(c);
            continue;
        }
        let n = *data.get(i).ok_or_else(|| bad("truncated RLE data"))?;
        i += 1;
        if n == 0 {
            out.push(0x80);
        } else {
            let v = *data.get(i).ok_or_else(|| bad("truncated RLE data"))?;
            i += 1;
            let count = usize::from(n) + 1;
            if out.len() + count > want {
                return Err(bad("RLE run overruns the image"));
            }
            out.resize(out.len() + count, v);
        }
    }
    Ok(out)
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !b.starts_with(&MAGIC) {
        return Err(bad("missing Sun Raster signature"));
    }
    let field = |i: usize| be32(b, 4 + i * 4).ok_or_else(|| bad("truncated header"));
    let (w, h, depth, length, kind, map_kind, map_len) = (field(0)?, field(1)?, field(2)?, field(3)?, field(4)?, field(5)?, field(6)? as usize);
    if !matches!(depth, 1 | 8 | 24 | 32) {
        return Err(bad(format!("unsupported depth {depth}")));
    }
    if !matches!(kind, 0..=3) {
        return Err(bad(format!("unsupported type {kind}")));
    }
    if !matches!(map_kind, 0..=2) {
        return Err(bad("unsupported colour map type"));
    }
    limits.check(w, h, ChannelLayout::Rgba, SampleType::U8)?;
    let map_end = 32usize.checked_add(map_len).ok_or_else(|| bad("bad colour map length"))?;
    let map = b.get(32..map_end).ok_or_else(|| bad("truncated colour map"))?;
    let (wu, hu) = (w as usize, h as usize);
    let row = match depth {
        1 => wu.div_ceil(16) * 2,
        8 => wu.next_multiple_of(2),
        d => (wu * (d as usize / 8)).next_multiple_of(2),
    };
    let want = row.checked_mul(hu).ok_or_else(|| bad("image size overflow"))?;
    let body = b.get(map_end..).unwrap_or_default();
    let body = if length != 0 && kind != 2 { body.get(..(length as usize).min(body.len())).unwrap_or(body) } else { body };
    let owned;
    let rows: &[u8] = if kind == 2 {
        owned = unrle(body, want)?;
        &owned
    } else {
        body
    };
    if rows.len() < want {
        return Err(bad("truncated pixel data"));
    }

    // Colour map entries (R, G, B planes); an 8-bit image without one is plain gray.
    let n = map_len / 3;
    let entry = |i: usize| -> [u8; 3] {
        if map_kind == 0 || n == 0 {
            [i as u8; 3]
        } else if i < n {
            [map[i], map[n + i], map[2 * n + i]]
        } else {
            [0, 0, 0]
        }
    };
    if depth <= 8 {
        let palette: Vec<[u8; 3]> = if depth == 1 {
            if n >= 2 { vec![entry(0), entry(1)] } else { vec![[255; 3], [0; 3]] }
        } else {
            (0..256).map(entry).collect()
        };
        let gray = palette.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
        let mut out = Vec::with_capacity(wu * hu * if gray { 1 } else { 3 });
        for y in 0..hu {
            for x in 0..wu {
                let idx = if depth == 1 { usize::from(bit_at(rows, row, x, y, true)) } else { usize::from(rows[y * row + x]) };
                let p = palette[idx];
                if gray {
                    out.push(p[0]);
                } else {
                    out.extend_from_slice(&p);
                }
            }
        }
        return Image::from_u8(w, h, if gray { ChannelLayout::Gray } else { ChannelLayout::Rgb }, out);
    }
    let bytes = depth as usize / 8;
    let rgb_order = kind == 3;
    // A 32-bit file's first byte is alpha in some writers and padding (0) in others.
    let alpha_used = depth == 32 && (0..hu).any(|y| (0..wu).any(|x| rows[y * row + x * 4] != 0));
    let layout = if alpha_used { ChannelLayout::Rgba } else { ChannelLayout::Rgb };
    let mut out = Vec::with_capacity(wu * hu * layout.channels());
    for y in 0..hu {
        for x in 0..wu {
            let p = &rows[y * row + x * bytes..y * row + (x + 1) * bytes];
            let c = if bytes == 4 { &p[1..] } else { p };
            if rgb_order {
                out.extend_from_slice(&c[..3]);
            } else {
                out.extend_from_slice(&[c[2], c[1], c[0]]);
            }
            if alpha_used {
                out.push(p[0]);
            }
        }
    }
    Image::from_u8(w, h, layout, out)
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let ch = img.layout().channels();
    let (depth, ramp) = match img.layout() {
        ChannelLayout::Gray => (8u32, true),
        ChannelLayout::Rgb => (24, false),
        _ => (32, false),
    };
    let row = (w * depth as usize / 8).next_multiple_of(2);
    let mut data = vec![0u8; row * h];
    for y in 0..h {
        for x in 0..w {
            let p = &img.data()[(y * w + x) * ch..(y * w + x + 1) * ch];
            let o = y * row + x * depth as usize / 8;
            match ch {
                1 => data[o] = p[0],
                3 => data[o..o + 3].copy_from_slice(&[p[2], p[1], p[0]]),
                _ => data[o..o + 4].copy_from_slice(&[p[3], p[2], p[1], p[0]]),
            }
        }
    }
    let map_len = if ramp { 768u32 } else { 0 };
    let mut out = Vec::with_capacity(32 + map_len as usize + data.len());
    out.extend_from_slice(&MAGIC);
    for v in [w as u32, h as u32, depth, data.len() as u32, 1, u32::from(ramp), map_len] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    if ramp {
        for _ in 0..3 {
            out.extend(0..=255u8);
        }
    }
    out.extend_from_slice(&data);
    Ok(out)
}
