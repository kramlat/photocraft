//! Read-only legacy formats (PlayStation TIM, X11 cursors, FLI/FLC), from hand-built files.

use photocraft_codecs::*;

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}
fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// A 15-bit PlayStation colour.
fn c15(r: u16, g: u16, b: u16) -> u16 {
    r | g << 5 | b << 10
}

fn tim(mode: u8, clut: Option<Vec<u16>>, words: u16, h: u16, data: &[u8]) -> Vec<u8> {
    let mut v = vec![0x10, 0, 0, 0, mode | if clut.is_some() { 8 } else { 0 }, 0, 0, 0];
    if let Some(c) = clut {
        v.extend(le32(12 + c.len() as u32 * 2));
        v.extend([0, 0, 0, 0]);
        v.extend(le16(c.len() as u16));
        v.extend(le16(1));
        c.iter().for_each(|x| v.extend(le16(*x)));
    }
    v.extend(le32(12 + data.len() as u32));
    v.extend([0, 0, 0, 0]);
    v.extend(le16(words));
    v.extend(le16(h));
    v.extend(data);
    v
}

#[test]
fn tim_reads_indexed_and_direct_colour() {
    // 4-bit: two pixels per byte, low nibble first.
    let clut = vec![c15(31, 0, 0), c15(0, 31, 0), c15(0, 0, 31), c15(31, 31, 31)];
    let f = tim(0, Some(clut.clone()), 1, 1, &[0x10, 0x32]);
    assert_eq!(detect(&f), Some(Format::Tim));
    assert_eq!(decode(&f).unwrap().data(), [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    // 8-bit.
    let f = tim(1, Some(clut), 1, 1, &[2, 1]);
    assert_eq!(decode(&f).unwrap().data(), [0, 0, 255, 0, 255, 0]);
    // 16-bit direct and 24-bit direct (3 pixels = 4.5 words: use 2 pixels = 3 words).
    let f = tim(2, None, 2, 1, &[le16(c15(31, 0, 0)), le16(c15(0, 0, 16))].concat());
    assert_eq!(decode(&f).unwrap().data(), [255, 0, 0, 0, 0, 131]);
    let f = tim(3, None, 3, 1, &[1, 2, 3, 4, 5, 6]);
    assert_eq!(decode(&f).unwrap().data(), [1, 2, 3, 4, 5, 6]);
}

fn xcursor(w: u32, h: u32, argb: &[u32]) -> Vec<u8> {
    let mut v = b"Xcur".to_vec();
    v.extend(le32(16));
    v.extend(le32(0x10000));
    v.extend(le32(1));
    v.extend(le32(0xfffd_0002));
    v.extend(le32(w));
    v.extend(le32(28));
    for x in [36, 0xfffd_0002, w, 1, w, h, 0, 0, 0] {
        v.extend(le32(x));
    }
    argb.iter().for_each(|p| v.extend(le32(*p)));
    v
}

#[test]
fn xcursor_unpremultiplies() {
    // Half-transparent premultiplied red (a=128, r=128) and an opaque green pixel.
    let f = xcursor(2, 1, &[0x8080_0000, 0xff00_ff00]);
    assert_eq!(detect(&f), Some(Format::Xcursor));
    let im = decode(&f).unwrap();
    assert_eq!(im.layout(), ChannelLayout::Rgba);
    assert_eq!(im.data(), [255, 0, 0, 128, 0, 255, 0, 255]);
}

fn fli_chunk(ty: u16, data: &[u8]) -> Vec<u8> {
    let mut v = le32(6 + data.len() as u32).to_vec();
    v.extend(le16(ty));
    v.extend(data);
    v
}

fn fli(w: u16, h: u16, chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut frame = le32(16 + body.len() as u32).to_vec();
    frame.extend(le16(0xf1fa));
    frame.extend(le16(chunks.len() as u16));
    frame.extend([0u8; 8]);
    frame.extend(body);
    let mut v = le32(128 + frame.len() as u32).to_vec();
    v.extend(le16(0xaf11));
    v.extend(le16(1));
    v.extend(le16(w));
    v.extend(le16(h));
    v.extend(le16(8));
    v.resize(128, 0);
    v.extend(frame);
    v
}

#[test]
fn fli_first_frame_with_palette_and_brun() {
    // COLOR_64: one packet (skip 0, 4 colours), 6-bit values scaled by 4: red, green, blue, white.
    let pal = [&le16(1)[..], &[0, 4], &[63, 0, 0, 0, 63, 0, 0, 0, 63, 63, 63, 63]].concat();
    // BRUN, 2 rows of 4 pixels (each row starts with a packet count byte): row 0 repeats index 1
    // four times; row 1 is 4 literals, 0 1 2 3.
    let brun = vec![1, 4, 1, 1, 252, 0, 1, 2, 3];
    let f = fli(4, 2, &[fli_chunk(11, &pal), fli_chunk(15, &brun)]);
    assert_eq!(detect(&f), Some(Format::Fli));
    let im = decode(&f).unwrap();
    assert_eq!((im.width(), im.height(), im.layout()), (4, 2, ChannelLayout::Rgb));
    assert_eq!(&im.data()[..12], &[0, 252, 0, 0, 252, 0, 0, 252, 0, 0, 252, 0]);
    assert_eq!(&im.data()[12..24], &[252, 0, 0, 0, 252, 0, 0, 0, 252, 252, 252, 252]);
}

#[test]
fn hostile_retro_files_error() {
    let samples = [tim(1, Some(vec![0; 4]), 1, 1, &[1, 2]), xcursor(2, 1, &[1, 2]), fli(2, 1, &[fli_chunk(16, &[1, 2])])];
    for s in samples {
        for cut in 0..s.len() {
            let _ = decode_as(detect(&s).unwrap(), &s[..cut]);
        }
        for i in 0..s.len().min(64) {
            let mut b = s.clone();
            b[i] = 0xff;
            let _ = decode_as(detect(&s).unwrap(), &b);
        }
    }
}

#[test]
fn fli_palette_values_beyond_six_bits_do_not_overflow() {
    let pal = [&le16(1)[..], &[0, 1], &[255, 255, 255]].concat();
    let f = fli(1, 1, &[fli_chunk(11, &pal), fli_chunk(16, &[0])]);
    assert_eq!(decode(&f).unwrap().data(), [255]);
}

#[test]
fn text_exports_describe_the_pixels() {
    let img = Image::from_u8(3, 2, ChannelLayout::Rgb, vec![255, 0, 0, 255, 0, 0, 0, 0, 255, 10, 20, 30, 40, 50, 60, 70, 80, 90]).unwrap();
    for f in [Format::CSource, Format::HtmlTable, Format::AsciiArt] {
        assert!(caps(f).write && !caps(f).read, "{f:?}");
        assert_eq!(detect(b"static const struct"), None);
        assert!(decode_as(f, b"x").is_err());
    }
    let c = String::from_utf8(encode(&img, Format::CSource, &Default::default()).unwrap()).unwrap();
    assert!(c.contains("3, 2, 3,") && c.contains("255, 0, 0, 255, 0, 0, 0, 0, 255,") && c.contains("pixel_data[18 + 1]"), "{c}");
    let html = String::from_utf8(encode(&img, Format::HtmlTable, &Default::default()).unwrap()).unwrap();
    assert!(html.contains("colspan=\"2\" bgcolor=\"#ff0000\"") && html.contains("bgcolor=\"#0000ff\"") && html.matches("<tr>").count() == 2, "{html}");
    // Alpha: transparent pixels get no background.
    let a = Image::from_u8(2, 1, ChannelLayout::Rgba, vec![1, 2, 3, 255, 9, 9, 9, 0]).unwrap();
    let html = String::from_utf8(encode(&a, Format::HtmlTable, &Default::default()).unwrap()).unwrap();
    assert!(html.contains("bgcolor=\"#010203\"") && html.contains("<td width=\"1\" height=\"1\"></td>"), "{html}");
    // ASCII art: black is a space, white is '@', and wide pictures are cut to 120 columns.
    let bw = Image::from_u8(4, 2, ChannelLayout::Gray, vec![0, 0, 255, 255, 0, 0, 255, 255]).unwrap();
    assert_eq!(encode(&bw, Format::AsciiArt, &Default::default()).unwrap(), b"  @@\n");
    let wide = Image::new(400, 100, ChannelLayout::Gray, SampleType::U8).unwrap();
    let art = encode(&wide, Format::AsciiArt, &Default::default()).unwrap();
    assert_eq!(art.split(|c| *c == b'\n').next().unwrap().len(), 120);
}

#[test]
fn mpo_and_jps_open_as_jpeg() {
    assert_eq!(from_extension("photo.MPO"), Some(Format::Jpeg));
    assert_eq!(from_extension("stereo.jps"), Some(Format::Jpeg));
}
