//! JPEG 2000 (`.jp2`, `.j2k`, `.j2c`, `.jpf`). Reads with `hayro-jpeg2000` (pure Rust): JP2 files
//! and bare codestreams, 1–4 components, any bit depth up to 16 (shown as 8 or 16 bits), palettes
//! resolved, an embedded ICC profile kept. Every call into the decoder runs under
//! `catch_unwind`, so a decoder panic on a malformed file is an error.

use crate::error::CodecError;
use crate::format::Format;
use crate::fidelity::Plan;
use crate::image::{ChannelLayout, Image};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Jp2;

pub(crate) fn is_jp2(b: &[u8]) -> bool {
    b.starts_with(b"\x00\x00\x00\x0cjP  \r\n\x87\n") || b.starts_with(&[0xff, 0x4f, 0xff, 0x51])
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext};
    let bad = |m: String| CodecError::malformed(F, m);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let image = hayro_jpeg2000::Image::new(b, &DecodeSettings::default()).map_err(|e| bad(e.to_string()))?;
        let (w, h) = (image.width(), image.height());
        let alpha = image.has_alpha();
        let layout = match (image.color_space(), alpha) {
            (ColorSpace::Gray, false) => ChannelLayout::Gray,
            (ColorSpace::Gray, true) => ChannelLayout::GrayA,
            (ColorSpace::RGB, false) => ChannelLayout::Rgb,
            (ColorSpace::RGB, true) => ChannelLayout::Rgba,
            (ColorSpace::CMYK, false) => ChannelLayout::Cmyk,
            (ColorSpace::Icc { num_channels, .. }, a) => match (num_channels, a) {
                (1, false) => ChannelLayout::Gray,
                (1, true) => ChannelLayout::GrayA,
                (3, false) => ChannelLayout::Rgb,
                (3, true) => ChannelLayout::Rgba,
                (4, false) => ChannelLayout::Cmyk,
                _ => return Err(CodecError::unsupported(F, "unsupported JPEG 2000 channel layout")),
            },
            _ => return Err(CodecError::unsupported(F, "unsupported JPEG 2000 colour space")),
        };
        let icc = match image.color_space() {
            ColorSpace::Icc { profile, .. } if !profile.is_empty() => Some(profile.clone()),
            _ => None,
        };
        let wide = image.original_bit_depth() > 8;
        limits.check(w, h, layout, if wide { crate::SampleType::U16 } else { crate::SampleType::U8 })?;
        let mut ctx = DecoderContext::default();
        let decoded = image.decode(&mut ctx).map_err(|e| bad(e.to_string()))?;
        let comps = decoded.components();
        if comps.len() != layout.channels() {
            return Err(CodecError::unsupported(F, "JPEG 2000 image with extra channels"));
        }
        let n = w as usize * h as usize;
        if comps.iter().any(|c| c.samples().len() < n) {
            return Err(bad("truncated component data".into()));
        }
        let scale = |c: &hayro_jpeg2000::ComponentData, v: f32, top: f32| -> f32 { (v * top / ((1u32 << c.bit_depth().min(31)) - 1) as f32).clamp(0.0, top) };
        let nc = comps.len();
        let out = if wide {
            let mut v = vec![0u16; n * nc];
            for (ci, c) in comps.iter().enumerate() {
                for (i, s) in c.samples()[..n].iter().enumerate() {
                    v[i * nc + ci] = scale(c, *s, 65535.0).round() as u16;
                }
            }
            Image::from_u16(w, h, layout, &v)?
        } else {
            let mut v = vec![0u8; n * nc];
            for (ci, c) in comps.iter().enumerate() {
                for (i, s) in c.samples()[..n].iter().enumerate() {
                    v[i * nc + ci] = scale(c, *s, 255.0).round() as u8;
                }
            }
            Image::from_u8(w, h, layout, v)?
        };
        Ok(out.with_icc(icc))
    }))
    .unwrap_or_else(|_| Err(bad("the JPEG 2000 decoder failed on this file".into())))
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// Lossless JP2: 16-bit unsigned components (8-bit data is widened exactly, `v * 257`), the
/// colour space as sRGB / gray or the image's ICC profile, alpha declared through a `cdef` box.
pub(crate) fn encode(src: &Image, plan: Plan, opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width(), img.height());
    let layout = img.layout();
    let nc = layout.channels();
    let samples = img.to_u16_samples().ok_or_else(|| CodecError::encode(F, "expected 16-bit samples"))?;
    let planes: Vec<Vec<u16>> = (0..nc).map(|c| samples.iter().skip(c).step_by(nc).copied().collect()).collect();
    let code = std::panic::catch_unwind(|| super::j2k::encode_planes(&planes, w, h))
        .map_err(|_| CodecError::encode(F, "the JPEG 2000 encoder failed on this image"))?
        .map_err(|e| CodecError::encode(F, e.to_string()))?;

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&(nc as u16).to_be_bytes());
    ihdr.extend_from_slice(&[15, 7, 0, 0]);
    let colour = layout.channels() - usize::from(layout.has_alpha());
    let colr = match img.icc.as_ref().filter(|_| opts.embed_icc) {
        Some(icc) => {
            let mut b = vec![2, 0, 0];
            b.extend_from_slice(icc);
            b
        }
        None => {
            let mut b = vec![1, 0, 0];
            b.extend_from_slice(&(if colour == 1 { 17u32 } else { 16 }).to_be_bytes());
            b
        }
    };
    let mut header = boxed(b"ihdr", &ihdr);
    header.extend(boxed(b"colr", &colr));
    if layout.has_alpha() {
        let mut cdef = (nc as u16).to_be_bytes().to_vec();
        for c in 0..nc as u16 {
            let (typ, asoc) = if usize::from(c) < colour { (0u16, c + 1) } else { (1, 0) };
            for v in [c, typ, asoc] {
                cdef.extend_from_slice(&v.to_be_bytes());
            }
        }
        header.extend(boxed(b"cdef", &cdef));
    }
    let mut out = b"\x00\x00\x00\x0cjP  \r\n\x87\n".to_vec();
    out.extend(boxed(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    out.extend(boxed(b"jp2h", &header));
    out.extend(boxed(b"jp2c", &code));
    Ok(out)
}
