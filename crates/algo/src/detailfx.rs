//! Detail filters: Propagate Values, Local Contrast Stretch and Wavelet Scale.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::fxutil::{gauss_blur_n, luma, rgba, set_rgba, xy};
use crate::image::Image;
use crate::noise::hash01;

/// What Propagate Values spreads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PropagateMode {
    /// Brighter pixels spread into darker neighbours.
    #[default]
    Lighter,
    /// Darker pixels spread into lighter neighbours.
    Darker,
    /// More opaque pixels spread into more transparent ones.
    MoreOpaque,
    /// More transparent pixels spread into more opaque ones.
    MoreTransparent,
}

/// What Wavelet Scale shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WaveletOutput {
    /// The detail at one scale, centred on mid-grey.
    #[default]
    Detail,
    /// What is left once the detail of that scale and finer is taken out.
    Residual,
}

/// Which neighbours may pass their value on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directions {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
}

/// How far Propagate Values reaches, for the tile margin.
pub(crate) fn propagate_halo(iterations: u32) -> i32 {
    iterations.clamp(1, 100) as i32 + 1
}

/// Propagate Values: each pixel looks at its neighbours (only from the allowed `directions`);
/// the one with the most extreme value for `mode`, if that value lies between `lower` and
/// `upper` % and beats the pixel's own, passes its colour on by `rate` %. Repeated
/// `iterations` times, so values creep further each time.
#[allow(clippy::too_many_arguments)]
pub(crate) fn propagate(src: &Image, out: Rect, ctx: &Ctx, mode: PropagateMode, lower: f32, upper: f32, rate: f32, iterations: u32, dirs: Directions) -> Vec<f32> {
    let n = src.ch;
    let (w, h) = (src.rect.width() as i32, src.rect.height() as i32);
    let (lo, hi) = ((lower / 100.0).min(upper / 100.0).clamp(0.0, 1.0), (upper / 100.0).max(lower / 100.0).clamp(0.0, 1.0));
    let k = (rate / 100.0).clamp(0.0, 1.0);
    let value = |px: &[f32]| match mode {
        PropagateMode::Lighter | PropagateMode::Darker => luma(ctx, px),
        PropagateMode::MoreOpaque | PropagateMode::MoreTransparent => rgba(ctx, px)[3],
    };
    // Whether `candidate` is a better thing to spread than `own`.
    let better = |candidate: f32, own: f32| match mode {
        PropagateMode::Lighter | PropagateMode::MoreOpaque => candidate > own,
        PropagateMode::Darker | PropagateMode::MoreTransparent => candidate < own,
    };
    let mut neighbours: Vec<(i32, i32)> = Vec::new();
    if dirs.left {
        neighbours.push((-1, 0));
    }
    if dirs.right {
        neighbours.push((1, 0));
    }
    if dirs.up {
        neighbours.push((0, -1));
    }
    if dirs.down {
        neighbours.push((0, 1));
    }
    let mut cur = src.data.clone();
    for _ in 0..iterations.clamp(1, 100) {
        let mut next = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let own = &cur[((y * w + x) as usize) * n..((y * w + x) as usize + 1) * n];
                let own_value = value(own);
                let mut best: Option<(f32, usize)> = None;
                for (dx, dy) in &neighbours {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let at = ((ny * w + nx) as usize) * n;
                    let v = value(&cur[at..at + n]);
                    if v >= lo && v <= hi && better(v, own_value) && best.is_none_or(|(b, _)| better(v, b)) {
                        best = Some((v, at));
                    }
                }
                if let Some((_, at)) = best {
                    let dst = ((y * w + x) as usize) * n;
                    for c in 0..n {
                        next[dst + c] = cur[dst + c] + (cur[at + c] - cur[dst + c]) * k;
                    }
                }
            }
        }
        cur = next;
    }
    crop(&cur, src.rect, out, n)
}

fn crop(data: &[f32], area: Rect, out: Rect, n: usize) -> Vec<f32> {
    let w = area.width() as usize;
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    for y in out.y0..out.y1 {
        let row = ((y - area.y0) as usize * w + (out.x0 - area.x0) as usize) * n;
        res.extend_from_slice(&data[row..row + out.width() as usize * n]);
    }
    res
}

/// How far Local Contrast Stretch reaches, for the tile margin.
pub(crate) fn stretch_halo(radius: f32) -> i32 {
    radius.clamp(1.0, 500.0).ceil() as i32 + 1
}

