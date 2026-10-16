//! Row, bend and tile distortions plus Illusion: Shift Rows, Bend, Glass Tiles, Paper Tiles and
//! Illusion. All are inverse maps resampled with the shared bilinear sampler.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::distort::remap;
use crate::fxutil::{set_rgba, xy};
use crate::image::{Edge, Image};
use crate::noise::hash01;

/// The curve Bend follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BendCurve {
    /// A single arch.
    #[default]
    Arc,
    /// One full wave.
    Wave,
}

/// What Paper Tiles shows where the shifted tiles leave gaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TileGaps {
    #[default]
    Transparent,
    Black,
    White,
}

/// Shift Rows: every row (or column, when `vertical`) slides by its own random distance up to
/// `distance` px either way. The shift of a row depends only on its position and the seed.
pub(crate) fn shift_rows(src: &Image, out: Rect, ctx: &Ctx, vertical: bool, distance: f32, seed: u32) -> Vec<f32> {
    let d = distance.clamp(0.0, 10_000.0);
    remap(src, out, ctx, Edge::Repeat, |x, y| {
        let line = if vertical { x.floor() as i32 } else { y.floor() as i32 };
        let off = ((hash01(line, 0, 81, seed) * 2.0 - 1.0) * d).round();
        if vertical { (x, y - off) } else { (x - off, y) }
    })
}

/// How far Shift Rows reaches, for the tile margin.
pub(crate) fn shift_halo(distance: f32) -> i32 {
    distance.clamp(0.0, 10_000.0).ceil() as i32 + 1
}

/// Bend: the image is displaced along a curve across the reference area; `amount` % is the
/// peak displacement as a fraction of half the perpendicular side.
pub(crate) fn bend(src: &Image, out: Rect, ctx: &Ctx, amount: f32, vertical_axis: bool, curve: BendCurve) -> Vec<f32> {
    let b = ctx.bounds;
    let (w, h) = (b.width().max(1) as f32, b.height().max(1) as f32);
    let k = amount.clamp(-100.0, 100.0) / 100.0;
    remap(src, out, ctx, Edge::Repeat, |x, y| {
        if vertical_axis {
            // Columns move up and down along a curve over the width.
            let u = ((x - b.x0 as f32) / w).clamp(0.0, 1.0);
            let f = match curve {
                BendCurve::Arc => (std::f32::consts::PI * u).sin(),
                BendCurve::Wave => (std::f32::consts::TAU * u).sin(),
            };
            (x, y - k * h / 2.0 * f)
        } else {
            let u = ((y - b.y0 as f32) / h).clamp(0.0, 1.0);
            let f = match curve {
                BendCurve::Arc => (std::f32::consts::PI * u).sin(),
                BendCurve::Wave => (std::f32::consts::TAU * u).sin(),
            };
            (x - k * w / 2.0 * f, y)
        }
    })
}

/// Glass Tiles: every `size` px square shows the picture through its own lens: 0 % leaves it,
/// 50 % flattens it to the colour at the tile's centre, 100 % turns it upside down.
pub(crate) fn glass_tiles(src: &Image, out: Rect, ctx: &Ctx, size: f32, refraction: f32) -> Vec<f32> {
    let size = size.clamp(2.0, 2000.0);
    let factor = 1.0 - 2.0 * (refraction / 100.0).clamp(0.0, 1.0);
    let b = ctx.bounds;
    remap(src, out, ctx, Edge::Repeat, |x, y| {
        let (lx, ly) = (x - b.x0 as f32, y - b.y0 as f32);
        let (cx, cy) = (((lx / size).floor() + 0.5) * size, ((ly / size).floor() + 0.5) * size);
        (b.x0 as f32 + cx + (lx - cx) * factor, b.y0 as f32 + cy + (ly - cy) * factor)
    })
}

