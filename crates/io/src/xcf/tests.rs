use photocraft_color::{BlendMode, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerContent, LayerMask};
use photocraft_geom::{Rect, Size};
use photocraft_raster::{Interrupt, Surface};

use super::*;

fn import_bytes(bytes: &[u8]) -> Result<ImportResult> {
    import("t.xcf", bytes, &Interrupt::NONE)
}

fn raster(name: &str, rect: Rect, color: [f32; 4]) -> Layer {
    let mut s = Surface::new(PixelFormat::RGBA8);
    let px: Vec<f32> = (0..rect.width() * rect.height()).flat_map(|_| color).collect();
    s.write_region(rect, &px);
    Layer::new(name, LayerContent::Raster(s))
}

fn sample_doc() -> Document {
    let mut doc = Document::new("sample", Size::new(100, 80), ColorMode::Rgb, SampleType::U8);
    let mut bottom = raster("Bottom", Rect::new(0, 0, 100, 80), [0.2, 0.4, 0.6, 1.0]);
    bottom.locks.transparency = true;
    let mut top = raster("Top", Rect::new(10, 20, 75, 50), [1.0, 0.0, 0.0, 0.5]);
    top.opacity = 0.5;
    top.blend = BlendMode::Multiply;
    top.visible = false;
    let mut masked = raster("Masked", Rect::new(0, 0, 70, 70), [0.0, 1.0, 0.0, 1.0]);
    let mut m = LayerMask::reveal_all();
    let mut ms = Surface::new(PixelFormat::GRAY8);
    ms.write_region(Rect::new(0, 0, 70, 70), &vec![0.0; 70 * 70]);
    ms.write_region(Rect::new(0, 0, 35, 70), &vec![1.0; 35 * 70]);
    m.surface = ms;
    masked.mask = Some(m);
    let inner = raster("Inner", Rect::new(5, 5, 30, 30), [1.0, 1.0, 0.0, 1.0]);
    let group = Layer::group("Group", vec![inner]);
    doc.layers = vec![bottom, top, masked, group];
    doc.resolution_dpi = 150.0;
    doc.guides.horizontal.push(12.0);
    doc.guides.vertical.push(40.0);
    doc
}

fn pixels(l: &Layer, r: Rect) -> Vec<f32> {
    l.surface().map(|s| s.read_region(r)).unwrap_or_default()
}

#[test]
fn run_length_coding_round_trips_every_shape_of_data() {
    let mut cases: Vec<Vec<u8>> = vec![vec![], vec![7], vec![1, 2, 3, 4, 5], vec![9; 5], vec![9; 127], vec![9; 128], vec![9; 3000], (0..=255).collect()];
    let mut x = 12345u32;
    cases.push((0..4096).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect());
    cases.push((0..4096).map(|i| if i % 7 < 3 { 5 } else { (i % 251) as u8 }).collect());
    for plane in cases {
        let mut enc = Vec::new();
        rle_plane(&plane, &mut enc);
        if plane.is_empty() {
            continue;
        }
        let dec = rle_tile(&enc, plane.len(), 1).expect("decodes");
        assert_eq!(dec, plane, "len {}", plane.len());
        // Several planes interleave.
        let mut two = Vec::new();
        rle_plane(&plane, &mut two);
        rle_plane(&plane.iter().map(|b| b ^ 0xff).collect::<Vec<_>>(), &mut two);
        let dec = rle_tile(&two, plane.len(), 2).expect("decodes two planes");
        assert!(dec.chunks(2).zip(&plane).all(|(p, b)| p[0] == *b && p[1] == !*b));
    }
}

