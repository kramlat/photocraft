//! Photoshop Raw (`.raw`): headerless pixel data whose shape the user states: width, height,
//! channel count, 8 or 16 bits, byte order, interleaved or one plane per channel, and how many
//! header bytes to skip. One to four channels read as gray, gray and alpha, RGB and RGBA.

use crate::error::CodecError;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::Limits;

/// The numbers Photoshop's Raw Options dialog asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawParams {
    pub width: u32,
    pub height: u32,
    /// 1 = gray, 2 = gray and alpha, 3 = RGB, 4 = RGBA.
    pub channels: u8,
    /// 8 or 16 bits per sample.
    pub depth: u8,
    /// Multi-byte samples are most-significant byte first (Mac order) rather than IBM PC order.
    pub big_endian: bool,
    /// Samples are pixel by pixel (RGBRGB…) rather than a plane per channel (RR…GG…BB…).
    pub interleaved: bool,
    /// Bytes to skip at the start of the file.
    pub header: usize,
}

fn layout(channels: u8) -> Result<ChannelLayout, CodecError> {
    Ok(match channels {
        1 => ChannelLayout::Gray,
        2 => ChannelLayout::GrayA,
        3 => ChannelLayout::Rgb,
        4 => ChannelLayout::Rgba,
        _ => return Err(CodecError::InvalidImage("Photoshop Raw has 1 to 4 channels".into())),
    })
}

/// Decodes `bytes` as the stated shape; too few bytes is an error.
pub fn decode_raw(bytes: &[u8], p: &RawParams, limits: &Limits) -> Result<Image, CodecError> {
    let lay = layout(p.channels)?;
    let wide = match p.depth {
        8 => false,
        16 => true,
        _ => return Err(CodecError::InvalidImage("Photoshop Raw is 8 or 16 bits".into())),
    };
    limits.check(p.width, p.height, lay, if wide { SampleType::U16 } else { SampleType::U8 })?;
    let (w, h, c) = (p.width as usize, p.height as usize, usize::from(p.channels));
    let unit = if wide { 2 } else { 1 };
    let need = w * h * c * unit;
    let data = bytes.get(p.header.min(bytes.len())..).unwrap_or_default();
    if data.len() < need {
        return Err(CodecError::malformed(Format::Pnm, format!("the file has {} bytes of pixel data; {w} x {h} x {c} x {} bits needs {need}", data.len(), p.depth)));
    }
    let sample = |i: usize| -> u16 {
        if wide {
            let b = [data[i * 2], data[i * 2 + 1]];
            if p.big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
        } else {
            u16::from(data[i])
        }
    };
    // Index of channel `ch` of pixel `px` in the stored order.
    let at = |px: usize, ch: usize| if p.interleaved { px * c + ch } else { ch * w * h + px };
    if wide {
        let mut out = vec![0u16; w * h * c];
        for px in 0..w * h {
            for ch in 0..c {
                out[px * c + ch] = sample(at(px, ch));
            }
        }
        Image::from_u16(p.width, p.height, lay, &out)
    } else {
        let mut out = vec![0u8; w * h * c];
        for px in 0..w * h {
            for ch in 0..c {
                out[px * c + ch] = sample(at(px, ch)) as u8;
            }
        }
        Image::from_u8(p.width, p.height, lay, out)
    }
}

/// Writes `img` (8-bit stays 8-bit, anything deeper is written as 16-bit) as headerless raw data.
pub fn encode_raw(img: &Image, big_endian: bool, interleaved: bool) -> Result<Vec<u8>, CodecError> {
    let c = img.layout().channels();
    let wide = img.sample_type() != SampleType::U8;
    let converted = img.converted(img.layout(), if wide { SampleType::U16 } else { SampleType::U8 });
    let (w, h) = (img.width() as usize, img.height() as usize);
    let samples: Vec<u16> = if wide { converted.to_u16_samples().unwrap_or_default() } else { converted.data().iter().map(|v| u16::from(*v)).collect() };
    let mut out = Vec::with_capacity(samples.len() * if wide { 2 } else { 1 });
    let mut put = |v: u16| {
        if !wide {
            out.push(v as u8);
        } else if big_endian {
            out.extend(v.to_be_bytes());
        } else {
            out.extend(v.to_le_bytes());
        }
    };
    if interleaved {
        samples.iter().for_each(|v| put(*v));
    } else {
        for ch in 0..c {
            for px in 0..w * h {
                put(samples[px * c + ch]);
            }
        }
    }
    Ok(out)
}
