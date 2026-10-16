//! DICOM read paths a writer of our own never takes: implicit VR, windowing, signed data,
//! palettes, MONOCHROME1 and RLE. The files are built by hand from the standard's layout.

use photocraft_codecs::*;

fn elem(out: &mut Vec<u8>, g: u16, e: u16, vr: &[u8; 2], value: &[u8], explicit: bool) {
    let mut v = value.to_vec();
    if v.len() % 2 == 1 {
        v.push(0);
    }
    out.extend(g.to_le_bytes());
    out.extend(e.to_le_bytes());
    if explicit {
        out.extend(vr);
        if matches!(vr, b"OB" | b"OW") {
            out.extend([0, 0]);
            out.extend((v.len() as u32).to_le_bytes());
        } else {
            out.extend((v.len() as u16).to_le_bytes());
        }
    } else {
        out.extend((v.len() as u32).to_le_bytes());
    }
    out.extend(v);
}

struct Spec<'a> {
    transfer: &'a str,
    photometric: &'a str,
    spp: u16,
    rows: u16,
    cols: u16,
    bits: u16,
    signed: bool,
    extra: Vec<(u16, u16, [u8; 2], Vec<u8>)>,
    pixels: Vec<u8>,
    pixel_vr: [u8; 2],
}

fn build(s: &Spec) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out.extend(b"DICM");
    let mut meta = Vec::new();
    elem(&mut meta, 2, 0x10, b"UI", s.transfer.as_bytes(), true);
    let mut gl = Vec::new();
    elem(&mut gl, 2, 0, b"UL", &(meta.len() as u32).to_le_bytes(), true);
    out.extend(gl);
    out.extend(meta);
    let explicit = s.transfer != "1.2.840.10008.1.2";
    // A sequence of undefined length first: it must be skipped whole.
    out.extend([0x08, 0x00, 0x15, 0x11]);
    if explicit {
        out.extend(b"SQ\0\0");
    }
    out.extend(0xffff_ffffu32.to_le_bytes());
    out.extend([0xfe, 0xff, 0x00, 0xe0]);
    out.extend(0xffff_ffffu32.to_le_bytes());
    elem(&mut out, 8, 0x1150, b"UI", b"1.2.3", explicit);
    out.extend([0xfe, 0xff, 0x0d, 0xe0, 0, 0, 0, 0]);
    out.extend([0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0]);
    elem(&mut out, 0x28, 2, b"US", &s.spp.to_le_bytes(), explicit);
    elem(&mut out, 0x28, 4, b"CS", s.photometric.as_bytes(), explicit);
    elem(&mut out, 0x28, 0x10, b"US", &s.rows.to_le_bytes(), explicit);
    elem(&mut out, 0x28, 0x11, b"US", &s.cols.to_le_bytes(), explicit);
    elem(&mut out, 0x28, 0x100, b"US", &(if s.bits > 8 { 16u16 } else { 8 }).to_le_bytes(), explicit);
    elem(&mut out, 0x28, 0x101, b"US", &s.bits.to_le_bytes(), explicit);
    elem(&mut out, 0x28, 0x102, b"US", &(s.bits - 1).to_le_bytes(), explicit);
    elem(&mut out, 0x28, 0x103, b"US", &u16::from(s.signed).to_le_bytes(), explicit);
    for (g, e, vr, v) in &s.extra {
        elem(&mut out, *g, *e, vr, v, explicit);
    }
    elem(&mut out, 0x7fe0, 0x10, &s.pixel_vr, &s.pixels, explicit);
    out
}

fn gray(bits: u16, signed: bool, photometric: &str, pixels: Vec<u8>, extra: Vec<(u16, u16, [u8; 2], Vec<u8>)>, transfer: &str) -> Vec<u8> {
    build(&Spec { transfer, photometric, spp: 1, rows: 1, cols: (pixels.len() / if bits > 8 { 2 } else { 1 }) as u16, bits, signed, extra, pixels, pixel_vr: *b"OW" })
}

const IMPLICIT: &str = "1.2.840.10008.1.2";
const EXPLICIT: &str = "1.2.840.10008.1.2.1";

#[test]
fn plain_images_are_read_as_stored_in_both_vr_styles() {
    for t in [IMPLICIT, EXPLICIT] {
        let b = gray(8, false, "MONOCHROME2", vec![0, 128, 255], vec![], t);
        assert_eq!(detect(&b), Some(Format::Dicom));
        let im = decode(&b).unwrap();
        assert_eq!((im.layout(), im.sample_type(), im.data()), (ChannelLayout::Gray, SampleType::U8, &[0u8, 128, 255][..]), "{t}");
    }
    // 12 bits stored in 16: scaled to the full 16-bit range.
    let b = gray(12, false, "MONOCHROME2", [0u16, 2048, 4095].iter().flat_map(|v| v.to_le_bytes()).collect(), vec![], EXPLICIT);
    assert_eq!(decode(&b).unwrap().to_u16_samples().unwrap(), [0, 32776, 65535]);
}

