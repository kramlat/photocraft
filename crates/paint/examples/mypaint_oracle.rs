//! Renders one stroke with the Rust MyPaint engine in the format of the libmypaint test harness
//! (premultiplied RGBA at 15 bits, u16 little-endian), so the two can be compared pixel by pixel.
//! usage: mypaint_oracle brush.myb W H r g b radius_log|- < points   (x y pressure dt xtilt ytilt)

use std::io::{Read, Write};

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_paint::mypaint::{Brush, Event, TileSurface, tables::set};
use photocraft_raster::Surface;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&a[1]).unwrap();
    let (w, h): (i32, i32) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let mut brush = Brush::from_json(&text).unwrap();
    brush.set_color([a[4].parse().unwrap(), a[5].parse().unwrap(), a[6].parse().unwrap()]);
    if a.len() > 7 && a[7] != "-" {
        brush.set_base_value(set::RADIUS_LOGARITHMIC, a[7].parse().unwrap());
    }
    let mut surf = TileSurface::new(Surface::new(PixelFormat::RGBA32F), Rect::new(0, 0, w, h));
    surf.exhaustive_sampling = std::env::var_os("MYP_FULL").is_some();
    // Optional prelude: another brush paints first (argv 8 brush, argv 9 points).
    if a.len() > 9 {
        let mut pre = Brush::from_json(&std::fs::read_to_string(&a[8]).unwrap()).unwrap();
        pre.set_base_value(set::COLOR_H, 0.08);
        pre.set_base_value(set::COLOR_S, 0.9);
        pre.set_base_value(set::COLOR_V, 0.9);
        pre.set_base_value(set::RADIUS_LOGARITHMIC, 2.4);
        pre.reset();
        let v: Vec<f64> = std::fs::read_to_string(&a[9]).unwrap().split_whitespace().map(|t| t.parse().unwrap()).collect();
        for c in v.as_chunks::<6>().0 {
            pre.stroke_to(&mut surf, Event { x: c[0] as f32, y: c[1] as f32, pressure: c[2] as f32, xtilt: c[4] as f32, ytilt: c[5] as f32, dtime: c[3], viewzoom: 1.0, viewrotation: 0.0, barrel_rotation: 0.0 }, false);
        }
    }
    surf.exhaustive_sampling = std::env::var_os("MYP_FULL").is_some();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    brush.reset();
    let v: Vec<f64> = input.split_whitespace().map(|t| t.parse().unwrap()).collect();
    for c in v.as_chunks::<6>().0 {
        brush.stroke_to(&mut surf, Event { x: c[0] as f32, y: c[1] as f32, pressure: c[2] as f32, xtilt: c[4] as f32, ytilt: c[5] as f32, dtime: c[3], viewzoom: 1.0, viewrotation: 0.0, barrel_rotation: 0.0 }, false);
    }
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let p = surf.pixel(x, y);
            for k in 0..3 {
                out.extend(((p[k] * p[3] * 32768.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes());
            }
            out.extend(((p[3] * 32768.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes());
        }
    }
    std::io::stdout().write_all(&out).unwrap();
}