/// Local Contrast Stretch: every colour channel of every pixel is stretched between the darkest
/// and brightest values found at `samples` random points within `radius` px (the envelopes of a
/// Retinex-style normalisation); `repeats` such passes with different points are averaged.
/// `strength` 0–100 blends the result with the original.
#[allow(clippy::too_many_arguments)]
pub(crate) fn local_contrast(src: &Image, out: Rect, ctx: &Ctx, radius: f32, samples: u32, repeats: u32, strength: f32, seed: u32) -> Vec<f32> {
    let n = src.ch;
    let r = radius.clamp(1.0, 500.0);
    let samples = samples.clamp(2, 200);
    let repeats = repeats.clamp(1, 8);
    let k = (strength / 100.0).clamp(0.0, 1.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let own = rgba(ctx, px);
        let mut acc = [0.0f32; 3];
        for pass in 0..repeats {
            let mut lo = [own[0], own[1], own[2]];
            let mut hi = lo;
            for s in 0..samples {
                let salt = pass * 977 + s * 13 + 1;
                // A point in the disc, uniformly: radius by the square root of a uniform value.
                let (u, a) = (hash01(x, y, salt, seed), hash01(x, y, salt + 5000, seed) * std::f32::consts::TAU);
                let d = u.sqrt() * r;
                let (sx, sy) = ((x as f32 + d * a.cos()).round() as i32, (y as f32 + d * a.sin()).round() as i32);
                let (sx, sy) = (sx.clamp(src.rect.x0, src.rect.x1 - 1), sy.clamp(src.rect.y0, src.rect.y1 - 1));
                let c = rgba(ctx, src.px(sx, sy));
                for ch in 0..3 {
                    lo[ch] = lo[ch].min(c[ch]);
                    hi[ch] = hi[ch].max(c[ch]);
                }
            }
            for ch in 0..3 {
                let span = hi[ch] - lo[ch];
                acc[ch] += if span > 1e-4 { (own[ch] - lo[ch]) / span } else { own[ch] };
            }
        }
        let f = |ch: usize| own[ch] + (acc[ch] / repeats as f32 - own[ch]) * k;
        set_rgba(ctx, px, [f(0), f(1), f(2), own[3]]);
    }
    res
}

/// How far Wavelet Scale reaches, for the tile margin.
pub(crate) fn wavelet_halo(scale: u32) -> i32 {
    3 * (1i32 << scale.clamp(1, 8)) + 2
}

