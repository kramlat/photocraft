//! X11 cursors (Xcursor, `XMC`): `Xcur`, a table of contents, then image chunks of
//! premultiplied ARGB32. Reads the largest image of the first size. Read-only.

use crate::error::CodecError;
use crate::format::Format;
use crate::image::{ChannelLayout, Image, SampleType};
use crate::options::Limits;

const F: Format = Format::Xcursor;

fn bad(m: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, m)
}
fn le32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

pub(crate) fn is_xcursor(b: &[u8]) -> bool {
    b.starts_with(b"Xcur")
}

pub(crate) fn decode(b: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    if !is_xcursor(b) {
        return Err(bad("missing Xcur signature"));
    }
    let n = le32(b, 12).ok_or_else(|| bad("truncated header"))?.min(4096);
    // Image entries: (nominal size, offset); the largest nominal size wins.
    let mut best: Option<(usize, usize)> = None;
    for i in 0..n {
        let at = 16 + i * 12;
        let (ty, size, off) = (le32(b, at).ok_or_else(|| bad("truncated table"))?, le32(b, at + 4).unwrap_or(0), le32(b, at + 8).unwrap_or(0));
        if ty == 0xfffd_0002 && best.is_none_or(|(s, _)| size > s) {
            best = Some((size, off));
        }
    }
    let (_, off) = best.ok_or_else(|| bad("no image in the cursor"))?;
    let (w, h) = (le32(b, off + 16).ok_or_else(|| bad("truncated image"))?, le32(b, off + 20).ok_or_else(|| bad("truncated image"))?);
    let (wu, hu) = (u32::try_from(w).map_err(|_| bad("bad size"))?, u32::try_from(h).map_err(|_| bad("bad size"))?);
    limits.check(wu, hu, ChannelLayout::Rgba, SampleType::U8)?;
    let data = b.get(off + 36..off + 36 + w * h * 4).ok_or_else(|| bad("truncated pixel data"))?;
    let mut out = Vec::with_capacity(w * h * 4);
    for p in data.as_chunks::<4>().0 {
        let argb = u32::from_le_bytes(*p);
        let (a, r, g, bl) = ((argb >> 24) as u8, (argb >> 16) as u8, (argb >> 8) as u8, argb as u8);
        // Premultiplied to straight alpha.
        let un = |c: u8| if a == 0 { 0 } else { ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8 };
        out.extend([un(r), un(g), un(bl), a]);
    }
    Image::from_u8(wu, hu, ChannelLayout::Rgba, out)
}
