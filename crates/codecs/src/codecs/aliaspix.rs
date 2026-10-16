//! Alias|Wavefront PIX: a 10-byte big-endian header (width, height, x and y offset, bits per
//! pixel: 24 or 8) then run-length packets `count, value…` where `count` is 1–255 and a value is
//! one gray byte or three bytes in blue, green, red order. Runs carry on across row ends.
//!
//! The format has no magic number: [`looks_like_pix`] accepts a file only when its packets add up
//! to exactly the declared pixel count and fill the file.

use super::legacy::be16;
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::AliasPix;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}

fn header(b: &[u8]) -> Option<(u32, u32, usize)> {
    let (w, h, depth) = (u32::from(be16(b, 0)?), u32::from(be16(b, 2)?), be16(b, 8)?);
    (w > 0 && h > 0 && matches!(depth, 8 | 24)).then_some((w, h, usize::from(depth) / 8))
}

pub(crate) fn looks_like_pix(b: &[u8]) -> bool {
    let Some((w, h, bytes)) = header(b) else { return false };
    let (mut pos, mut pixels) = (10usize, 0u64);
    let total = u64::from(w) * u64::from(h);
    while pixels < total {
        let Some(count) = b.get(pos).copied().filter(|c| *c != 0) else { return false };
        pos += 1 + bytes;
        pixels += u64::from(count);
    }
    pixels == total && pos == b.len()
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    let (w, h, bytes) = header(b).ok_or_else(|| bad("invalid header"))?;
    let layout = if bytes == 1 { ChannelLayout::Gray } else { ChannelLayout::Rgb };
    limits.check(w, h, layout, SampleType::U8)?;
    let total = w as usize * h as usize;
    let mut out = Vec::with_capacity(total * bytes);
    let mut pos = 10;
    let mut pixels = 0usize;
    while pixels < total {
        let count = usize::from(*b.get(pos).ok_or_else(|| bad("truncated pixel data"))?);
        let v = b.get(pos + 1..pos + 1 + bytes).ok_or_else(|| bad("truncated pixel data"))?;
        if count == 0 || pixels + count > total {
            return Err(bad("bad run length"));
        }
        for _ in 0..count {
            if bytes == 1 {
                out.push(v[0]);
            } else {
                out.extend_from_slice(&[v[2], v[1], v[0]]);
            }
        }
        pos += 1 + bytes;
        pixels += count;
    }
    Image::from_u8(w, h, layout, out)
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width(), img.height());
    if w > 65535 || h > 65535 {
        return Err(CodecError::encode(F, "Alias PIX is limited to 65535 pixels per side"));
    }
    let bytes = img.layout().channels();
    let mut out = Vec::new();
    for v in [w as u16, h as u16, 0, 0, (bytes * 8) as u16] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    for row in img.data().chunks_exact(w as usize * bytes) {
        let px: Vec<&[u8]> = row.chunks_exact(bytes).collect();
        let mut i = 0;
        while i < px.len() {
            let mut run = 1;
            while i + run < px.len() && px[i + run] == px[i] && run < 255 {
                run += 1;
            }
            out.push(run as u8);
            if bytes == 1 {
                out.push(px[i][0]);
            } else {
                out.extend_from_slice(&[px[i][2], px[i][1], px[i][0]]);
            }
            i += run;
        }
    }
    Ok(out)
}
