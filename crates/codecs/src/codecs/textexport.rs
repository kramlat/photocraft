//! Write-only text exports: a C source file holding the pixels (`.c`, `.h`), an HTML table with
//! one coloured cell per run of equal pixels (`.html`), and ASCII art (`.aa`, `.ansi`). None can
//! be read back: they are for embedding a picture in code, a web page or a terminal.

use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::image::{ChannelLayout, Image};
use crate::options::EncodeOptions;

/// A C source file with a `struct { width, height, bytes_per_pixel, pixel_data[] }` named
/// `image` holding the 8-bit pixels (gray, RGB or RGBA) as a byte array.
pub(crate) fn encode_csource(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h, bpp) = (img.width(), img.height(), img.layout().channels());
    let kind = match img.layout() {
        ChannelLayout::Gray => "Gray",
        ChannelLayout::Rgb => "RGB",
        _ => "RGBA",
    };
    let mut s = format!("/* PhotoCraft {kind} C source image dump */\n\nstatic const struct {{\n  unsigned int  width;\n  unsigned int  height;\n  unsigned int  bytes_per_pixel; /* 1 = gray, 3 = RGB, 4 = RGBA */\n  unsigned char pixel_data[{} + 1];\n}} image = {{\n  {w}, {h}, {bpp},\n  {{\n", w as usize * h as usize * bpp);
    for row in img.data().chunks(w as usize * bpp) {
        for chunk in row.chunks(16) {
            s.push_str("    ");
            for v in chunk {
                s.push_str(&format!("{v}, "));
            }
            s.push('\n');
        }
    }
    s.push_str("    0\n  }\n};\n");
    Ok(s.into_bytes())
}

/// An HTML table, one row per pixel row; runs of equal opaque colours share a cell (`colspan`).
/// Transparent pixels become cells without a background.
pub(crate) fn encode_html(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, ch) = (img.width() as usize, img.layout().channels());
    let mut s = String::from("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>image</title></head><body>\n<table border=\"0\" cellspacing=\"0\" cellpadding=\"0\" style=\"border-collapse:collapse\">\n");
    let colour = |p: &[u8]| -> Option<String> {
        match ch {
            1 => Some(format!("#{0:02x}{0:02x}{0:02x}", p[0])),
            2 => (p[1] > 127).then(|| format!("#{0:02x}{0:02x}{0:02x}", p[0])),
            3 => Some(format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])),
            _ => (p[3] > 127).then(|| format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])),
        }
    };
    for row in img.data().chunks(w * ch) {
        s.push_str("<tr>");
        let px: Vec<Option<String>> = row.chunks(ch).map(colour).collect();
        let mut x = 0;
        while x < w {
            let mut run = 1;
            while x + run < w && px[x + run] == px[x] {
                run += 1;
            }
            let span = if run > 1 { format!(" colspan=\"{run}\"") } else { String::new() };
            match &px[x] {
                Some(c) => s.push_str(&format!("<td{span} bgcolor=\"{c}\" width=\"{run}\" height=\"1\"></td>")),
                None => s.push_str(&format!("<td{span} width=\"{run}\" height=\"1\"></td>")),
            }
            x += run;
        }
        s.push_str("</tr>\n");
    }
    s.push_str("</table>\n</body></html>\n");
    Ok(s.into_bytes())
}

/// ASCII art: at most 120 columns, rows halved for the height of a character cell, a ten-step
/// brightness ramp from space (dark) to `@` (bright).
pub(crate) fn encode_ascii(src: &Image, plan: Plan, _opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    const RAMP: &[u8] = b" .:-=+*#%@";
    let img = src.converted(plan.layout, plan.sample);
    let (w, h, ch) = (img.width() as usize, img.height() as usize, img.layout().channels());
    let cols = w.min(120);
    let rows = ((h * cols) as f64 / w as f64 / 2.0).round().max(1.0) as usize;
    let luma = |x: usize, y: usize| -> f64 {
        let p = &img.data()[(y * w + x) * ch..(y * w + x + 1) * ch];
        let (l, a) = match ch {
            1 => (f64::from(p[0]), 1.0),
            2 => (f64::from(p[0]), f64::from(p[1]) / 255.0),
            3 => (0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]), 1.0),
            _ => (0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]), f64::from(p[3]) / 255.0),
        };
        // Transparent areas read as dark paper.
        l * a
    };
    let mut out = Vec::with_capacity((cols + 1) * rows);
    for r in 0..rows {
        let (y0, y1) = (r * h / rows, ((r + 1) * h / rows).max(r * h / rows + 1).min(h));
        for c in 0..cols {
            let (x0, x1) = (c * w / cols, ((c + 1) * w / cols).max(c * w / cols + 1).min(w));
            let (mut sum, mut n) = (0.0, 0.0);
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += luma(x, y);
                    n += 1.0;
                }
            }
            let v = (sum / n / 255.0).clamp(0.0, 1.0);
            out.push(RAMP[((v * (RAMP.len() - 1) as f64).round()) as usize]);
        }
        out.push(b'\n');
    }
    Ok(out)
}

