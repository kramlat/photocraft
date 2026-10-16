
fn tree(layers: &[photocraft_doc::Layer], depth: usize) {
    for l in layers.iter().rev() {
        println!("{}{} [{}] vis={} op={:.2} blend={:?} mask={}", "  ".repeat(depth), l.name, l.content.kind_name(), l.visible, l.opacity, l.blend, l.mask.is_some());
        if let Some(c) = l.children() {
            tree(c, depth + 1);
        }
    }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let t = std::time::Instant::now();
    let r = match photocraft_io::import(&args[1], &bytes) {
        Ok(r) => r,
        Err(e) => {
            println!("ERROR {e}");
            return;
        }
    };
    println!("imported in {:?}: {}x{} {:?} {:?}, {} top layers", t.elapsed(), r.document.size.width, r.document.size.height, r.document.mode, r.document.depth, r.document.layers.len());
    for w in &r.warnings {
        println!("warning: {w}");
    }
    tree(&r.document.layers, 1);
    if let Some(png) = args.get(2) {
        let flat = photocraft_compose::flatten(&r.document);
        let mut rgba = Vec::new();
        for p in &flat.px {
            for c in p {
                rgba.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        let img = photocraft_codecs::Image::from_raw(flat.rect.width(), flat.rect.height(), photocraft_codecs::ChannelLayout::Rgba, photocraft_codecs::SampleType::U8, rgba).unwrap();
        std::fs::write(png, photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap()).unwrap();
    }
    if let Some(out) = args.get(3) {
        let e = photocraft_io::export(&r.document, out, &Default::default()).unwrap();
        for w in &e.warnings {
            println!("export warning: {w}");
        }
        std::fs::write(out, e.bytes).unwrap();
        println!("wrote {out}");
    }
}
