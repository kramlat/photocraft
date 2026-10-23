//! Brush thumbnails. MyPaint packs carry one per brush: `<name>_prev.png` beside `<name>.myb`
//! (128×128, hand-made art, the brush's name often drawn in). The `.myb` itself holds nothing
//! about it, and libmypaint has no notion of previews. A brush without a file gets a stroke drawn
//! by the engine instead.

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_geom::Rect;
use photocraft_paint::StrokePoint;
use photocraft_paint::mypaint::tables::set;
use photocraft_paint::mypaint::{Brush, TileSurface};
use photocraft_raster::Surface;

/// A thumbnail: straight RGBA, 8 bits.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Size of the thumbnails the engine draws (what MyPaint's own are).
pub const SIZE: i32 = 128;

/// A pack's `_prev.png` (any size up to 1024 px, so a stray huge file can't take memory).
pub fn decode(bytes: &[u8]) -> Option<Preview> {
    if bytes.len() > 4 << 20 {
        return None;
    }
    let img = photocraft_codecs::decode(bytes).ok()?;
    let (width, height) = (img.width(), img.height());
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return None;
    }
    Some(Preview { width, height, rgba: img.to_rgba8() })
}

/// Draws the brush's own stroke: an S curve whose pressure swells and fades, in dark ink on pale
/// paper, with the brush shrunk to fit if its own size is large.
pub fn render(text: &str) -> Option<Preview> {
    let mut brush = Brush::from_json(text).ok()?;
    let own = 2.0 * brush.base_value(set::RADIUS_LOGARITHMIC).exp();
    brush.set_base_value(set::RADIUS_LOGARITHMIC, (own.clamp(3.0, 24.0) / 2.0).ln());
    brush.set_color([0.12, 0.12, 0.16]);
    brush.seed(1);
    brush.reset();
    let fmt = PixelFormat::new(ColorMode::Rgb, SampleType::U8, true);
    let bounds = Rect::new(0, 0, SIZE, SIZE);
    let mut surf = Surface::new(fmt);
    surf.fill_rect(bounds, &[0.96, 0.96, 0.95, 1.0]);
    let mut tiles = TileSurface::new(surf.clone(), bounds);
    let mut prev = None;
    let steps = 90;
    for i in 0..=steps {
        let t = f64::from(i) / f64::from(steps);
        let x = 14.0 + 100.0 * t;
        let y = 64.0 + 30.0 * (t * std::f64::consts::TAU).sin();
        let pressure = (0.15 + 0.85 * (t * std::f64::consts::PI).sin()) as f32;
        let mut sp = StrokePoint::new(x, y, pressure);
        sp.time = f64::from(i) * 8.0;
        brush.stroke_to(&mut tiles, super::event_for(&mut prev, &sp, 1.0), false);
    }
    let dirty = tiles.take_dirty();
    tiles.write_into(&mut surf, dirty, None, false);
    let data = surf.read_region(bounds);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let rgba = data.as_chunks::<4>().0.iter().flat_map(|p| [q(p[0]), q(p[1]), q(p[2]), 255]).collect();
    Some(Preview { width: SIZE as u32, height: SIZE as u32, rgba })
}
