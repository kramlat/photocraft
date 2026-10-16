//! Tone and pixel filters: Stretch Levels, Enhance Colors, Dither and Scatter Pixels. Colours are
//! handled as straight sRGB RGBA, so every colour model and depth takes the same path; alpha is
//! never changed.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::colorops::{from_hsv, to_hsv};
use crate::fxutil::{rgba, set_rgba, xy};
use crate::image::Image;
use crate::noise::hash01;

/// What Stretch Levels stretches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum StretchMode {
    /// Each of R, G and B to the full range on its own (colours shift).
    #[default]
    Channels,
    /// One shared range for all three, so colours keep their balance.
    Together,
    /// Only the HSB brightness; hue and saturation stay.
    Brightness,
}

/// How Dither reduces each channel to a few levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DitherMethod {
    /// Ordered dithering with a 2 × 2 Bayer matrix.
    Bayer2,
    /// Ordered dithering with a 4 × 4 Bayer matrix.
    #[default]
    Bayer4,
    /// Ordered dithering with an 8 × 8 Bayer matrix.
    Bayer8,
    /// Random threshold per pixel.
    Random,
    /// Floyd–Steinberg error diffusion.
    FloydSteinberg,
}

/// How Scatter Pixels moves pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ScatterMode {
    /// Pixels replaced by random colours.
    #[default]
    Random,
    /// Pixels replaced by a random neighbour.
    Pick,
    /// Pixels smeared from the pixels above.
    Smear,
    /// Pixels displaced by a random offset.
    Spread,
}

fn luma_pixels(src: &Image, ctx: &Ctx) -> impl Iterator<Item = [f32; 4]> {
    src.data.chunks_exact(src.ch).map(|px| rgba(ctx, px))
}

