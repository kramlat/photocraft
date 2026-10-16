//! Superpixels: SLIC and Waterpixels. The picture is divided into compact regions that follow
//! its edges, and each region is painted with its average colour (optionally outlined).
//!
//! * SLIC: R. Achanta et al., *SLIC Superpixels Compared to State-of-the-Art Superpixel Methods*,
//!   IEEE TPAMI 34(11), 2012 — k-means in (L, a, b, x, y) over a window of twice the grid step.
//! * Waterpixels: V. Machairas et al., *Waterpixels*, IEEE TIP 24(11), 2015 — a watershed of the
//!   gradient from one marker per grid cell, with a regularising distance term.
//!
//! The regions are found on a copy reduced to at most `PROXY` px on its long side, so the cost
//! stays flat on big pictures; edges of regions then follow the reduced picture's pixels.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::colorops::srgb_to_linear;
use crate::fxutil::{rgba, set_rgba, xy};
use crate::image::Image;

const PROXY: usize = 1600;

/// How the regions are found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SuperpixelMethod {
    #[default]
    Slic,
    Waterpixels,
}

fn lab(c: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]));
    let (x, y, z) = ((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047, 0.2126 * r + 0.7152 * g + 0.0722 * b, (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The reduction factor and size of the working grid.
fn proxy_dims(w: usize, h: usize) -> (usize, usize, usize) {
    let k = w.max(h).div_ceil(PROXY).max(1);
    (k, w.div_ceil(k), h.div_ceil(k))
}

/// Box-averaged colours of the working grid.
fn reduce(src: &Image, ctx: &Ctx, k: usize, sw: usize, sh: usize) -> Vec<[f32; 3]> {
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let mut out = vec![[0.0f32; 3]; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
            for yy in y * k..((y + 1) * k).min(h) {
                for xx in x * k..((x + 1) * k).min(w) {
                    let c = rgba(ctx, &src.data[(yy * w + xx) * src.ch..(yy * w + xx + 1) * src.ch]);
                    for i in 0..3 {
                        acc[i] += c[i];
                    }
                    n += 1.0;
                }
            }
            out[y * sw + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
        }
    }
    out
}

/// SLIC on the working grid: a label per cell (`0..centres`).
fn slic(colours: &[[f32; 3]], w: usize, h: usize, step: f32, compactness: f32, iterations: u32) -> Vec<u32> {
    let s = step.max(2.0);
    let labs: Vec<[f32; 3]> = colours.iter().map(|c| lab(*c)).collect();
    // Centres on a grid, nudged to the lowest-gradient pixel of their 3 × 3 neighbourhood.
    let mut centres: Vec<[f32; 5]> = Vec::new();
    let mut y = s / 2.0;
    while y < h as f32 {
        let mut x = s / 2.0;
        while x < w as f32 {
            let (cx, cy) = (x as usize, y as usize);
            let grad = |x: usize, y: usize| {
                let at = |xx: usize, yy: usize| labs[yy.min(h - 1) * w + xx.min(w - 1)][0];
                (at(x + 1, y) - at(x.saturating_sub(1), y)).abs() + (at(x, y + 1) - at(x, y.saturating_sub(1))).abs()
            };
            let mut best = (cx.min(w - 1), cy.min(h - 1));
            let mut best_g = f32::MAX;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = ((cx as i32 + dx).clamp(0, w as i32 - 1) as usize, (cy as i32 + dy).clamp(0, h as i32 - 1) as usize);
                    let g = grad(nx, ny);
                    if g < best_g {
                        best_g = g;
                        best = (nx, ny);
                    }
                }
            }
            let l = labs[best.1 * w + best.0];
            centres.push([l[0], l[1], l[2], best.0 as f32, best.1 as f32]);
            x += s;
        }
        y += s;
    }
    if centres.is_empty() {
        // Picture smaller than half a cell: one region around its middle.
        let (cx, cy) = (w / 2, h / 2);
        let l = labs[cy * w + cx];
        centres.push([l[0], l[1], l[2], cx as f32, cy as f32]);
    }
    let mut labels = vec![0u32; w * h];
    let mut dist = vec![f32::MAX; w * h];
    let m2 = (compactness.max(0.1) / s).powi(2);
    for _ in 0..iterations.clamp(1, 20) {
        dist.iter_mut().for_each(|d| *d = f32::MAX);
        for (ci, c) in centres.iter().enumerate() {
            let (x0, x1) = (((c[3] - 2.0 * s).max(0.0)) as usize, ((c[3] + 2.0 * s).min(w as f32 - 1.0)) as usize);
            let (y0, y1) = (((c[4] - 2.0 * s).max(0.0)) as usize, ((c[4] + 2.0 * s).min(h as f32 - 1.0)) as usize);
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    let l = labs[yy * w + xx];
                    let dc = (l[0] - c[0]).powi(2) + (l[1] - c[1]).powi(2) + (l[2] - c[2]).powi(2);
                    let ds = (xx as f32 - c[3]).powi(2) + (yy as f32 - c[4]).powi(2);
                    let d = dc + m2 * ds;
                    if d < dist[yy * w + xx] {
                        dist[yy * w + xx] = d;
                        labels[yy * w + xx] = ci as u32;
                    }
                }
            }
        }
        let mut sums = vec![[0.0f32; 6]; centres.len()];
        for yy in 0..h {
            for xx in 0..w {
                let (l, s) = (labs[yy * w + xx], &mut sums[labels[yy * w + xx] as usize]);
                s[0] += l[0];
                s[1] += l[1];
                s[2] += l[2];
                s[3] += xx as f32;
                s[4] += yy as f32;
                s[5] += 1.0;
            }
        }
        for (c, s) in centres.iter_mut().zip(&sums) {
            if s[5] > 0.0 {
                *c = [s[0] / s[5], s[1] / s[5], s[2] / s[5], s[3] / s[5], s[4] / s[5]];
            }
        }
    }
    labels
}

