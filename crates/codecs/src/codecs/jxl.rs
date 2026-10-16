//! JPEG XL (`.jxl`). Reads with `jxl-oxide` (pure Rust; native builds only, as in the raw crate)
//! and writes with `zune-jpegxl`, a pure-Rust **lossless** encoder: 8 and 16 bits, gray or RGB,
//! with or without alpha, at least 2 × 2 pixels.
//!
//! Every call into the decoder runs under `catch_unwind`, so a decoder panic on a malformed file
//! is an error.

use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Jxl;

/// A bare codestream (`FF 0A`) or the ISO BMFF container.
pub(crate) fn is_jxl(b: &[u8]) -> bool {
    b.starts_with(&[0xff, 0x0a]) || b.starts_with(&[0, 0, 0, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a])
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn decode(_b: &[u8], _l: &Limits) -> Result<Image, CodecError> {
    Err(CodecError::unsupported(F, "JPEG XL can't be opened in the browser version of PhotoCraft; open it in the desktop app"))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    use jxl_oxide::{AllocTracker, JxlImage, PixelFormat};
    let bad = |m: String| CodecError::malformed(F, m);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let budget = usize::try_from(limits.max_alloc.saturating_mul(4)).unwrap_or(usize::MAX);
        let image = JxlImage::builder().alloc_tracker(AllocTracker::with_limit(budget)).read(b).map_err(|e| bad(e.to_string()))?;
        let layout = match image.pixel_format() {
            PixelFormat::Gray => ChannelLayout::Gray,
            PixelFormat::Graya => ChannelLayout::GrayA,
            PixelFormat::Rgb => ChannelLayout::Rgb,
            PixelFormat::Rgba => ChannelLayout::Rgba,
            PixelFormat::Cmyk | PixelFormat::Cmyka => return Err(CodecError::unsupported(F, "CMYK JPEG XL images are not supported")),
        };
        let depth = image.image_header().metadata.bit_depth;
        let bits = depth.bits_per_sample();
        let float = matches!(depth, jxl_oxide::image::BitDepth::FloatSample { .. });
        let sample = if float {
            SampleType::F32
        } else if bits > 8 {
            SampleType::U16
        } else {
            SampleType::U8
        };
        let (w, h) = (image.width(), image.height());
        limits.check(w, h, layout, sample)?;
        let render = image.render_frame(0).map_err(|e| bad(e.to_string()))?;
        let mut stream = render.stream();
        let n = w as usize * h as usize * layout.channels();
        if (stream.width(), stream.height()) != (w, h) {
            return Err(CodecError::unsupported(F, "rotated JPEG XL images are not supported"));
        }
        if stream.channels() as usize != layout.channels() {
            return Err(CodecError::unsupported(F, "JPEG XL image with extra channels"));
        }
        let out = match sample {
            SampleType::U8 => {
                let mut v = vec![0u8; n];
                if stream.write_to_buffer(&mut v) != n {
                    return Err(bad("truncated image data".into()));
                }
                Image::from_u8(w, h, layout, v)?
            }
            SampleType::U16 => {
                let mut v = vec![0u16; n];
                if stream.write_to_buffer(&mut v) != n {
                    return Err(bad("truncated image data".into()));
                }
                Image::from_u16(w, h, layout, &v)?
            }
            _ => {
                let mut v = vec![0f32; n];
                if stream.write_to_buffer(&mut v) != n {
                    return Err(bad("truncated image data".into()));
                }
                Image::from_f32(w, h, layout, &v)?
            }
        };
        let icc = image.rendered_icc();
        Ok(out.with_icc((!icc.is_empty()).then_some(icc)))
    }))
    .unwrap_or_else(|_| Err(bad("the JPEG XL decoder failed on this file".into())))
}

pub(crate) fn encode(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    use super::jxl_enc::JxlSimpleEncoder;
    use zune_core::bit_depth::BitDepth;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::EncoderOptions;
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 2 || h < 2 {
        return Err(CodecError::encode(F, "the JPEG XL encoder needs at least 2 x 2 pixels"));
    }
    let colorspace = match img.layout() {
        ChannelLayout::Gray => ColorSpace::Luma,
        ChannelLayout::GrayA => ColorSpace::LumaA,
        ChannelLayout::Rgb => ColorSpace::RGB,
        _ => ColorSpace::RGBA,
    };
    let (depth, bytes) = if img.sample_type() == SampleType::U16 {
        (BitDepth::Sixteen, img.to_u16_samples().unwrap_or_default().iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>())
    } else {
        (BitDepth::Eight, img.data().to_vec())
    };
    let options = EncoderOptions::default().set_width(w).set_height(h).set_depth(depth).set_colorspace(colorspace);
    // The forked encoder asserts on internal invariants: a failure there is an error, not a crash.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut out = Vec::new();
        JxlSimpleEncoder::new(&bytes, options).encode(&mut out).map_err(|e| CodecError::encode(F, format!("{e:?}")))?;
        Ok(out)
    }))
    .unwrap_or_else(|_| Err(CodecError::encode(F, "the JPEG XL encoder failed on this image")))
}
