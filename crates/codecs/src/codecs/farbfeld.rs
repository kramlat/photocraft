#![allow(clippy::chunks_exact_to_as_chunks)]
//! Farbfeld: `farbfeld`, width and height as big-endian u32, then 16-bit RGBA, all big-endian.

use super::legacy::be32;
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Farbfeld;

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !b.starts_with(b"farbfeld") {
        return Err(CodecError::malformed(F, "missing farbfeld signature"));
    }
    let (Some(w), Some(h)) = (be32(b, 8), be32(b, 12)) else {
        return Err(CodecError::malformed(F, "truncated header"));
    };
    limits.check(w, h, ChannelLayout::Rgba, SampleType::U16)?;
    let n = w as usize * h as usize * 4;
    let body = b.get(16..).unwrap_or_default();
    if body.len() < n * 2 {
        return Err(CodecError::malformed(F, "truncated pixel data"));
    }
    let samples: Vec<u16> = body[..n * 2].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    Image::from_u16(w, h, ChannelLayout::Rgba, &samples)
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let samples = img.to_u16_samples().ok_or_else(|| CodecError::encode(F, "expected 16-bit samples"))?;
    let mut out = Vec::with_capacity(16 + samples.len() * 2);
    out.extend_from_slice(b"farbfeld");
    out.extend_from_slice(&img.width().to_be_bytes());
    out.extend_from_slice(&img.height().to_be_bytes());
    for v in samples {
        out.extend_from_slice(&v.to_be_bytes());
    }
    Ok(out)
}