/// Waterpixels on the working grid: a watershed of the smoothed gradient from one marker per grid
/// cell; each marker spreads by `gradient + regularity · distance / step`.
fn waterpixels(colours: &[[f32; 3]], w: usize, h: usize, step: f32, regularity: f32) -> Vec<u32> {
    let s = step.max(2.0);
    let lum: Vec<f32> = colours.iter().map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).collect();
    let at = |x: i64, y: i64| lum[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize];
    let mut grad = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as i64, y as i64);
            let gx = at(xi + 1, yi) - at(xi - 1, yi);
            let gy = at(xi, yi + 1) - at(xi, yi - 1);
            grad[y * w + x] = (gx * gx + gy * gy).sqrt();
        }
    }
    // One marker per cell: the lowest-gradient pixel of the cell's central half.
    let mut markers: Vec<(usize, usize)> = Vec::new();
    let mut cy = 0.0;
    while cy < h as f32 {
        let mut cx = 0.0;
        while cx < w as f32 {
            let (x0, x1) = ((cx + s * 0.25) as usize, ((cx + s * 0.75) as usize).min(w - 1));
            let (y0, y1) = ((cy + s * 0.25) as usize, ((cy + s * 0.75) as usize).min(h - 1));
            let mut best = (x0.min(w - 1), y0.min(h - 1));
            let mut best_g = f32::MAX;
            for y in y0.min(h - 1)..=y1 {
                for x in x0.min(w - 1)..=x1 {
                    if grad[y * w + x] < best_g {
                        best_g = grad[y * w + x];
                        best = (x, y);
                    }
                }
            }
            markers.push(best);
            cx += s;
        }
        cy += s;
    }
    let k = regularity.max(0.0) / 100.0;
    let mut labels = vec![u32::MAX; w * h];
    let mut heap: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::new();
    for (i, &(x, y)) in markers.iter().enumerate() {
        heap.push(Reverse((0f32.to_bits(), (y * w + x) as u32, i as u32)));
    }
    while let Some(Reverse((_, idx, label))) = heap.pop() {
        let idx = idx as usize;
        if labels[idx] != u32::MAX {
            continue;
        }
        labels[idx] = label;
        let (x, y) = ((idx % w) as i64, (idx / w) as i64);
        for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let nidx = ny as usize * w + nx as usize;
            if labels[nidx] != u32::MAX {
                continue;
            }
            let (mx, my) = markers[label as usize];
            let d = ((nx as f32 - mx as f32).powi(2) + (ny as f32 - my as f32).powi(2)).sqrt() / s;
            let cost = grad[nidx] + k * d;
            heap.push(Reverse((cost.max(0.0).to_bits(), nidx as u32, label)));
        }
    }
    labels
}

