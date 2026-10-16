//! Apple icon images (`.icns`): `icns`, a big-endian total length, then `type, length, data`
//! elements. Reads the largest picture: PNG and JPEG 2000 elements (`icp4`–`ic14`, 16 px up to
//! 1024 px) and the classic RLE-compressed 24-bit elements (`is32`, `il32`, `ih32`, `it32`)
//! with their 8-bit masks. Read-only: an icon file holds a few fixed square sizes, which a plain
//! Save As (any size) cannot honour; see `ASYMMETRIC_EXCEPTIONS`.

use super::legacy::be32;
use crate::error::CodecError;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::Limits;

const F: Format = Format::Icns;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

pub(crate) fn is_icns(b: &[u8]) -> bool {
    b.starts_with(b"icns")
}

/// Pixel side of an icon element type, and whether it holds PNG/JPEG 2000 (`true`) or classic RLE.
fn kind(ty: &[u8]) -> Option<(usize, bool)> {
    Some(match ty {
        b"icp4" => (16, true),
        b"icp5" => (32, true),
        b"icp6" => (64, true),
        b"ic07" => (128, true),
        b"ic08" => (256, true),
        b"ic09" => (512, true),
        b"ic10" => (1024, true),
        b"ic11" => (32, true),
        b"ic12" => (64, true),
        b"ic13" => (256, true),
        b"ic14" => (512, true),
        b"is32" => (16, false),
        b"il32" => (32, false),
        b"ih32" => (48, false),
        b"it32" => (128, false),
        _ => return None,
    })
}

/// One channel of the classic ICNS run-length coding: `side * side` values.
fn unrle(data: &[u8], pos: &mut usize, count: usize) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let c = *data.get(*pos).ok_or_else(|| bad("truncated RLE data"))?;
        *pos += 1;
        if c < 128 {
            let n = usize::from(c) + 1;
            out.extend_from_slice(data.get(*pos..*pos + n).ok_or_else(|| bad("truncated RLE data"))?);
            *pos += n;
        } else {
            let n = usize::from(c) - 125;
            out.resize(out.len() + n, *data.get(*pos).ok_or_else(|| bad("truncated RLE data"))?);
            *pos += 1;
        }
    }
    out.truncate(count);
    Ok(out)
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !is_icns(b) {
        return Err(bad("missing icns signature"));
    }
    let total = (be32(b, 4).unwrap_or(0) as usize).min(b.len());
    let mut elements: Vec<(&[u8], &[u8])> = Vec::new();
    let mut pos = 8;
    while pos + 8 <= total {
        let len = be32(b, pos + 4).unwrap_or(0) as usize;
        if len < 8 || pos + len > total {
            break;
        }
        elements.push((&b[pos..pos + 4], &b[pos + 8..pos + len]));
        pos += len;
    }
    let best = elements.iter().filter_map(|(t, d)| kind(t).map(|(side, modern)| (side, modern, *t, *d))).max_by_key(|(side, modern, ..)| (*side, *modern));
    let (side, modern, ty, data) = best.ok_or_else(|| bad("no icon images"))?;
    if modern {
        return if data.starts_with(b"\x89PNG") { super::png::decode(data, limits) } else { super::jp2::decode(data, limits) };
    }
    let side32 = u32::try_from(side).map_err(|_| bad("bad icon size"))?;
    limits.check(side32, side32, ChannelLayout::Rgba, SampleType::U8)?;
    let n = side * side;
    let mut p = if ty == b"it32" { 4 } else { 0 };
    let planes: Vec<Vec<u8>> = (0..3).map(|_| unrle(data, &mut p, n)).collect::<Result<_, _>>()?;
    let mask_type: &[u8; 4] = match ty {
        b"is32" => b"s8mk",
        b"il32" => b"l8mk",
        b"ih32" => b"h8mk",
        _ => b"t8mk",
    };
    let mask = elements.iter().find(|(t, d)| *t == mask_type && d.len() >= n).map(|(_, d)| *d);
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        out.extend([planes[0][i], planes[1][i], planes[2][i], mask.map_or(255, |m| m[i])]);
    }
    Image::from_u8(side32, side32, ChannelLayout::Rgba, out)
}
