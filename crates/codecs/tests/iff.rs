//! Amiga IFF reading beyond what our own writer produces, built by hand from the EA IFF 85 and
//! ILBM specifications.

use photocraft_codecs::*;

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend((data.len() as u32).to_be_bytes());
    v.extend(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn form(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut body = kind.to_vec();
    chunks.iter().for_each(|c| body.extend(c));
    let mut v = b"FORM".to_vec();
    v.extend((body.len() as u32).to_be_bytes());
    v.extend(body);
    v
}

fn bmhd(w: u16, h: u16, planes: u8, masking: u8, compression: u8, trans: u16) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [w, h, 0, 0] {
        v.extend(x.to_be_bytes());
    }
    v.extend([planes, masking, compression, 0]);
    v.extend(trans.to_be_bytes());
    v.extend([1, 1]);
    v.extend(w.to_be_bytes());
    v.extend(h.to_be_bytes());
    v
}

/// Pixel indices as planar rows (one row per slice entry), `planes` bit-planes, 16-bit padded.
fn planar(rows: &[Vec<u8>], planes: usize, mask: Option<&[Vec<u8>]>) -> Vec<u8> {
    let mut out = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        for p in 0..planes {
            let mut bytes = vec![0u8; row.len().div_ceil(16) * 2];
            for (x, v) in row.iter().enumerate() {
                if v >> p & 1 == 1 {
                    bytes[x / 8] |= 0x80 >> (x % 8);
                }
            }
            out.extend(bytes);
        }
        if let Some(m) = mask {
            let mut bytes = vec![0u8; row.len().div_ceil(16) * 2];
            for (x, v) in m[y].iter().enumerate() {
                if *v == 1 {
                    bytes[x / 8] |= 0x80 >> (x % 8);
                }
            }
            out.extend(bytes);
        }
    }
    out
}