/// Superpixels: regions of about `size` px found by `method` (`compactness` 1–100 trades edge
/// following for regular shapes; `iterations` applies to SLIC), each painted with its mean colour;
/// `outline` draws the borders in `outline_color`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn superpixels(src: &Image, out: Rect, ctx: &Ctx, method: SuperpixelMethod, size: f32, compactness: f32, iterations: u32, outline: bool, outline_color: [f32; 3]) -> Vec<f32> {
    let n = src.ch;
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let (k, sw, sh) = proxy_dims(w, h);
    let small = reduce(src, ctx, k, sw, sh);
    let step = (size.clamp(4.0, 1000.0) / k as f32).max(2.0);
    let labels = match method {
        SuperpixelMethod::Slic => slic(&small, sw, sh, step, compactness.clamp(1.0, 100.0), iterations),
        SuperpixelMethod::Waterpixels => waterpixels(&small, sw, sh, step, compactness.clamp(1.0, 100.0)),
    };
    let count = labels.iter().copied().max().map_or(0, |m| m as usize + 1).max(1);
    let label_at = |x: i32, y: i32| labels[(((y - src.rect.y0) as usize) / k).min(sh - 1) * sw + (((x - src.rect.x0) as usize) / k).min(sw - 1)];
    // Mean colour per region over the full-resolution pixels.
    let mut sums = vec![[0.0f64; 4]; count];
    for y in 0..h {
        for x in 0..w {
            let l = label_at(src.rect.x0 + x as i32, src.rect.y0 + y as i32) as usize;
            let c = rgba(ctx, &src.data[(y * w + x) * n..(y * w + x + 1) * n]);
            for i in 0..4 {
                sums[l][i] += f64::from(c[i]);
            }
        }
    }
    let mut counts = vec![0.0f64; count];
    for y in 0..h {
        for x in 0..w {
            counts[label_at(src.rect.x0 + x as i32, src.rect.y0 + y as i32) as usize] += 1.0;
        }
    }
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let l = label_at(x, y) as usize;
        let m = &sums[l];
        let c = counts[l].max(1.0);
        let edge = outline && {
            let (xr, yd) = (x + 1, y + 1);
            (xr < src.rect.x1 && label_at(xr, y) as usize != l) || (yd < src.rect.y1 && label_at(x, yd) as usize != l)
        };
        if edge {
            set_rgba(ctx, px, [outline_color[0], outline_color[1], outline_color[2], (m[3] / c) as f32]);
        } else {
            set_rgba(ctx, px, [(m[0] / c) as f32, (m[1] / c) as f32, (m[2] / c) as f32, (m[3] / c) as f32]);
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    /// 60 × 60: four flat colour quadrants.
    fn quadrants() -> (Image, Ctx) {
        let rect = Rect::new(0, 0, 60, 60);
        let mut i = Image::new(rect, 4);
        for y in 0..60 {
            for x in 0..60 {
                let c = match (x < 30, y < 30) {
                    (true, true) => [0.9, 0.1, 0.1],
                    (false, true) => [0.1, 0.9, 0.1],
                    (true, false) => [0.1, 0.1, 0.9],
                    _ => [0.9, 0.9, 0.1],
                };
                let k = ((y * 60 + x) * 4) as usize;
                i.data[k..k + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
            }
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    #[test]
    fn both_methods_follow_the_edges_between_flat_areas() {
        let (i, ctx) = quadrants();
        for method in [SuperpixelMethod::Slic, SuperpixelMethod::Waterpixels] {
            let o = superpixels(&i, i.rect, &ctx, method, 15.0, 10.0, 6, false, [0.0; 3]);
            // Flat quadrants keep their colours exactly: no region straddles an edge.
            let wrong = o.chunks(4).zip(i.data.chunks(4)).filter(|(p, q)| (p[0] - q[0]).abs() + (p[1] - q[1]).abs() + (p[2] - q[2]).abs() > 0.3).count();
            assert!(wrong < 300, "{method:?}: {wrong} of 3600 pixels took a neighbour's colour (a single region would be about 2700)");
            assert_eq!(o, superpixels(&i, i.rect, &ctx, method, 15.0, 10.0, 6, false, [0.0; 3]));
        }
    }

    #[test]
    fn regions_average_a_noisy_area_and_can_be_outlined() {
        let rect = Rect::new(0, 0, 48, 48);
        let mut i = Image::new(rect, 4);
        for y in 0..48 {
            for x in 0..48 {
                let v = 0.4 + (((x * 7919 + y * 104_729) % 251) as f32 / 251.0) * 0.2;
                let k = ((y * 48 + x) * 4) as usize;
                i.data[k..k + 4].copy_from_slice(&[v, v, v, 1.0]);
            }
        }
        let ctx = Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true };
        let o = superpixels(&i, rect, &ctx, SuperpixelMethod::Slic, 12.0, 80.0, 5, false, [0.0; 3]);
        let distinct = |d: &[f32]| d.chunks(4).map(|p| (p[0] * 10_000.0) as i32).collect::<std::collections::BTreeSet<_>>().len();
        assert!(distinct(&o) < 40 && distinct(&i.data) > 40, "{} vs {}", distinct(&o), distinct(&i.data));
        let outlined = superpixels(&i, rect, &ctx, SuperpixelMethod::Slic, 12.0, 80.0, 5, true, [1.0, 0.0, 0.0]);
        let red = outlined.chunks(4).filter(|p| p[0] == 1.0 && p[1] == 0.0).count();
        assert!(red > 100 && red < 1200, "{red} outline pixels");
    }

    #[test]
    fn the_region_count_follows_the_size() {
        let (i, ctx) = quadrants();
        let regions = |size: f32| {
            let (k, sw, sh) = proxy_dims(60, 60);
            let small = reduce(&i, &ctx, k, sw, sh);
            let labels = slic(&small, sw, sh, size, 10.0, 4);
            labels.iter().collect::<std::collections::BTreeSet<_>>().len()
        };
        let (coarse, fine) = (regions(20.0), regions(8.0));
        assert!(fine > coarse * 3, "{fine} vs {coarse}");
        assert!((7..=12).contains(&coarse), "about (60/20)² = 9 regions: {coarse}");
    }

    #[test]
    fn picture_smaller_than_a_cell_is_one_region() {
        let labels = slic(&vec![[0.5, 0.5, 0.5]; 25], 5, 5, 24.0, 10.0, 4);
        assert!(labels.iter().all(|l| *l == 0));
    }
}
