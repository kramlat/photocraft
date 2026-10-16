//! Render and effect filters: Noise Pattern, Checkerboard, Grid, Kaleidoscope, Long Shadow and
//! Vignette. Colours are straight sRGB RGBA; every filter works in the image's own model and depth.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::distort::{centre, remap};
use crate::fxutil::{rgba, set_rgba, smoothstep, xy};
use crate::image::{Edge, Image};
use crate::noise::hash01;

/// The kind of noise Noise Pattern renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum NoiseKind {
    /// Smooth gradient noise (Perlin).
    #[default]
    Perlin,
    /// Simplex noise: fewer directional artefacts than Perlin.
    Simplex,
    /// Distance to the nearest of a scattering of points (Worley): cells and cracks.
    Cell,
    /// Smoothly interpolated random values.
    Value,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// A unit gradient vector for lattice point `(ix, iy)`.
fn gradient(ix: i32, iy: i32, seed: u32) -> (f32, f32) {
    let a = hash01(ix, iy, 7, seed) * std::f32::consts::TAU;
    (a.cos(), a.sin())
}

/// Perlin gradient noise, about `-0.7..0.7`.
fn perlin(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let corner = |dx: i32, dy: i32| {
        let (gx, gy) = gradient(ix + dx, iy + dy, seed);
        gx * (fx - dx as f32) + gy * (fy - dy as f32)
    };
    let (u, v) = (fade(fx), fade(fy));
    lerp(lerp(corner(0, 0), corner(1, 0), u), lerp(corner(0, 1), corner(1, 1), u), v)
}

/// 2-D simplex noise, about `-1..1`.
fn simplex(x: f32, y: f32, seed: u32) -> f32 {
    const F2: f32 = 0.366_025_4; // (sqrt(3) - 1) / 2
    const G2: f32 = 0.211_324_87; // (3 - sqrt(3)) / 6
    let s = (x + y) * F2;
    let (i, j) = ((x + s).floor(), (y + s).floor());
    let t = (i + j) * G2;
    let (x0, y0) = (x - (i - t), y - (j - t));
    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let (x1, y1) = (x0 - i1 as f32 + G2, y0 - j1 as f32 + G2);
    let (x2, y2) = (x0 - 1.0 + 2.0 * G2, y0 - 1.0 + 2.0 * G2);
    let (ii, jj) = (i as i32, j as i32);
    let contrib = |dx: f32, dy: f32, gi: i32, gj: i32| {
        let t = 0.5 - dx * dx - dy * dy;
        if t <= 0.0 {
            return 0.0;
        }
        let (gx, gy) = gradient(gi, gj, seed);
        let t2 = t * t;
        t2 * t2 * (gx * dx + gy * dy)
    };
    70.0 * (contrib(x0, y0, ii, jj) + contrib(x1, y1, ii + i1, jj + j1) + contrib(x2, y2, ii + 1, jj + 1))
}

/// Distance to the nearest feature point, about `0..1`.
fn cell(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (ix, iy) = (x0 as i32, y0 as i32);
    let mut best = f32::MAX;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix + dx, iy + dy);
            let px = cx as f32 + hash01(cx, cy, 1, seed);
            let py = cy as f32 + hash01(cx, cy, 2, seed);
            let d = (px - x) * (px - x) + (py - y) * (py - y);
            best = best.min(d);
        }
    }
    best.sqrt().min(1.0)
}

fn value(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (u, v) = (fade(x - x0), fade(y - y0));
    let h = |dx: i32, dy: i32| hash01(ix + dx, iy + dy, 3, seed);
    lerp(lerp(h(0, 0), h(1, 0), u), lerp(h(0, 1), h(1, 1), u), v)
}

/// One octave of `kind` at lattice position `(x, y)`, normalised to `0..=1`.
pub(crate) fn octave(kind: NoiseKind, x: f32, y: f32, seed: u32) -> f32 {
    match kind {
        NoiseKind::Perlin => (0.5 + perlin(x, y, seed) * 0.9).clamp(0.0, 1.0),
        NoiseKind::Simplex => (0.5 + simplex(x, y, seed) * 0.5).clamp(0.0, 1.0),
        NoiseKind::Cell => cell(x, y, seed),
        NoiseKind::Value => value(x, y, seed),
    }
}