/// Paper Tiles: the picture is cut into `size` px squares, each shifted by up to `jitter` px
/// at random; later (random-order) tiles lie on top, and the gaps show `gaps`.
pub(crate) fn paper_tiles(src: &Image, out: Rect, ctx: &Ctx, size: f32, jitter: f32, gaps: TileGaps, seed: u32) -> Vec<f32> {
    let n = src.ch;
    let size = size.clamp(2.0, 2000.0).round() as i32;
    let jitter = jitter.clamp(0.0, size as f32);
    let b = ctx.bounds;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (lx, ly) = (x - b.x0, y - b.y0);
        let (tx, ty) = (lx.div_euclid(size), ly.div_euclid(size));
        let mut best: Option<(f32, i32, i32)> = None;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (cx, cy) = (tx + dx, ty + dy);
                let ox = ((hash01(cx, cy, 91, seed) * 2.0 - 1.0) * jitter).round() as i32;
                let oy = ((hash01(cx, cy, 92, seed) * 2.0 - 1.0) * jitter).round() as i32;
                let (x0, y0) = (cx * size + ox, cy * size + oy);
                if lx >= x0 && lx < x0 + size && ly >= y0 && ly < y0 + size {
                    let rank = hash01(cx, cy, 93, seed);
                    if best.is_none_or(|(r, ..)| rank > r) {
                        best = Some((rank, ox, oy));
                    }
                }
            }
        }
        match best {
            Some((_, ox, oy)) => {
                let (sx, sy) = (x - ox, y - oy);
                let (sx, sy) = (sx.clamp(src.rect.x0, src.rect.x1 - 1), sy.clamp(src.rect.y0, src.rect.y1 - 1));
                px.copy_from_slice(src.px(sx, sy));
            }
            None => match gaps {
                TileGaps::Transparent if ctx.alpha => px.iter_mut().for_each(|v| *v = 0.0),
                TileGaps::White => set_rgba(ctx, px, [1.0, 1.0, 1.0, 1.0]),
                _ => set_rgba(ctx, px, [0.0, 0.0, 0.0, 1.0]),
            },
        }
    }
    res
}

/// How far Paper Tiles reaches, for the tile margin.
pub(crate) fn paper_halo(jitter: f32, size: f32) -> i32 {
    jitter.clamp(0.0, size.clamp(2.0, 2000.0)).ceil() as i32 + 2
}