/// Stretch Levels: the occupied range of the reference area is stretched to the full range.
pub(crate) fn stretch_levels(src: &Image, out: Rect, ctx: &Ctx, mode: StretchMode) -> Vec<f32> {
    let n = src.ch;
    let seen: Vec<[f32; 4]> = luma_pixels(src, ctx).filter(|c| c[3] > 0.0).collect();
    let mut res = src.crop(out);
    let span = |lo: f32, hi: f32| if hi - lo > 1e-6 { 1.0 / (hi - lo) } else { 1.0 };
    match mode {
        StretchMode::Channels | StretchMode::Together => {
            let mut lo = [1.0f32; 3];
            let mut hi = [0.0f32; 3];
            for c in &seen {
                for k in 0..3 {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
            if mode == StretchMode::Together {
                let (l, h) = (lo.iter().copied().fold(1.0, f32::min), hi.iter().copied().fold(0.0, f32::max));
                lo = [l; 3];
                hi = [h; 3];
            }
            for px in res.chunks_exact_mut(n) {
                let c = rgba(ctx, px);
                let f = |k: usize| ((c[k] - lo[k]) * span(lo[k], hi[k])).clamp(0.0, 1.0);
                set_rgba(ctx, px, [f(0), f(1), f(2), c[3]]);
            }
        }
        StretchMode::Brightness => {
            let (mut lo, mut hi) = (1.0f32, 0.0f32);
            for c in &seen {
                let v = c[0].max(c[1]).max(c[2]);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let k = span(lo, hi);
            for px in res.chunks_exact_mut(n) {
                let c = rgba(ctx, px);
                let [h, s, v] = to_hsv([c[0], c[1], c[2]]);
                let rgb = from_hsv(h, s, ((v - lo) * k).clamp(0.0, 1.0));
                set_rgba(ctx, px, [rgb[0], rgb[1], rgb[2], c[3]]);
            }
        }
    }
    res
}

/// Enhance Colors: saturation and brightness are each stretched over their occupied range
/// (hue is kept), which makes a dull picture more vivid without shifting its colours.
pub(crate) fn enhance_colors(src: &Image, out: Rect, ctx: &Ctx) -> Vec<f32> {
    let n = src.ch;
    let (mut slo, mut shi, mut vlo, mut vhi) = (1.0f32, 0.0f32, 1.0f32, 0.0f32);
    for c in luma_pixels(src, ctx).filter(|c| c[3] > 0.0) {
        let [_, s, v] = to_hsv([c[0], c[1], c[2]]);
        slo = slo.min(s);
        shi = shi.max(s);
        vlo = vlo.min(v);
        vhi = vhi.max(v);
    }
    let span = |lo: f32, hi: f32| if hi - lo > 1e-6 { 1.0 / (hi - lo) } else { 1.0 };
    let (ks, kv) = (span(slo, shi), span(vlo, vhi));
    let mut res = src.crop(out);
    for px in res.chunks_exact_mut(n) {
        let c = rgba(ctx, px);
        let [h, s, v] = to_hsv([c[0], c[1], c[2]]);
        // Grey pixels keep zero saturation.
        let s2 = if s > 0.0 { ((s - slo) * ks).clamp(0.0, 1.0) } else { 0.0 };
        let rgb = from_hsv(h, s2, ((v - vlo) * kv).clamp(0.0, 1.0));
        set_rgba(ctx, px, [rgb[0], rgb[1], rgb[2], c[3]]);
    }
    res
}

fn bayer(order: usize) -> Vec<f32> {
    // Recursive construction: M(2n) = [[4M, 4M+2], [4M+3, 4M+1]].
    let mut m = vec![0u32];
    let mut size = 1;
    while size < order {
        let mut next = vec![0u32; (size * 2) * (size * 2)];
        for y in 0..size {
            for x in 0..size {
                let v = m[y * size + x] * 4;
                next[y * size * 2 + x] = v;
                next[y * size * 2 + x + size] = v + 2;
                next[(y + size) * size * 2 + x] = v + 3;
                next[(y + size) * size * 2 + x + size] = v + 1;
            }
        }
        m = next;
        size *= 2;
    }
    let count = (order * order) as f32;
    m.into_iter().map(|v| (v as f32 + 0.5) / count).collect()
}

/// Dither: every colour channel is reduced to `levels` values (2–32), with the dither method
/// spreading the rounding error so smooth ramps keep their look.
pub(crate) fn dither(src: &Image, out: Rect, ctx: &Ctx, method: DitherMethod, levels: u32, seed: u32) -> Vec<f32> {
    let n = src.ch;
    let steps = (levels.clamp(2, 32) - 1) as f32;
    let quantize = |v: f32, bias: f32| ((v.clamp(0.0, 1.0) * steps + bias).floor().clamp(0.0, steps)) / steps;
    let b = ctx.bounds;
    let mut res = src.crop(out);
    match method {
        DitherMethod::Bayer2 | DitherMethod::Bayer4 | DitherMethod::Bayer8 | DitherMethod::Random => {
            let order = match method {
                DitherMethod::Bayer2 => 2,
                DitherMethod::Bayer8 => 8,
                _ => 4,
            };
            let matrix = bayer(order);
            for (i, px) in res.chunks_exact_mut(n).enumerate() {
                let (x, y) = xy(out, i);
                let threshold = if method == DitherMethod::Random {
                    hash01(x, y, 11, seed)
                } else {
                    matrix[((y - b.y0).rem_euclid(order as i32) as usize) * order + (x - b.x0).rem_euclid(order as i32) as usize]
                };
                let c = rgba(ctx, px);
                // Rounding up when the fraction beats the threshold.
                let f = |v: f32| quantize(v, 1.0 - threshold);
                set_rgba(ctx, px, [f(c[0]), f(c[1]), f(c[2]), c[3]]);
            }
        }
        DitherMethod::FloydSteinberg => {
            // Error diffusion runs in reading order over the whole reference area.
            let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
            let mut buf: Vec<[f32; 3]> = src.data.chunks_exact(n).map(|px| {
                let c = rgba(ctx, px);
                [c[0], c[1], c[2]]
            }).collect();
            for y in 0..h {
                for x in 0..w {
                    let old = buf[y * w + x];
                    let new = [quantize(old[0], 0.5), quantize(old[1], 0.5), quantize(old[2], 0.5)];
                    buf[y * w + x] = new;
                    let err = [old[0] - new[0], old[1] - new[1], old[2] - new[2]];
                    let mut push = |dx: isize, dy: usize, k: f32| {
                        let (nx, ny) = (x as isize + dx, y + dy);
                        if nx >= 0 && (nx as usize) < w && ny < h {
                            let t = &mut buf[ny * w + nx as usize];
                            for c in 0..3 {
                                t[c] += err[c] * k;
                            }
                        }
                    };
                    push(1, 0, 7.0 / 16.0);
                    push(-1, 1, 3.0 / 16.0);
                    push(0, 1, 5.0 / 16.0);
                    push(1, 1, 1.0 / 16.0);
                }
            }
            for (i, px) in res.chunks_exact_mut(n).enumerate() {
                let (x, y) = xy(out, i);
                let k = (y - src.rect.y0) as usize * w + (x - src.rect.x0) as usize;
                let c = rgba(ctx, px);
                let v = buf.get(k).copied().unwrap_or([c[0], c[1], c[2]]);
                set_rgba(ctx, px, [v[0], v[1], v[2], c[3]]);
            }
        }
    }
    res
}

/// How far Scatter Pixels reaches, for the tile margin.
pub(crate) fn scatter_halo(radius: f32) -> i32 {
    radius.clamp(1.0, 200.0).ceil() as i32 + 1
}

/// Scatter Pixels: each pixel is, with probability `amount` %, replaced according to `mode`
/// (random colour, random neighbour within `radius`, pixel from above, random displacement).
pub(crate) fn scatter_pixels(src: &Image, out: Rect, ctx: &Ctx, mode: ScatterMode, amount: f32, radius: f32, seed: u32) -> Vec<f32> {
    let n = src.ch;
    let p = (amount / 100.0).clamp(0.0, 1.0);
    let r = radius.clamp(1.0, 200.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        if hash01(x, y, 21, seed) >= p {
            continue;
        }
        let sample = |dx: i32, dy: i32| -> Option<&[f32]> {
            let (sx, sy) = (x + dx, y + dy);
            src.rect.contains(sx, sy).then(|| src.px(sx, sy))
        };
        let off = |salt: u32| ((hash01(x, y, salt, seed) * 2.0 - 1.0) * r).round() as i32;
        match mode {
            ScatterMode::Random => {
                let c = rgba(ctx, px);
                set_rgba(ctx, px, [hash01(x, y, 31, seed), hash01(x, y, 32, seed), hash01(x, y, 33, seed), c[3]]);
            }
            ScatterMode::Pick => {
                if let Some(s) = sample(off(41), off(42)) {
                    px.copy_from_slice(s);
                }
            }
            ScatterMode::Smear => {
                let up = (hash01(x, y, 51, seed) * r).ceil().max(1.0) as i32;
                if let Some(s) = sample(0, -up) {
                    px.copy_from_slice(s);
                }
            }
            ScatterMode::Spread => {
                if let Some(s) = sample(off(61), off(62)) {
                    px.copy_from_slice(s);
                }
            }
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn img(w: i32, h: i32, f: impl Fn(i32, i32) -> [f32; 4]) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        let mut i = Image::new(rect, 4);
        for y in 0..h {
            for x in 0..w {
                let k = ((y * w + x) * 4) as usize;
                i.data[k..k + 4].copy_from_slice(&f(x, y));
            }
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    #[test]
    fn stretching_fills_the_range_in_each_mode() {
        let (i, ctx) = img(8, 1, |x, _| {
            let v = 0.25 + x as f32 / 8.0 * 0.5;
            [v, v * 0.5 + 0.1, 0.4, 1.0]
        });
        let ch = stretch_levels(&i, i.rect, &ctx, StretchMode::Channels);
        for k in 0..3 {
            let vals: Vec<f32> = ch.chunks(4).map(|p| p[k]).collect();
            let (lo, hi) = vals.iter().fold((1.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
            if k < 2 {
                assert!(lo < 0.001 && hi > 0.999, "channel {k}: {lo}..{hi}");
            } else {
                assert!(vals.iter().all(|v| (*v - vals[0]).abs() < 1e-6), "a flat channel stays flat");
            }
        }
        let br = stretch_levels(&i, i.rect, &ctx, StretchMode::Brightness);
        let v: Vec<f32> = br.chunks(4).map(|p| p[0].max(p[1]).max(p[2])).collect();
        assert!(v.iter().copied().fold(1.0, f32::min) < 0.001 && v.iter().copied().fold(0.0, f32::max) > 0.999);
        let all = stretch_levels(&i, i.rect, &ctx, StretchMode::Together);
        assert!(all.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn enhancing_keeps_hue_and_grey_and_spreads_saturation() {
        let (i, ctx) = img(4, 1, |x, _| match x {
            0 => [0.5, 0.5, 0.5, 1.0],
            1 => [0.5, 0.45, 0.4, 1.0],
            2 => [0.5, 0.4, 0.3, 1.0],
            _ => [0.6, 0.45, 0.3, 1.0],
        });
        let o = enhance_colors(&i, i.rect, &ctx);
        assert!((o[0] - o[1]).abs() < 1e-6 && (o[1] - o[2]).abs() < 1e-6, "grey stays grey");
        let sat = |k: usize| to_hsv([o[k * 4], o[k * 4 + 1], o[k * 4 + 2]])[1];
        assert!(sat(3) > 0.99 && sat(1) < 0.5);
    }

    #[test]
    fn dither_reaches_only_the_allowed_levels_and_keeps_the_mean() {
        let (i, ctx) = img(32, 32, |x, _| {
            let v = x as f32 / 31.0;
            [v, v, v, 1.0]
        });
        for method in [DitherMethod::Bayer2, DitherMethod::Bayer4, DitherMethod::Bayer8, DitherMethod::Random, DitherMethod::FloydSteinberg] {
            let o = dither(&i, i.rect, &ctx, method, 2, 3);
            assert!(o.chunks(4).all(|p| p[0] == 0.0 || p[0] == 1.0), "{method:?} binary");
            let mean = o.chunks(4).map(|p| p[0]).sum::<f32>() / 1024.0;
            assert!((mean - 0.5).abs() < 0.06, "{method:?}: mean {mean}");
            assert!(o.chunks(4).all(|p| p[3] == 1.0));
        }
        let four = dither(&i, i.rect, &ctx, DitherMethod::Bayer4, 4, 0);
        assert!(four.chunks(4).all(|p| [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0].iter().any(|l| (p[0] - l).abs() < 1e-5)));
    }

    #[test]
    fn scatter_modes_move_pixels_deterministically() {
        let (i, ctx) = img(24, 24, |x, y| [x as f32 / 23.0, y as f32 / 23.0, 0.5, 1.0]);
        for mode in [ScatterMode::Random, ScatterMode::Pick, ScatterMode::Smear, ScatterMode::Spread] {
            let a = scatter_pixels(&i, i.rect, &ctx, mode, 40.0, 4.0, 9);
            let b = scatter_pixels(&i, i.rect, &ctx, mode, 40.0, 4.0, 9);
            assert_eq!(a, b);
            let changed = a.chunks(4).zip(i.data.chunks(4)).filter(|(p, q)| p != q).count();
            assert!(changed > 100 && changed < 400, "{mode:?}: {changed} of 576 changed");
            assert_eq!(scatter_pixels(&i, i.rect, &ctx, mode, 0.0, 4.0, 9), i.data);
        }
    }

    #[test]
    fn bayer_matrices_hold_each_threshold_once() {
        for order in [2usize, 4, 8] {
            let mut m = bayer(order);
            m.sort_by(f32::total_cmp);
            assert!(m.windows(2).all(|w| w[1] > w[0]));
            assert!(m[0] > 0.0 && m[m.len() - 1] < 1.0);
        }
    }
}