#[test]
fn indexed_ilbm_with_a_colour_map() {
    let cmap = [255u8, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
    let body = planar(&[vec![0, 1, 2, 3], vec![3, 2, 1, 0]], 2, None);
    let f = form(b"ILBM", &[chunk(b"BMHD", &bmhd(4, 2, 2, 0, 0, 0)), chunk(b"CMAP", &cmap), chunk(b"BODY", &body)]);
    assert_eq!(detect(&f), Some(Format::Iff));
    let im = decode(&f).unwrap();
    assert_eq!(im.layout(), ChannelLayout::Rgb);
    assert_eq!(&im.data()[..12], &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
    assert_eq!(&im.data()[12..15], &[255, 255, 0]);
}

#[test]
fn byterun1_bodies_and_mask_planes() {
    // 20 pixels wide (two 16-bit words per row), one plane, one row: all index 1. PackBits: the
    // row's four bytes are FF FF F0 00 (20 set bits then padding): literal 4.
    let cmap = [0u8, 0, 0, 200, 100, 50];
    let row = vec![1u8; 20];
    let raw = planar(std::slice::from_ref(&row), 1, Some(&[(0..20).map(|x| u8::from(x % 2 == 0)).collect()]));
    let mut packed = Vec::new();
    for r in raw.chunks(4) {
        packed.push(3);
        packed.extend(r);
    }
    let f = form(b"ILBM", &[chunk(b"BMHD", &bmhd(20, 1, 1, 1, 1, 0)), chunk(b"CMAP", &cmap), chunk(b"BODY", &packed)]);
    let im = decode(&f).unwrap();
    assert_eq!(im.layout(), ChannelLayout::Rgba);
    assert_eq!(&im.data()[..8], &[200, 100, 50, 255, 200, 100, 50, 0]);
}

#[test]
fn ham6_holds_and_modifies() {
    // Palette 0 = (0,0,0), 1 = (0xff, 0, 0). Pixels: 0b00_0001 (palette 1), 0b10_1000 (modify red
    // to 8 -> 0x88), 0b01_1111 (modify blue to 15 -> 0xff), 0b11_0100 (modify green to 4 -> 0x44).
    let mut cmap = vec![0u8; 16 * 3];
    cmap[3] = 255;
    let body = planar(&[vec![0b00_0001, 0b10_1000, 0b01_1111, 0b11_0100]], 6, None);
    let f = form(b"ILBM", &[chunk(b"BMHD", &bmhd(4, 1, 6, 0, 0, 0)), chunk(b"CAMG", &0x800u32.to_be_bytes()), chunk(b"CMAP", &cmap), chunk(b"BODY", &body)]);
    let im = decode(&f).unwrap();
    assert_eq!(im.data(), [255, 0, 0, 0x88, 0, 0, 0x88, 0, 0xff, 0x88, 0x44, 0xff]);
}

#[test]
fn extra_half_brite_halves_the_upper_colours() {
    let mut cmap = vec![0u8; 32 * 3];
    cmap[..3].copy_from_slice(&[200, 100, 50]);
    let body = planar(&[vec![0, 32]], 6, None);
    let f = form(b"ILBM", &[chunk(b"BMHD", &bmhd(2, 1, 6, 0, 0, 0)), chunk(b"CAMG", &0x80u32.to_be_bytes()), chunk(b"CMAP", &cmap), chunk(b"BODY", &body)]);
    assert_eq!(decode(&f).unwrap().data(), [200, 100, 50, 100, 50, 25]);
}

#[test]
fn deep_24_and_pbm_chunky() {
    // 24-bit: planes are red bits 0-7, green, blue; one pixel (r, g, b) = (5, 0x80, 0xff).
    let mut body = Vec::new();
    for comp in [5u8, 0x80, 0xff] {
        for bit in 0..8 {
            body.extend([if comp >> bit & 1 == 1 { 0x80 } else { 0 }, 0]);
        }
    }
    let f = form(b"ILBM", &[chunk(b"BMHD", &bmhd(1, 1, 24, 0, 0, 0)), chunk(b"BODY", &body)]);
    assert_eq!(decode(&f).unwrap().data(), [5, 0x80, 0xff]);
    // PBM: 8-bit chunky rows, rows padded to even length, transparent colour 1.
    let cmap = [0u8, 0, 0, 10, 20, 30, 40, 50, 60];
    let f = form(b"PBM ", &[chunk(b"BMHD", &bmhd(3, 1, 8, 2, 0, 1)), chunk(b"CMAP", &cmap), chunk(b"BODY", &[2, 1, 0, 0])]);
    let im = decode(&f).unwrap();
    assert_eq!((im.layout(), im.data()), (ChannelLayout::Rgba, &[40u8, 50, 60, 255, 10, 20, 30, 0, 0, 0, 0, 255][..]));
}

#[test]
fn hostile_iff_errors() {
    let cmap = [0u8; 6];
    let f = form(b"ILBM", &[chunk(b"BMHD", &bmhd(40, 40, 1, 0, 1, 0)), chunk(b"CMAP", &cmap), chunk(b"BODY", &[0x7f, 1, 2])]);
    assert!(decode(&f).is_err());
    for cut in 0..f.len() {
        let _ = decode_as(Format::Iff, &f[..cut]);
    }
    assert!(decode(&form(b"ILBM", &[chunk(b"BODY", &[])])).is_err(), "no BMHD");
}

#[test]
fn icns_reads_the_largest_picture_png_or_classic() {
    let png = encode(&Image::from_u8(16, 16, ChannelLayout::Rgba, [10u8, 20, 30, 255].repeat(256)).unwrap(), Format::Png, &EncodeOptions::default()).unwrap();
    let element = |ty: &[u8; 4], data: &[u8]| {
        let mut v = ty.to_vec();
        v.extend(((data.len() + 8) as u32).to_be_bytes());
        v.extend(data);
        v
    };
    let wrap = |parts: Vec<Vec<u8>>| {
        let body: Vec<u8> = parts.concat();
        let mut v = b"icns".to_vec();
        v.extend(((body.len() + 8) as u32).to_be_bytes());
        v.extend(body);
        v
    };
    let f = wrap(vec![element(b"icp4", &png)]);
    assert_eq!(detect(&f), Some(Format::Icns));
    assert_eq!(decode(&f).unwrap().data()[..4], [10, 20, 30, 255]);
    // Classic 16 px: three RLE channels of 256 values and an 8-bit mask. A run byte c >= 128
    // repeats the next byte c - 125 times: 255 gives 130, 253 gives 128, and the decoder trims.
    let channel = |v: u8| vec![255u8, v, 253, v];
    let mut rle = Vec::new();
    for v in [200u8, 100, 50] {
        rle.extend(channel(v));
    }
    let f = wrap(vec![element(b"is32", &rle), element(b"s8mk", &[128u8; 256])]);
    let im = decode(&f).unwrap();
    assert_eq!((im.width(), &im.data()[..4]), (16, &[200u8, 100, 50, 128][..]));
}
