//! Edge detection (Sobel, Prewitt, Roberts, Gradient, Differential, Laplace) and Difference of
//! Gaussians. Both write bright edges on a dark ground in the image's own channels (ink models
//! invert, so edges stay "bright" there too) and leave alpha alone.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::fxutil::{gauss_blur_n, subtractive};
use crate::image::Image;

/// The operator Edge Detect applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EdgeAlgorithm {
    #[default]
    Sobel,
    Prewitt,
    /// Roberts cross: a 2×2 diagonal difference.
    Roberts,
    /// Central differences along x and y.
    Gradient,
    /// Forward differences to the right and below, added.
    Differential,
    /// The 8-neighbour Laplacian.
    Laplace,
}

fn channels(src: &Image, ctx: &Ctx) -> usize {
    if ctx.alpha { src.ch - 1 } else { src.ch }
}

/// Edge strength of channel `c` at `(x, y)` before the amount is applied (about `0..=1`).
fn strength(src: &Image, x: i32, y: i32, c: usize, alg: EdgeAlgorithm) -> f32 {
    let g = |dx: i32, dy: i32| src.get(x + dx, y + dy, c);
    match alg {
        EdgeAlgorithm::Sobel => {
            let gx = (g(1, -1) + 2.0 * g(1, 0) + g(1, 1)) - (g(-1, -1) + 2.0 * g(-1, 0) + g(-1, 1));
            let gy = (g(-1, 1) + 2.0 * g(0, 1) + g(1, 1)) - (g(-1, -1) + 2.0 * g(0, -1) + g(1, -1));
            (gx * gx + gy * gy).sqrt() / 4.0
        }
        EdgeAlgorithm::Prewitt => {
            let gx = (g(1, -1) + g(1, 0) + g(1, 1)) - (g(-1, -1) + g(-1, 0) + g(-1, 1));
            let gy = (g(-1, 1) + g(0, 1) + g(1, 1)) - (g(-1, -1) + g(0, -1) + g(1, -1));
            (gx * gx + gy * gy).sqrt() / 3.0
        }
        EdgeAlgorithm::Roberts => {
            let a = g(0, 0) - g(1, 1);
            let b = g(1, 0) - g(0, 1);
            (a * a + b * b).sqrt()
        }
        EdgeAlgorithm::Gradient => {
            let gx = (g(1, 0) - g(-1, 0)) / 2.0;
            let gy = (g(0, 1) - g(0, -1)) / 2.0;
            (gx * gx + gy * gy).sqrt()
        }
        EdgeAlgorithm::Differential => (g(0, 0) - g(1, 0)).abs() + (g(0, 0) - g(0, 1)).abs(),
        EdgeAlgorithm::Laplace => {
            let sum: f32 = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().map(|&(dx, dy)| g(dx, dy)).sum();
            (8.0 * g(0, 0) - sum).abs() / 8.0
        }
    }
}

/// Edge Detect with `amount` (a multiplier, 1 = the operator's own scale).
pub(crate) fn edge_detect(src: &Image, out: Rect, ctx: &Ctx, alg: EdgeAlgorithm, amount: f32) -> Vec<f32> {
    let n = src.ch;
    let cc = channels(src, ctx);
    let sub = subtractive(ctx);
    let mut res = src.crop(out);
    let w = out.width() as usize;
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = (out.x0 + (i % w) as i32, out.y0 + (i / w) as i32);
        for (c, v) in px.iter_mut().enumerate().take(cc) {
            let e = (strength(src, x, y, c, alg) * amount).clamp(0.0, 1.0);
            *v = if sub { 1.0 - e } else { e };
        }
    }
    res
}

/// The margin Difference of Gaussians needs around a tile.
pub(crate) fn dog_halo(radius1: f32, radius2: f32) -> i32 {
    (radius1.max(radius2).max(0.0) * 3.0).ceil() as i32 + 1
}