/// Noise Pattern: fractal noise of `kind` (`detail` octaves, features about `size` px wide),
/// mapped from the background colour (0) to the foreground colour (1).
#[allow(clippy::too_many_arguments)]
pub(crate) fn noise_pattern(src: &Image, out: Rect, ctx: &Ctx, kind: NoiseKind, size: f32, detail: u32, seed: u32, colours: ([f32; 4], [f32; 4])) -> Vec<f32> {
    let (fg, bg) = colours;
    let n = src.ch;
    let size = size.clamp(1.0, 10_000.0);
    let detail = detail.clamp(1, 8);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (mut sum, mut norm, mut amp, mut freq) = (0.0, 0.0, 1.0, 1.0 / size);
        for o in 0..detail {
            sum += octave(kind, x as f32 * freq, y as f32 * freq, seed.wrapping_add(o * 7919)) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        let t = (sum / norm).clamp(0.0, 1.0);
        set_rgba(ctx, px, [lerp(bg[0], fg[0], t), lerp(bg[1], fg[1], t), lerp(bg[2], fg[2], t), 1.0]);
    }
    res
}

/// Checkerboard of `size`-pixel squares anchored at the reference bounds' top-left corner:
/// the foreground colour on the first square.
pub(crate) fn checkerboard(src: &Image, out: Rect, ctx: &Ctx, size: f32, colours: ([f32; 4], [f32; 4])) -> Vec<f32> {
    let (fg, bg) = colours;
    let n = src.ch;
    let size = size.clamp(1.0, 10_000.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (cx, cy) = (((x - ctx.bounds.x0) as f32 / size).floor() as i64, ((y - ctx.bounds.y0) as f32 / size).floor() as i64);
        let c = if (cx + cy) & 1 == 0 { fg } else { bg };
        set_rgba(ctx, px, [c[0], c[1], c[2], 1.0]);
    }
    res
}

/// Grid lines `line_width` px thick every `spacing` px, in `color`, over the image.
pub(crate) fn grid(src: &Image, out: Rect, ctx: &Ctx, spacing: f32, line_width: f32, color: [f32; 4]) -> Vec<f32> {
    let n = src.ch;
    let spacing = spacing.clamp(2.0, 10_000.0);
    let width = line_width.clamp(1.0, spacing);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let along = |v: i32, o: i32| ((v - o) as f32).rem_euclid(spacing);
        if along(x, ctx.bounds.x0) < width || along(y, ctx.bounds.y0) < width {
            let under = rgba(ctx, px);
            set_rgba(ctx, px, over(color, under));
        }
    }
    res
}

/// `top` composited over `under` (both straight RGBA).
fn over(top: [f32; 4], under: [f32; 4]) -> [f32; 4] {
    let a = top[3] + under[3] * (1.0 - top[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mix = |t: f32, u: f32| (t * top[3] + u * under[3] * (1.0 - top[3])) / a;
    [mix(top[0], under[0]), mix(top[1], under[1]), mix(top[2], under[2]), a]
}

/// Kaleidoscope: the wedge of the picture starting at `angle` degrees (one of `segments`
/// equal wedges) repeated all the way round the centre, so the circle holds `segments`
/// copies of it. With `mirror` every other copy is flipped, which joins them seamlessly.
pub(crate) fn kaleidoscope(src: &Image, out: Rect, ctx: &Ctx, segments: u32, angle: f32, center: (f32, f32), mirror: bool) -> Vec<f32> {
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let wedge = std::f32::consts::TAU / segments.clamp(2, 64) as f32;
    let a0 = angle.to_radians();
    remap(src, out, ctx, Edge::Repeat, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        if r <= 0.0 {
            return (x, y);
        }
        // The angle within its own wedge: the same positions in every wedge repeat the source.
        let mut t = (dy.atan2(dx) - a0).rem_euclid(wedge);
        if mirror {
            let index = ((dy.atan2(dx) - a0).rem_euclid(std::f32::consts::TAU) / wedge).floor() as i32;
            if index % 2 == 1 {
                t = wedge - t;
            }
        }
        let t = t + a0;
        (cx + r * t.cos(), cy + r * t.sin())
    })
}

/// How far a Long Shadow reaches, in pixels, for the tile margin.
pub(crate) fn long_shadow_halo(length: f32) -> i32 {
    length.clamp(1.0, 2000.0).ceil() as i32 + 1
}

/// Long Shadow: the layer's opaque areas cast a hard shadow `length` px long toward `angle`
/// degrees (clockwise from "right"; 45 casts down and to the right), drawn behind the layer.
/// With `fade` the shadow dies away along its length.
pub(crate) fn long_shadow(src: &Image, out: Rect, ctx: &Ctx, angle: f32, length: f32, color: [f32; 4], fade: bool) -> Vec<f32> {
    let n = src.ch;
    let mut res = src.crop(out);
    if !ctx.alpha {
        return res;
    }
    let length = length.clamp(1.0, 2000.0);
    let (dy, dx) = angle.to_radians().sin_cos();
    let steps = length.ceil() as i32;
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let own = rgba(ctx, px);
        if own[3] >= 1.0 {
            continue;
        }
        // The nearest caster upstream decides: its distance sets the fade.
        let mut shadow = 0.0f32;
        for t in 1..=steps {
            let (sx, sy) = (x - (dx * t as f32).round() as i32, y - (dy * t as f32).round() as i32);
            let a = if src.rect.contains(sx, sy) { src.px(sx, sy)[n - 1] } else { 0.0 };
            if a > 0.0 {
                let k = if fade { 1.0 - (t as f32 - 1.0) / length } else { 1.0 };
                shadow = shadow.max(a * k.clamp(0.0, 1.0));
                if shadow >= 1.0 {
                    break;
                }
            }
        }
        if shadow > 0.0 {
            let behind = [color[0], color[1], color[2], color[3] * shadow];
            set_rgba(ctx, px, over(own, behind));
        }
    }
    res
}

