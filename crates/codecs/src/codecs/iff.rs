//! Amiga IFF pictures: `FORM ILBM` (planar bitmaps) and `FORM PBM ` (chunky, from DeluxePaint on
//! the PC).
//!
//! Reads 1–8 plane colour-mapped images (including EHB and HAM6/HAM8), 24-bit and 32-bit "deep"
//! images, a mask plane or transparent colour as alpha, and ByteRun1 or uncompressed bodies.
//! Writes 24-bit deep ILBM, or 8-plane gray with a gray colour map, ByteRun1-compressed.

use super::legacy::{be16, be32};
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Iff;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

pub(crate) fn is_iff(b: &[u8]) -> bool {
    b.len() >= 12 && &b[0..4] == b"FORM" && matches!(&b[8..12], b"ILBM" | b"PBM ")
}

fn unpack(data: &[u8], pos: &mut usize, out: &mut [u8]) -> Result<(), CodecError> {
    let mut n = 0;
    while n < out.len() {
        let c = *data.get(*pos).ok_or_else(|| bad("truncated ByteRun1 data"))? as i8;
        *pos += 1;
        if c >= 0 {
            let count = c as usize + 1;
            if count > out.len() - n {
                return Err(bad("ByteRun1 run crosses a row"));
            }
            out[n..n + count].copy_from_slice(data.get(*pos..*pos + count).ok_or_else(|| bad("truncated ByteRun1 data"))?);
            *pos += count;
            n += count;
        } else if c != -128 {
            let count = 1 - c as isize;
            let count = count as usize;
            if count > out.len() - n {
                return Err(bad("ByteRun1 run crosses a row"));
            }
            out[n..n + count].fill(*data.get(*pos).ok_or_else(|| bad("truncated ByteRun1 data"))?);
            *pos += 1;
            n += count;
        }
    }
    Ok(())
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !is_iff(b) {
        return Err(bad("not an IFF ILBM or PBM file"));
    }
    let chunky = &b[8..12] == b"PBM ";
    let end = (be32(b, 4).unwrap_or(0) as usize + 8).min(b.len());
    let mut bmhd: Option<&[u8]> = None;
    let mut body: Option<&[u8]> = None;
    let (mut cmap, mut camg): (&[u8], u32) = (&[], 0);
    let mut pos = 12;
    while pos + 8 <= end {
        let len = be32(b, pos + 4).unwrap_or(0) as usize;
        let data = b.get(pos + 8..(pos + 8).saturating_add(len).min(end)).unwrap_or_default();
        match &b[pos..pos + 4] {
            b"BMHD" => bmhd = Some(data),
            b"CMAP" => cmap = data,
            b"CAMG" => camg = be32(data, 0).unwrap_or(0),
            b"BODY" => body = Some(data),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let (bmhd, body) = (bmhd.filter(|h| h.len() >= 20).ok_or_else(|| bad("missing BMHD"))?, body.ok_or_else(|| bad("missing BODY"))?);
    let (w, h) = (u32::from(be16(bmhd, 0).unwrap_or(0)), u32::from(be16(bmhd, 2).unwrap_or(0)));
    let (planes, masking, compression, trans) = (usize::from(bmhd[8]), bmhd[9], bmhd[10], be16(bmhd, 12).unwrap_or(0));
    if !matches!(compression, 0 | 1) {
        return Err(CodecError::unsupported(F, "unsupported IFF compression"));
    }
    if planes == 0 || planes > 32 || (chunky && planes != 8) {
        return Err(CodecError::unsupported(F, "unsupported IFF plane count"));
    }
    let deep = planes >= 24;
    let alpha = (masking == 1 || masking == 2) || planes == 32;
    limits.check(w, h, if alpha { ChannelLayout::Rgba } else { ChannelLayout::Rgb }, SampleType::U8)?;
    let (wu, hu) = (w as usize, h as usize);
    let palette: Vec<[u8; 3]> = cmap.as_chunks::<3>().0.to_vec();
    let ehb = !deep && camg & 0x80 != 0 && planes == 6;
    let ham = !deep && camg & 0x800 != 0 && (planes == 6 || planes == 8);
    let row_bytes = if chunky { wu.next_multiple_of(2) } else { wu.div_ceil(16) * 2 };
    let row_planes = if chunky { 1 } else { planes + usize::from(masking == 1) };
    let mut row = vec![0u8; row_bytes * row_planes];
    let mut bpos = 0usize;
    let ch = if alpha { 4 } else { 3 };
    let mut out = vec![0u8; wu * hu * ch];
    let pal = |i: usize| -> [u8; 3] {
        if ehb && i >= 32 {
            let p = palette.get(i - 32).copied().unwrap_or([0; 3]);
            [p[0] / 2, p[1] / 2, p[2] / 2]
        } else {
            palette.get(i).copied().unwrap_or_else(|| {
                let v = if planes < 8 { (i * 255 / ((1usize << planes) - 1).max(1)) as u8 } else { i as u8 };
                [v; 3]
            })
        }
    };
    for y in 0..hu {
        if compression == 1 {
            unpack(body, &mut bpos, &mut row)?;
        } else {
            let n = row.len();
            row.copy_from_slice(body.get(bpos..bpos + n).ok_or_else(|| bad("truncated image data"))?);
            bpos += n;
        }
        let mut prev = pal(0);
        for x in 0..wu {
            let mut rgb: [u8; 3];
            let mut a = 255u8;
            if chunky {
                rgb = pal(usize::from(row[x]));
                if masking == 2 && u16::from(row[x]) == trans {
                    a = 0;
                }
            } else {
                let bit = |plane: usize| -> usize { usize::from(row[plane * row_bytes + x / 8] >> (7 - x % 8) & 1) };
                if deep {
                    // Planes run red, green, blue (8 each), then alpha for 32.
                    let comp = |c: usize| -> u8 { (0..8).fold(0u8, |v, k| v | (bit(c * 8 + k) as u8) << k) };
                    rgb = [comp(0), comp(1), comp(2)];
                    if planes == 32 {
                        a = comp(3);
                    }
                } else {
                    let index = (0..planes).fold(0usize, |v, p| v | bit(p) << p);
                    if ham {
                        let hb = planes - 2;
                        let (ctl, data) = (index >> hb, index & ((1 << hb) - 1));
                        let scaled = (if hb == 4 { data * 17 } else { data << 2 | data >> 4 }) as u8;
                        rgb = prev;
                        match ctl {
                            0 => rgb = pal(data),
                            1 => rgb[2] = scaled,
                            2 => rgb[0] = scaled,
                            _ => rgb[1] = scaled,
                        }
                        prev = rgb;
                    } else {
                        rgb = pal(index);
                        if masking == 2 && index as u16 == trans {
                            a = 0;
                        }
                    }
                }
                if masking == 1 {
                    a = if bit(planes) == 1 { 255 } else { 0 };
                }
            }
            let o = (y * wu + x) * ch;
            out[o..o + 3].copy_from_slice(&rgb);
            if alpha {
                out[o + 3] = a;
            }
        }
    }
    if !alpha && out.as_chunks::<3>().0.iter().all(|p| p[0] == p[1] && p[1] == p[2]) {
        return Image::from_u8(w, h, ChannelLayout::Gray, out.as_chunks::<3>().0.iter().map(|p| p[0]).collect());
    }
    Image::from_u8(w, h, if alpha { ChannelLayout::Rgba } else { ChannelLayout::Rgb }, out)
}

fn byte_run1(row: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && row[i + run] == row[i] && run < 128 {
            run += 1;
        }
        if run >= 3 {
            out.push((1 - run as i32) as u8);
            out.push(row[i]);
            i += run;
        } else {
            let start = i;
            while i < row.len() && i - start < 128 && !(i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2]) {
                i += 1;
            }
            out.push((i - start - 1) as u8);
            out.extend_from_slice(&row[start..i]);
        }
    }
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w > 65535 || h > 65535 {
        return Err(CodecError::encode(F, "IFF is limited to 65535 pixels per side"));
    }
    let gray = img.layout() == ChannelLayout::Gray;
    let planes = if gray { 8 } else { 24 };
    let row_bytes = w.div_ceil(16) * 2;
    let mut body = Vec::new();
    let mut row = vec![0u8; row_bytes];
    for y in 0..h {
        for p in 0..planes {
            row.fill(0);
            let (c, k) = (p / 8, p % 8);
            for x in 0..w {
                let v = if gray { img.data()[y * w + x] } else { img.data()[(y * w + x) * 3 + c] };
                if v >> k & 1 == 1 {
                    row[x / 8] |= 0x80 >> (x % 8);
                }
            }
            byte_run1(&row, &mut body);
        }
    }
    let mut bmhd = Vec::new();
    for v in [w as u16, h as u16, 0, 0] {
        bmhd.extend(v.to_be_bytes());
    }
    bmhd.extend([planes as u8, 0, 1, 0]);
    bmhd.extend(0u16.to_be_bytes());
    bmhd.extend([1, 1]);
    for v in [w as u16, h as u16] {
        bmhd.extend(v.to_be_bytes());
    }
    let mut chunks: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"BMHD", bmhd)];
    if gray {
        chunks.push((b"CMAP", (0..=255u8).flat_map(|v| [v; 3]).collect()));
    }
    chunks.push((b"BODY", body));
    let mut form = b"ILBM".to_vec();
    for (id, data) in chunks {
        form.extend(id);
        form.extend((data.len() as u32).to_be_bytes());
        form.extend(&data);
        if data.len() % 2 == 1 {
            form.push(0);
        }
    }
    let mut out = b"FORM".to_vec();
    out.extend((form.len() as u32).to_be_bytes());
    out.extend(form);
    Ok(out)
}