/// Difference of Gaussians: `|blur(radius1) − blur(radius2)|` per colour channel (radii are standard
/// deviations in pixels). `normalize` stretches the result so its strongest edge reaches 1, which
/// needs the whole image (the caller gives it the full bounds); `invert` flips it.
pub(crate) fn difference_of_gaussians(src: &Image, out: Rect, ctx: &Ctx, radius1: f32, radius2: f32, normalize: bool, invert: bool) -> Vec<f32> {
    let n = src.ch;
    let cc = channels(src, ctx);
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let mut a = src.data.clone();
    let mut b = src.data.clone();
    gauss_blur_n(&mut a, w, h, n, radius1);
    gauss_blur_n(&mut b, w, h, n, radius2);
    let sub = subtractive(ctx);
    let diff = |x: i32, y: i32, c: usize| {
        let k = ((y - src.rect.y0) as usize * w + (x - src.rect.x0) as usize) * n + c;
        (a[k] - b[k]).abs()
    };
    let peak = if normalize {
        let mut m = 1e-6f32;
        for y in src.rect.y0..src.rect.y1 {
            for x in src.rect.x0..src.rect.x1 {
                for c in 0..cc {
                    m = m.max(diff(x, y, c));
                }
            }
        }
        m
    } else {
        1.0
    };
    let mut res = src.crop(out);
    let ow = out.width() as usize;
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = (out.x0 + (i % ow) as i32, out.y0 + (i / ow) as i32);
        for (c, v) in px.iter_mut().enumerate().take(cc) {
            let mut e = (diff(x, y, c) / peak).clamp(0.0, 1.0);
            if invert {
                e = 1.0 - e;
            }
            *v = if sub { 1.0 - e } else { e };
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    /// A 9×9 grey image: left half black, right half white.
    fn step() -> (Image, Ctx) {
        let rect = Rect::new(0, 0, 9, 9);
        let mut i = Image::new(rect, 1);
        for y in 0..9 {
            for x in 0..9 {
                i.data[y * 9 + x] = if x >= 5 { 1.0 } else { 0.0 };
            }
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Grayscale, alpha: false })
    }

    #[test]
    fn every_operator_lights_the_edge_and_leaves_flat_areas_black() {
        let (i, ctx) = step();
        for alg in [EdgeAlgorithm::Sobel, EdgeAlgorithm::Prewitt, EdgeAlgorithm::Roberts, EdgeAlgorithm::Gradient, EdgeAlgorithm::Differential, EdgeAlgorithm::Laplace] {
            let inner = Rect::new(1, 1, 8, 8);
            let o = edge_detect(&i, inner, &ctx, alg, 1.0);
            let w = inner.width() as usize;
            let at = |x: i32, y: i32| o[(y - 1) as usize * w + (x - 1) as usize];
            assert!(at(2, 4) < 1e-6 && at(7, 4) < 1e-6, "{alg:?}: flat areas stay black");
            let near_edge = at(4, 4).max(at(5, 4));
            assert!(near_edge > 0.2, "{alg:?}: the step lights up ({near_edge})");
        }
    }

    #[test]
    fn amount_scales_and_clamps() {
        let (i, ctx) = step();
        let area = Rect::new(4, 4, 5, 5);
        let one = edge_detect(&i, area, &ctx, EdgeAlgorithm::Sobel, 1.0)[0];
        let two = edge_detect(&i, area, &ctx, EdgeAlgorithm::Sobel, 2.0)[0];
        assert!((two - (one * 2.0).min(1.0)).abs() < 1e-6);
        assert_eq!(edge_detect(&i, area, &ctx, EdgeAlgorithm::Sobel, 100.0)[0], 1.0);
    }

    #[test]
    fn difference_of_gaussians_finds_the_step_and_normalizes() {
        let (i, ctx) = step();
        let all = i.rect;
        let raw = difference_of_gaussians(&i, all, &ctx, 0.8, 2.0, false, false);
        let norm = difference_of_gaussians(&i, all, &ctx, 0.8, 2.0, true, false);
        let at = |v: &[f32], x: usize, y: usize| v[y * 9 + x];
        assert!(at(&raw, 4, 4) > at(&raw, 1, 4), "edge brighter than the flat area");
        assert!((norm.iter().cloned().fold(0.0, f32::max) - 1.0).abs() < 1e-5, "strongest edge reaches 1");
        let inv = difference_of_gaussians(&i, all, &ctx, 0.8, 2.0, false, true);
        assert!((at(&raw, 4, 4) + at(&inv, 4, 4) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn halo_covers_three_sigma() {
        assert_eq!(dog_halo(1.0, 4.0), 13);
    }
}