/// Illusion: `copies` turned copies of the picture, each rotated a further `360° / copies`
/// about the centre of the reference area, averaged into one.
pub(crate) fn illusion(src: &Image, out: Rect, ctx: &Ctx, copies: u32, center: (f32, f32)) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let copies = copies.clamp(2, 32);
    let turns: Vec<(f32, f32)> = (0..copies).map(|k| (std::f32::consts::TAU * k as f32 / copies as f32).sin_cos()).collect();
    let mut res = src.crop(out);
    let mut tmp = vec![0.0f32; n];
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let mut acc = vec![0.0f32; n];
        let mut weight = 0.0f32;
        for (s, c) in &turns {
            src.sample(cx + dx * c - dy * s, cy + dx * s + dy * c, Edge::Repeat, b, ctx.alpha, &mut tmp);
            let a = if ctx.alpha { tmp[n - 1] } else { 1.0 };
            let colours = if ctx.alpha { n - 1 } else { n };
            for k in 0..colours {
                acc[k] += tmp[k] * a;
            }
            weight += a;
        }
        let colours = if ctx.alpha { n - 1 } else { n };
        if weight > 0.0 {
            for k in 0..colours {
                px[k] = acc[k] / weight;
            }
        }
        if ctx.alpha {
            px[n - 1] = weight / copies as f32;
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    /// A `w × h` image whose red channel counts x and green counts y (0..1), opaque.
    fn ramp(w: i32, h: i32) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        let mut i = Image::new(rect, 4);
        for y in 0..h {
            for x in 0..w {
                let k = ((y * w + x) * 4) as usize;
                i.data[k..k + 4].copy_from_slice(&[x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32, 0.5, 1.0]);
            }
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    #[test]
    fn shifted_rows_keep_their_content_and_differ_from_each_other() {
        let (i, ctx) = ramp(40, 20);
        let o = shift_rows(&i, i.rect, &ctx, false, 6.0, 3);
        for y in 0..20usize {
            // Within a row the green channel (y) is constant, and the red ramp is just slid.
            let row: Vec<f32> = (0..40).map(|x| o[(y * 40 + x) * 4]).collect();
            assert!(row.windows(2).all(|w| w[1] >= w[0] - 1e-6), "row {y} still rises");
        }
        let offsets: std::collections::BTreeSet<i32> = (0..20usize).map(|y| (o[(y * 40 + 20) * 4] * 39.0).round() as i32).collect();
        assert!(offsets.len() > 3, "rows shift by different amounts: {offsets:?}");
        assert_eq!(shift_rows(&i, i.rect, &ctx, false, 0.0, 3), i.data);
        assert_eq!(o, shift_rows(&i, i.rect, &ctx, false, 6.0, 3));
    }

    #[test]
    fn bend_moves_the_middle_most_and_not_the_ends() {
        let (i, ctx) = ramp(41, 41);
        let o = bend(&i, i.rect, &ctx, 50.0, true, BendCurve::Arc);
        let green = |x: usize, y: usize| o[(y * 41 + x) * 4 + 1];
        assert!((green(0, 20) - i.data[(20 * 41) * 4 + 1]).abs() < 0.05, "the edge barely moves");
        assert!((green(20, 20) - 20.0 / 40.0).abs() > 0.15, "the middle is displaced");
        assert_eq!(bend(&i, i.rect, &ctx, 0.0, true, BendCurve::Wave), i.data);
    }

    #[test]
    fn glass_tiles_flip_each_tile_at_full_refraction() {
        let (i, ctx) = ramp(32, 32);
        let flipped = glass_tiles(&i, i.rect, &ctx, 16.0, 100.0);
        // Pixel (2, 3) of the first tile shows what sits at the opposite corner of that tile.
        let red = |o: &[f32], x: usize, y: usize| o[(y * 32 + x) * 4];
        assert!((red(&flipped, 2, 3) - red(&i.data, 13, 12)).abs() < 0.04, "{} vs {}", red(&flipped, 2, 3), red(&i.data, 13, 12));
        assert_eq!(glass_tiles(&i, i.rect, &ctx, 16.0, 0.0), i.data);
    }

    #[test]
    fn paper_tiles_leave_gaps_and_keep_whole_tiles_of_the_picture() {
        let (i, ctx) = ramp(48, 48);
        let o = paper_tiles(&i, i.rect, &ctx, 12.0, 4.0, TileGaps::Transparent, 1);
        let gaps = o.chunks(4).filter(|p| p[3] == 0.0).count();
        assert!(gaps > 20 && gaps < 1000, "{gaps} transparent pixels");
        // Wherever a pixel is shown, it is a pixel of the picture (never an invented colour).
        assert!(o.chunks(4).filter(|p| p[3] > 0.0).all(|p| (p[2] - 0.5).abs() < 1e-6));
        let white = paper_tiles(&i, i.rect, &ctx, 12.0, 4.0, TileGaps::White, 1);
        assert!(white.chunks(4).all(|p| p[3] == 1.0));
        assert_eq!(paper_tiles(&i, i.rect, &ctx, 12.0, 0.0, TileGaps::Black, 1), i.data, "no jitter, no change");
    }

    #[test]
    fn illusion_averages_rotated_copies_symmetrically() {
        let (i, ctx) = ramp(33, 33);
        let o = illusion(&i, i.rect, &ctx, 4, (0.5, 0.5));
        let at = |x: usize, y: usize| (o[(y * 33 + x) * 4], o[(y * 33 + x) * 4 + 1]);
        // Four-fold symmetry: a quarter turn about the centre maps the result onto itself.
        let (a, b) = (at(8, 4), at(28, 8));
        assert!((a.0 - b.0).abs() < 0.03 && (a.1 - b.1).abs() < 0.03, "{a:?} vs {b:?}");
        assert!(o.chunks(4).all(|p| (p[3] - 1.0).abs() < 1e-5));
    }
}