/// Wavelet Scale: the picture is blurred with doubling radii (1, 2, 4, … px); `scale` 1–8
/// picks the band between the previous blur and this one (`detail`, centred on mid-grey) or
/// the blur itself (`residual`). Detail at every scale plus the last residual rebuilds the image.
pub(crate) fn wavelet_scale(src: &Image, out: Rect, ctx: &Ctx, scale: u32, output: WaveletOutput) -> Vec<f32> {
    let n = src.ch;
    let scale = scale.clamp(1, 8);
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let sigma = |level: u32| (1u32 << level.saturating_sub(1)) as f32;
    let mut prev = src.data.clone();
    let mut cur = src.data.clone();
    for level in 1..=scale {
        prev = cur.clone();
        gauss_blur_n(&mut cur, w, h, n, sigma(level));
    }
    let colours = if ctx.alpha { n - 1 } else { n };
    let mut res = src.crop(out);
    let cols = src.rect.width() as usize;
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let k = ((y - src.rect.y0) as usize * cols + (x - src.rect.x0) as usize) * n;
        for c in 0..colours {
            px[c] = match output {
                WaveletOutput::Detail => (0.5 + prev[k + c] - cur[k + c]).clamp(0.0, 1.0),
                WaveletOutput::Residual => cur[k + c],
            };
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn img(w: i32, h: i32, f: impl Fn(i32, i32) -> f32) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        let mut i = Image::new(rect, 4);
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                let k = ((y * w + x) * 4) as usize;
                i.data[k..k + 4].copy_from_slice(&[v, v, v, 1.0]);
            }
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    const ALL: Directions = Directions { left: true, right: true, up: true, down: true };

    #[test]
    fn bright_pixels_spread_into_dark_neighbours_a_step_per_iteration() {
        // One white pixel on black.
        let (i, ctx) = img(11, 11, |x, y| if (x, y) == (5, 5) { 1.0 } else { 0.0 });
        let one = propagate(&i, i.rect, &ctx, PropagateMode::Lighter, 0.0, 100.0, 100.0, 1, ALL);
        let at = |o: &[f32], x: usize, y: usize| o[(y * 11 + x) * 4];
        assert_eq!((at(&one, 4, 5), at(&one, 6, 5), at(&one, 5, 4), at(&one, 5, 6)), (1.0, 1.0, 1.0, 1.0));
        assert_eq!(at(&one, 3, 5), 0.0, "one step per iteration");
        let three = propagate(&i, i.rect, &ctx, PropagateMode::Lighter, 0.0, 100.0, 100.0, 3, ALL);
        assert_eq!(at(&three, 2, 5), 1.0);
        let dark = propagate(&i, i.rect, &ctx, PropagateMode::Darker, 0.0, 100.0, 100.0, 1, ALL);
        assert_eq!(at(&dark, 5, 5), 0.0, "the white pixel is eaten by the dark ones");
        let only_right = propagate(&i, i.rect, &ctx, PropagateMode::Lighter, 0.0, 100.0, 100.0, 1, Directions { left: true, right: false, up: false, down: false });
        assert_eq!((at(&only_right, 6, 5), at(&only_right, 4, 5)), (1.0, 0.0), "values flow only in the allowed direction");
        let limited = propagate(&i, i.rect, &ctx, PropagateMode::Lighter, 0.0, 50.0, 100.0, 1, ALL);
        assert_eq!(limited, i.data, "a value above the upper limit does not spread");
    }

    #[test]
    fn local_contrast_stretches_a_dull_gradient_toward_the_full_range() {
        let (i, ctx) = img(64, 64, |x, _| 0.4 + 0.2 * x as f32 / 63.0);
        let o = local_contrast(&i, i.rect, &ctx, 40.0, 40, 2, 100.0, 1);
        let vals: Vec<f32> = o.chunks(4).map(|p| p[0]).collect();
        let (lo, hi) = vals.iter().fold((1.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(hi - lo > 0.6, "{lo}..{hi} from 0.4..0.6");
        assert_eq!(o, local_contrast(&i, i.rect, &ctx, 40.0, 40, 2, 100.0, 1));
        assert_eq!(local_contrast(&i, i.rect, &ctx, 40.0, 40, 2, 0.0, 1), i.data);
        let (flat, c2) = img(16, 16, |_, _| 0.3);
        assert_eq!(local_contrast(&flat, flat.rect, &c2, 10.0, 20, 1, 100.0, 1), flat.data);
    }

    #[test]
    fn wavelet_scales_split_and_rebuild_the_picture() {
        let (i, ctx) = img(48, 48, |x, y| if (x / 3 + y / 3) % 2 == 0 { 0.8 } else { 0.2 });
        // Detail at every scale plus the final residual adds back to the original (away from the edges).
        let scales = 3;
        let mut sum: Vec<f32> = wavelet_scale(&i, i.rect, &ctx, scales, WaveletOutput::Residual).chunks(4).map(|p| p[0]).collect();
        for s in 1..=scales {
            let d = wavelet_scale(&i, i.rect, &ctx, s, WaveletOutput::Detail);
            for (acc, p) in sum.iter_mut().zip(d.chunks(4)) {
                *acc += p[0] - 0.5;
            }
        }
        for (x, y) in [(20usize, 20usize), (24, 24), (30, 17)] {
            let want = i.data[(y * 48 + x) * 4];
            assert!((sum[y * 48 + x] - want).abs() < 0.02, "({x},{y}): {} vs {want}", sum[y * 48 + x]);
        }
        let fine = wavelet_scale(&i, i.rect, &ctx, 1, WaveletOutput::Detail);
        let coarse = wavelet_scale(&i, i.rect, &ctx, 6, WaveletOutput::Detail);
        let energy = |o: &[f32]| o.chunks(4).map(|p| (p[0] - 0.5).powi(2)).sum::<f32>();
        assert!(energy(&fine) > 0.0 && energy(&coarse) > 0.0);
        let (flat, c2) = img(16, 16, |_, _| 0.4);
        assert!(wavelet_scale(&flat, flat.rect, &c2, 2, WaveletOutput::Detail).chunks(4).all(|p| (p[0] - 0.5).abs() < 0.02), "flat has no detail");
    }
}