#[test]
fn monochrome1_is_inverted() {
    let b = gray(8, false, "MONOCHROME1", vec![0, 255], vec![], EXPLICIT);
    assert_eq!(decode(&b).unwrap().data(), [255, 0]);
}

#[test]
fn signed_rescaled_data_uses_its_window_or_its_range() {
    // Signed samples with slope 2 and intercept 10.
    let stored: Vec<u8> = [-100i16, 0, 100].iter().flat_map(|v| v.to_le_bytes()).collect();
    let extra = vec![(0x28, 0x1052, *b"DS", b"10".to_vec()), (0x28, 0x1053, *b"DS", b"2".to_vec())];
    let b = gray(16, true, "MONOCHROME2", stored.clone(), extra.clone(), EXPLICIT);
    // Real values -190, 10, 210: stretched over their range.
    assert_eq!(decode(&b).unwrap().to_u16_samples().unwrap(), [0, 32768, 65535]);
    // With a window of centre 10 width 200 (-90..110): values outside clip.
    let mut with_window = extra;
    with_window.push((0x28, 0x1050, *b"DS", b"10".to_vec()));
    with_window.push((0x28, 0x1051, *b"DS", b"200".to_vec()));
    let b = gray(16, true, "MONOCHROME2", stored, with_window, EXPLICIT);
    let v = decode(&b).unwrap().to_u16_samples().unwrap();
    assert_eq!((v[0], v[2]), (0, 65535));
    assert!((i32::from(v[1]) - 32768).abs() < 400, "{}", v[1]);
}

#[test]
fn palette_color_looks_up_each_pixel() {
    let lut = |f: fn(u16) -> u16| -> Vec<u8> { (0u16..4).flat_map(|i| f(i).to_le_bytes()).collect() };
    let extra = vec![
        (0x28, 0x1101, *b"US", [4u16, 0, 16].iter().flat_map(|v| v.to_le_bytes()).collect()),
        (0x28, 0x1201, *b"OW", lut(|i| i * 0x4000)),
        (0x28, 0x1202, *b"OW", lut(|_| 0)),
        (0x28, 0x1203, *b"OW", lut(|i| 0xffff - i * 0x4000)),
    ];
    let b = gray(8, false, "PALETTE COLOR", vec![0, 3], extra, EXPLICIT);
    let im = decode(&b).unwrap();
    assert_eq!((im.layout(), im.data()), (ChannelLayout::Rgb, &[0u8, 0, 255, 192, 0, 63][..]));
}

#[test]
fn rle_lossless_segments_rebuild_the_pixels() {
    // 4 pixels of 16-bit gray (0x0102, 0x0102, 0x0304, 0x0506) as two PackBits segments, high
    // bytes first. PackBits: 0xFF repeats the next byte twice, 0x01 introduces two literals.
    // High bytes 01 01 03 05.
    let hi = vec![0xffu8, 0x01, 0x01, 0x03, 0x05];
    // Low bytes 02 02 04 06.
    let lo = vec![0xffu8, 0x02, 0x01, 0x04, 0x06];
    let mut rle = Vec::new();
    rle.extend(2u32.to_le_bytes());
    rle.extend(64u32.to_le_bytes());
    rle.extend((64 + hi.len() as u32).to_le_bytes());
    rle.resize(64, 0);
    rle.extend(&hi);
    rle.extend(&lo);
    if rle.len() % 2 == 1 {
        rle.push(0);
    }
    let mut b = build(&Spec { transfer: "1.2.840.10008.1.2.5", photometric: "MONOCHROME2", spp: 1, rows: 1, cols: 4, bits: 16, signed: false, extra: vec![], pixels: vec![], pixel_vr: *b"OB" });
    // Replace the empty native pixel data element with an encapsulated one.
    let cut = b.len() - 12;
    b.truncate(cut);
    b.extend([0xe0, 0x7f, 0x10, 0x00, b'O', b'B', 0, 0]);
    b.extend(0xffff_ffffu32.to_le_bytes());
    b.extend([0xfe, 0xff, 0x00, 0xe0, 0, 0, 0, 0]); // empty basic offset table
    b.extend([0xfe, 0xff, 0x00, 0xe0]);
    b.extend((rle.len() as u32).to_le_bytes());
    b.extend(&rle);
    b.extend([0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0]);
    let im = decode(&b).unwrap();
    assert_eq!(im.to_u16_samples().unwrap(), [0x0102, 0x0102, 0x0304, 0x0506]);
}

#[test]
fn hostile_dicom_errors() {
    let ok = gray(8, false, "MONOCHROME2", vec![1, 2, 3, 4], vec![], EXPLICIT);
    for cut in (0..ok.len()).step_by(7) {
        let _ = decode_as(Format::Dicom, &ok[..cut]);
    }
    let mut huge = ok.clone();
    let at = huge.windows(4).position(|w| w == [0x28, 0x00, 0x11, 0x00]).unwrap();
    huge[at + 8] = 0xff;
    huge[at + 9] = 0xff;
    assert!(decode_as(Format::Dicom, &huge).is_err(), "pixel data shorter than the declared size");
}
