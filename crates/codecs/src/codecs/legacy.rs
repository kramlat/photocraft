//! Small helpers shared by the legacy raster readers/writers (Sun Raster, SGI, XBM, WBMP,
//! Farbfeld, XWD, Alias PIX).

pub(crate) fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at.checked_add(2)?).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

pub(crate) fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Gray 8-bit pixels → 1 = "set" bit per pixel, rows padded to whole bytes. `msb_first` picks the
/// bit order; a pixel is set when `is_set(value)`.
pub(crate) fn pack_bits(gray: &[u8], w: usize, h: usize, msb_first: bool, is_set: impl Fn(u8) -> bool) -> Vec<u8> {
    let stride = w.div_ceil(8);
    let mut out = vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            if is_set(gray[y * w + x]) {
                out[y * stride + x / 8] |= if msb_first { 0x80 >> (x % 8) } else { 1 << (x % 8) };
            }
        }
    }
    out
}

/// Inverse of [`pack_bits`]: bit at (x, y) of rows padded to whole bytes.
pub(crate) fn bit_at(rows: &[u8], stride: usize, x: usize, y: usize, msb_first: bool) -> bool {
    let byte = rows[y * stride + x / 8];
    byte & if msb_first { 0x80 >> (x % 8) } else { 1 << (x % 8) } != 0
}