#[test]
fn an_exported_document_imports_with_its_layers_and_settings() {
    let doc = sample_doc();
    let out = export(&doc).unwrap();
    assert!(out.bytes.starts_with(b"gimp xcf v011\0"));
    let back = import_bytes(&out.bytes).unwrap();
    let d = back.document;
    assert_eq!((d.size.width, d.size.height, d.resolution_dpi), (100, 80, 150.0));
    assert_eq!((d.guides.horizontal.clone(), d.guides.vertical.clone()), (vec![12.0], vec![40.0]));
    let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Bottom", "Top", "Masked", "Group"], "bottom to top, as in the original");
    let (bottom, top, masked, group) = (&d.layers[0], &d.layers[1], &d.layers[2], &d.layers[3]);
    assert!(bottom.locks.transparency && bottom.visible && bottom.opacity == 1.0);
    assert!(!top.visible && top.blend == BlendMode::Multiply && (top.opacity - 0.5).abs() < 0.01);
    let r = Rect::new(10, 20, 75, 50);
    let want = pixels(&doc.layers[1], r);
    let got = pixels(top, r);
    assert!(want.iter().zip(&got).all(|(a, b)| (a - b).abs() < 0.01), "pixels survive at their position");
    assert_eq!(group.children().map(|c| c.len()), Some(1));
    assert_eq!(group.children().unwrap()[0].name, "Inner");
    let m = masked.mask.as_ref().expect("mask kept");
    assert!(m.surface.sample_channel(10, 10, 0) > 0.9 && m.surface.sample_channel(50, 10, 0) < 0.1);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn a_grayscale_document_stays_grayscale() {
    let mut doc = Document::new("g", Size::new(8, 8), ColorMode::Grayscale, SampleType::U8);
    let mut s = Surface::new(PixelFormat::new(ColorMode::Grayscale, SampleType::U8, true));
    s.write_region(Rect::new(0, 0, 8, 8), &(0..64).flat_map(|i| [i as f32 / 63.0, 1.0]).collect::<Vec<_>>());
    doc.layers = vec![Layer::new("Gray", LayerContent::Raster(s))];
    let back = import_bytes(&export(&doc).unwrap().bytes).unwrap().document;
    assert_eq!(back.mode, ColorMode::Grayscale);
    let px = pixels(&back.layers[0], Rect::new(0, 0, 8, 8));
    assert!((px[2 * 63] - 1.0).abs() < 0.01 && px[0] < 0.01);
}

#[test]
fn export_says_what_it_cannot_keep() {
    let mut doc = sample_doc();
    doc.layers[1].blend = BlendMode::Reflect;
    doc.depth = SampleType::U16;
    let w = export(&doc).unwrap().warnings.join("\n");
    assert!(w.contains("8-bit") && w.contains("no GIMP equivalent"), "{w}");
    let lost = losses(&doc);
    assert!(!lost.is_empty());
    let mut cmyk = Document::new("c", Size::new(4, 4), ColorMode::Cmyk, SampleType::U8);
    cmyk.layers = vec![];
    assert!(export(&cmyk).is_err());
}

#[test]
fn damaged_files_are_errors_never_panics() {
    let good = export(&sample_doc()).unwrap().bytes;
    assert!(import_bytes(b"not xcf").is_err());
    assert!(import_bytes(b"gimp xcf v999\0").is_err());
    for cut in (0..good.len()).step_by(37) {
        let _ = import_bytes(&good[..cut]);
    }
    let mut x = 99u32;
    for _ in 0..300 {
        let mut b = good.clone();
        for _ in 0..4 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let at = (x as usize >> 3) % b.len();
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            b[at] = (x >> 24) as u8;
        }
        let _ = import_bytes(&b);
    }
    // A huge declared canvas is refused before anything is allocated.
    let mut huge = good.clone();
    huge[14..18].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(import_bytes(&huge).is_err());
}

#[test]
fn old_style_headers_and_pointer_sizes_are_read() {
    // Version 0 ("file"): 4-byte offsets, 8-bit, no precision field, one 2×1 RGB layer, uncompressed.
    let mut b: Vec<u8> = b"gimp xcf file\0".to_vec();
    let be = |v: u32| v.to_be_bytes();
    b.extend(be(2)); // width
    b.extend(be(1)); // height
    b.extend(be(0)); // RGB
    b.extend(be(0)); // no image properties (PROP_END)
    b.extend(be(0));
    let table = b.len();
    b.extend(be(0)); // layer pointer (patched)
    b.extend(be(0)); // end of layers
    b.extend(be(0)); // end of channels
    let layer = b.len();
    b[table..table + 4].copy_from_slice(&be(layer as u32));
    b.extend(be(2));
    b.extend(be(1));
    b.extend(be(0)); // RGB_IMAGE
    b.extend(be(2)); // "L\0"
    b.extend(b"L\0");
    b.extend(be(0));
    b.extend(be(0)); // props end
    let hier_ptr = b.len();
    b.extend(be(0));
    b.extend(be(0)); // no mask
    let hier = b.len();
    b[hier_ptr..hier_ptr + 4].copy_from_slice(&be(hier as u32));
    b.extend(be(2));
    b.extend(be(1));
    b.extend(be(3));
    let level_ptr = b.len();
    b.extend(be(0));
    b.extend(be(0));
    let level = b.len();
    b[level_ptr..level_ptr + 4].copy_from_slice(&be(level as u32));
    b.extend(be(2));
    b.extend(be(1));
    let tile_ptr = b.len();
    b.extend(be(0));
    b.extend(be(0));
    let tile = b.len();
    b[tile_ptr..tile_ptr + 4].copy_from_slice(&be(tile as u32));
    b.extend([255, 0, 0, 0, 0, 255]); // uncompressed: red, blue
    // Compression defaults to none when the property is absent.
    let d = import_bytes(&b).unwrap().document;
    assert_eq!(d.layers.len(), 1);
    let px = pixels(&d.layers[0], Rect::new(0, 0, 2, 1));
    assert_eq!(px, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0]);
}
