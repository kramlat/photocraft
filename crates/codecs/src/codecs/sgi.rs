//! Silicon Graphics image (`.sgi`, `.rgb`, `.rgba`, `.bw`): a 512-byte big-endian header (magic
//! 474, storage 0 = raw / 1 = RLE, bytes per channel 1 or 2, dimensions, size and channel count),
//! then planar channels stored bottom row first. RLE files carry a table of per-row offsets and
//! lengths; each row is a run of packets (high bit set: copy `n` literals, clear: repeat one
//! value `n` times, 0 ends the row).
//!
//! Reads raw and RLE at 8 and 16 bits with 1–4 channels; writes RLE.

use super::legacy::{be16, be32};
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Sgi;
const HEADER: usize = 512;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

pub(crate) fn looks_like_sgi(b: &[u8]) -> bool {
    b.len() >= HEADER && b[0] == 0x01 && b[1] == 0xda && b[2] <= 1 && matches!(b[3], 1 | 2)
}

/// Decode one RLE row into `out` (sample values, `out.len()` of them).
fn unrle_row(data: &[u8], wide: bool, out: &mut [u16]) -> Result<(), CodecError> {
    let unit = if wide { 2 } else { 1 };
    let read = |i: usize| -> Result<u16, CodecError> {
        let s = data.get(i * unit..(i + 1) * unit).ok_or_else(|| bad("truncated RLE row"))?;
        Ok(if wide { u16::from_be_bytes([s[0], s[1]]) } else { u16::from(s[0]) })
    };
    let (mut i, mut n) = (0usize, 0usize);
    while n < out.len() {
        let c = read(i)?;
        i += 1;
        let count = usize::from(c & 0x7f);
        if count == 0 {
            return Err(bad("RLE row ends early"));
        }
        if count > out.len() - n {
            return Err(bad("RLE packet overruns the row"));
        }
        if c & 0x80 != 0 {
            for k in 0..count {
                out[n + k] = read(i + k)?;
            }
            i += count;
        } else {
            let v = read(i)?;
            i += 1;
            out[n..n + count].fill(v);
        }
        n += count;
    }
    Ok(())
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !looks_like_sgi(b) {
        return Err(bad("invalid SGI header"));
    }
    let (rle, bpc, dim) = (b[2] == 1, usize::from(b[3]), be16(b, 4).unwrap_or(0));
    let (w, mut h, mut z) = (u32::from(be16(b, 6).unwrap_or(0)), u32::from(be16(b, 8).unwrap_or(0)), usize::from(be16(b, 10).unwrap_or(0)));
    if dim == 1 {
        h = 1;
    }
    if dim < 3 {
        z = 1;
    }
    let layout = match z {
        1 => ChannelLayout::Gray,
        2 => ChannelLayout::GrayA,
        3 => ChannelLayout::Rgb,
        4 => ChannelLayout::Rgba,
        _ => return Err(bad(format!("unsupported channel count {z}"))),
    };
    let sample = if bpc == 1 { SampleType::U8 } else { SampleType::U16 };
    limits.check(w, h, layout, sample)?;
    let (wu, hu) = (w as usize, h as usize);
    let mut planes = vec![0u16; wu * hu * z];
    let wide = bpc == 2;
    if rle {
        let rows = hu * z;
        let table_end = HEADER + rows * 8;
        if b.len() < table_end {
            return Err(bad("truncated offset tables"));
        }
        for r in 0..rows {
            let off = be32(b, HEADER + r * 4).unwrap_or(0) as usize;
            let len = be32(b, HEADER + rows * 4 + r * 4).unwrap_or(0) as usize;
            let data = b.get(off..off.saturating_add(len).min(b.len())).filter(|_| off >= table_end).ok_or_else(|| bad("row outside the file"))?;
            unrle_row(data, wide, &mut planes[r * wu..(r + 1) * wu])?;
        }
    } else {
        let need = wu * hu * z * bpc;
        let body = b.get(HEADER..HEADER + need).ok_or_else(|| bad("truncated pixel data"))?;
        for (i, v) in planes.iter_mut().enumerate() {
            *v = if wide { u16::from_be_bytes([body[i * 2], body[i * 2 + 1]]) } else { u16::from(body[i]) };
        }
    }
    // Planar, bottom row first → interleaved, top row first. Plane index = channel * h + row.
    let mut inter = vec![0u16; wu * hu * z];
    for c in 0..z {
        for y in 0..hu {
            let src = &planes[(c * hu + (hu - 1 - y)) * wu..][..wu];
            for (x, v) in src.iter().enumerate() {
                inter[(y * wu + x) * z + c] = *v;
            }
        }
    }
    if wide { Image::from_u16(w, h, layout, &inter) } else { Image::from_u8(w, h, layout, inter.iter().map(|v| *v as u8).collect()) }
}

fn rle_row(row: &[u16], wide: bool, out: &mut Vec<u8>) {
    let put = |v: u16, out: &mut Vec<u8>| {
        if wide {
            out.extend_from_slice(&v.to_be_bytes());
        } else {
            out.push(v as u8);
        }
    };
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && row[i + run] == row[i] && run < 127 {
            run += 1;
        }
        if run >= 3 {
            put(run as u16, out);
            put(row[i], out);
            i += run;
        } else {
            // Literal stretch until the next run of three or 127 samples.
            let start = i;
            while i < row.len() && i - start < 127 && !(i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2]) {
                i += 1;
            }
            put(0x80 | (i - start) as u16, out);
            row[start..i].iter().for_each(|v| put(*v, out));
        }
    }
    put(0, out);
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h, z) = (img.width() as usize, img.height() as usize, img.layout().channels());
    let wide = img.sample_type() == SampleType::U16;
    if w > 65535 || h > 65535 {
        return Err(CodecError::encode(F, "SGI is limited to 65535 pixels per side"));
    }
    let samples: Vec<u16> = if wide { img.to_u16_samples().unwrap_or_default() } else { img.data().iter().map(|v| u16::from(*v)).collect() };
    let mut rows: Vec<Vec<u8>> = Vec::with_capacity(h * z);
    for c in 0..z {
        for y in 0..h {
            let sy = h - 1 - y;
            let row: Vec<u16> = (0..w).map(|x| samples[(sy * w + x) * z + c]).collect();
            let mut data = Vec::new();
            rle_row(&row, wide, &mut data);
            rows.push(data);
        }
    }
    let mut out = vec![0u8; HEADER];
    out[0..2].copy_from_slice(&[0x01, 0xda]);
    out[2] = 1;
    out[3] = if wide { 2 } else { 1 };
    out[4..6].copy_from_slice(&(if z > 1 { 3u16 } else { 2 }).to_be_bytes());
    out[6..8].copy_from_slice(&(w as u16).to_be_bytes());
    out[8..10].copy_from_slice(&(h as u16).to_be_bytes());
    out[10..12].copy_from_slice(&(z as u16).to_be_bytes());
    out[12..16].copy_from_slice(&0u32.to_be_bytes());
    out[16..20].copy_from_slice(&(if wide { 65535u32 } else { 255 }).to_be_bytes());
    out[24..24 + 5].copy_from_slice(b"image");
    let mut off = (HEADER + rows.len() * 8) as u32;
    let (mut starts, mut lens) = (Vec::new(), Vec::new());
    for r in &rows {
        starts.extend_from_slice(&off.to_be_bytes());
        lens.extend_from_slice(&(r.len() as u32).to_be_bytes());
        off += r.len() as u32;
    }
    out.extend(starts);
    out.extend(lens);
    rows.iter().for_each(|r| out.extend_from_slice(r));
    Ok(out)
}