/// Vignette: darkens (`amount` < 0) or lightens (`amount` > 0) toward the edges. `size` is where
/// the effect starts, as a percentage of the distance from the centre to the corner; `softness`
/// is how gradually it builds up; `roundness` 100 is a circle, 0 follows the frame's shape.
pub(crate) fn vignette(src: &Image, out: Rect, ctx: &Ctx, amount: f32, size: f32, softness: f32, roundness: f32) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy, half) = (b.x0 as f32 + b.width() as f32 / 2.0, b.y0 as f32 + b.height() as f32 / 2.0, centre(b).2);
    let round = (roundness / 100.0).clamp(0.0, 1.0);
    let (hx, hy) = (b.width() as f32 / 2.0, b.height() as f32 / 2.0);
    // The semi-axes blend from the frame's own to the circle through the corner.
    let corner = (hx * hx + hy * hy).sqrt();
    let (rx, ry) = (lerp(hx * std::f32::consts::SQRT_2, corner, round).max(1.0), lerp(hy * std::f32::consts::SQRT_2, corner, round).max(1.0));
    let _ = half;
    let start = (size / 100.0).clamp(0.0, 1.0);
    let soft = (softness / 100.0).clamp(0.01, 1.0);
    let k = (amount / 100.0).clamp(-1.0, 1.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (dx, dy) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry);
        let d = (dx * dx + dy * dy).sqrt();
        let t = smoothstep(start, start + soft * (1.5 - start).max(0.1), d);
        if t <= 0.0 {
            continue;
        }
        let c = rgba(ctx, px);
        let f = |v: f32| if k < 0.0 { v * (1.0 + k * t) } else { v + (1.0 - v) * k * t };
        set_rgba(ctx, px, [f(c[0]), f(c[1]), f(c[2]), c[3]]);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn blank(w: i32, h: i32, ch: usize, fill: f32) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        let mut i = Image::new(rect, ch);
        i.data.iter_mut().for_each(|v| *v = fill);
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: ch == 4 })
    }

    const WHITE: [f32; 4] = [1.0; 4];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn every_noise_kind_varies_stays_in_range_and_follows_its_seed() {
        let (i, ctx) = blank(48, 48, 4, 0.0);
        for kind in [NoiseKind::Perlin, NoiseKind::Simplex, NoiseKind::Cell, NoiseKind::Value] {
            let a = noise_pattern(&i, i.rect, &ctx, kind, 12.0, 3, 5, (WHITE, BLACK));
            let b = noise_pattern(&i, i.rect, &ctx, kind, 12.0, 3, 5, (WHITE, BLACK));
            let c = noise_pattern(&i, i.rect, &ctx, kind, 12.0, 3, 6, (WHITE, BLACK));
            assert_eq!(a, b, "{kind:?} is deterministic");
            assert_ne!(a, c, "{kind:?} depends on the seed");
            let greys: Vec<f32> = a.chunks(4).map(|p| p[0]).collect();
            let (lo, hi) = greys.iter().fold((1.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
            assert!(lo >= 0.0 && hi <= 1.0 && hi - lo > 0.2, "{kind:?}: {lo}..{hi}");
            assert!(a.chunks(4).all(|p| p[3] == 1.0), "opaque");
        }
    }

    #[test]
    fn checkerboard_alternates_from_the_corner() {
        let (i, ctx) = blank(8, 8, 4, 0.0);
        let o = checkerboard(&i, i.rect, &ctx, 2.0, (WHITE, BLACK));
        let at = |x: usize, y: usize| o[(y * 8 + x) * 4];
        assert_eq!((at(0, 0), at(1, 1), at(2, 0), at(0, 2), at(2, 2)), (1.0, 1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn grid_draws_lines_and_keeps_the_rest() {
        let (i, ctx) = blank(10, 10, 4, 0.5);
        let o = grid(&i, i.rect, &ctx, 5.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        let at = |x: usize, y: usize| o[(y * 10 + x) * 4..(y * 10 + x) * 4 + 3].to_vec();
        assert_eq!(at(0, 3), [1.0, 0.0, 0.0]);
        assert_eq!(at(3, 5), [1.0, 0.0, 0.0]);
        assert_eq!(at(2, 2), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn kaleidoscope_repeats_the_wedge_in_every_segment() {
        let rect = Rect::new(0, 0, 65, 65);
        let mut i = Image::new(rect, 4);
        for y in 0..65 {
            for x in 0..65 {
                let k = (y * 65 + x) * 4;
                i.data[k..k + 4].copy_from_slice(&[x as f32 / 64.0, y as f32 / 64.0, ((x * 7 + y * 13) % 17) as f32 / 17.0, 1.0]);
            }
        }
        let ctx = Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true };
        for segments in [3u32, 4, 6] {
            let o = kaleidoscope(&i, rect, &ctx, segments, 0.0, (0.5, 0.5), false);
            let at = |x: f32, y: f32| {
                let (xi, yi) = (x.round() as usize, y.round() as usize);
                o[(yi * 65 + xi) * 4..(yi * 65 + xi) * 4 + 2].to_vec()
            };
            // The same point turned by one segment shows the same colour, n times round the circle.
            let r = 20.0f32;
            let first: Vec<(f32, f32)> = (0..segments).map(|k| {
                let a = 0.3 + std::f32::consts::TAU * k as f32 / segments as f32;
                (32.0 + r * a.cos(), 32.0 + r * a.sin())
            }).collect();
            for p in &first[1..] {
                let (a, b) = (at(first[0].0, first[0].1), at(p.0, p.1));
                let d: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum();
                assert!(d < 0.2, "{segments} segments: {a:?} vs {b:?}");
            }
        }
        // Mirrored copies flip every other wedge instead of repeating it.
        let rep = kaleidoscope(&i, rect, &ctx, 4, 0.0, (0.5, 0.5), false);
        let mir = kaleidoscope(&i, rect, &ctx, 4, 0.0, (0.5, 0.5), true);
        assert_ne!(rep, mir);
        assert_eq!(rep, kaleidoscope(&i, rect, &ctx, 4, 0.0, (0.5, 0.5), false));
    }

    #[test]
    fn long_shadow_extends_from_opaque_pixels_and_fades() {
        // One opaque pixel at (2, 5) on a transparent 20 × 11 layer, shadow to the right.
        let (mut i, ctx) = blank(20, 11, 4, 0.0);
        let k = (5 * 20 + 2) * 4;
        i.data[k..k + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
        let o = long_shadow(&i, i.rect, &ctx, 0.0, 6.0, [0.0, 0.0, 0.0, 1.0], false);
        let alpha = |x: usize| o[(5 * 20 + x) * 4 + 3];
        assert_eq!(alpha(2), 1.0, "the layer itself is untouched");
        assert!(alpha(3) == 1.0 && alpha(8) == 1.0, "the shadow reaches `length` px");
        assert_eq!(alpha(9), 0.0, "and stops");
        assert_eq!(alpha(1), 0.0, "nothing falls upstream");
        let faded = long_shadow(&i, i.rect, &ctx, 0.0, 6.0, [0.0, 0.0, 0.0, 1.0], true);
        assert!(faded[(5 * 20 + 3) * 4 + 3] > faded[(5 * 20 + 7) * 4 + 3]);
        // Without an alpha channel there is nothing to cast.
        let (j, c3) = blank(8, 8, 3, 0.5);
        assert_eq!(long_shadow(&j, j.rect, &c3, 0.0, 4.0, BLACK, false), j.data);
    }

    #[test]
    fn vignette_darkens_edges_not_the_centre_and_lightens_with_a_positive_amount() {
        let (i, ctx) = blank(41, 41, 4, 0.6);
        let dark = vignette(&i, i.rect, &ctx, -100.0, 30.0, 40.0, 100.0);
        let at = |o: &[f32], x: usize, y: usize| o[(y * 41 + x) * 4];
        assert!((at(&dark, 20, 20) - 0.6).abs() < 1e-6, "centre untouched");
        assert!(at(&dark, 0, 0) < 0.05, "corner goes dark: {}", at(&dark, 0, 0));
        let light = vignette(&i, i.rect, &ctx, 100.0, 30.0, 40.0, 100.0);
        assert!(at(&light, 0, 0) > 0.95);
        assert_eq!(vignette(&i, i.rect, &ctx, 0.0, 30.0, 40.0, 100.0), i.data);
    }
}
