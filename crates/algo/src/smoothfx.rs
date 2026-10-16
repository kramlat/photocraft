//! Smoothing and distance: Edge-Preserving Mean, Smooth Edges and Distance Map.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::fxutil::{luma, rgba, set_rgba, xy};
use crate::image::Image;

/// What Distance Map measures from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DistanceSource {
    /// Pixels brighter than the threshold are "inside".
    #[default]
    Brightness,
    /// Pixels more opaque than the threshold are "inside".
    Alpha,
}

fn colour_distance(a: &[f32], b: &[f32], colours: usize) -> f32 {
    a.iter().zip(b).take(colours).map(|(x, y)| (x - y).abs()).sum()
}

/// How far Edge-Preserving Mean reaches, for the tile margin.
pub(crate) fn mean_halo(radius: f32, iterations: u32) -> i32 {
    (radius.clamp(1.0, 50.0).ceil() as i32) * iterations.clamp(1, 20) as i32 + 1
}

/// Edge-Preserving Mean (symmetric nearest neighbour): for each pixel and each of four
/// directions, of the two neighbours `radius` px either side the one closer in colour joins the
/// average, so flat areas smooth while edges keep their sides. Repeated `iterations` times.
pub(crate) fn symmetric_mean(src: &Image, out: Rect, ctx: &Ctx, radius: f32, iterations: u32) -> Vec<f32> {
    let n = src.ch;
    let colours = if ctx.alpha { n - 1 } else { n };
    let r = radius.clamp(1.0, 50.0).round() as i32;
    let (w, h) = (src.rect.width() as i32, src.rect.height() as i32);
    let dirs = [(1, 0), (0, 1), (1, 1), (1, -1)];
    let mut cur = src.data.clone();
    for _ in 0..iterations.clamp(1, 20) {
        let mut next = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let at = |xx: i32, yy: i32| {
                    let (xx, yy) = (xx.clamp(0, w - 1), yy.clamp(0, h - 1));
                    let k = ((yy * w + xx) as usize) * n;
                    &cur[k..k + n]
                };
                let centre = at(x, y);
                let mut sum = vec![0.0f32; n];
                sum[..n].copy_from_slice(&centre[..n]);
                for (dx, dy) in dirs {
                    let (a, b) = (at(x + dx * r, y + dy * r), at(x - dx * r, y - dy * r));
                    let pick = if colour_distance(centre, a, colours) <= colour_distance(centre, b, colours) { a } else { b };
                    for k in 0..n {
                        sum[k] += pick[k];
                    }
                }
                let k = ((y * w + x) as usize) * n;
                for c in 0..n {
                    next[k + c] = sum[c] / 5.0;
                }
            }
        }
        cur = next;
    }
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    for y in out.y0..out.y1 {
        for x in out.x0..out.x1 {
            let k = (((y - src.rect.y0) * w + (x - src.rect.x0)) as usize) * n;
            res.extend_from_slice(&cur[k..k + n]);
        }
    }
    res
}

/// Smooth Edges: removes the stair-steps from hard, aliased edges (pixel art, 1-bit drawings).
/// Where two neighbours at right angles match the corner between them (within `tolerance` %)
/// and the other two differ, the pixel is mixed toward them by `strength` %.
pub(crate) fn smooth_edges(src: &Image, out: Rect, ctx: &Ctx, tolerance: f32, strength: f32) -> Vec<f32> {
    let n = src.ch;
    let colours = if ctx.alpha { n - 1 } else { n };
    let tol = (tolerance / 100.0).clamp(0.0, 1.0) * colours as f32 * 0.5;
    let k = (strength / 100.0).clamp(0.0, 1.0) * 0.5;
    let same = |a: &[f32], b: &[f32]| colour_distance(a, b, colours) <= tol;
    let mut res = src.crop(out);
    let w = out.width() as usize;
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = (out.x0 + (i % w) as i32, out.y0 + (i / w) as i32);
        let at = |dx: i32, dy: i32| {
            let (xx, yy) = ((x + dx).clamp(src.rect.x0, src.rect.x1 - 1), (y + dy).clamp(src.rect.y0, src.rect.y1 - 1));
            src.px(xx, yy)
        };
        let (up, down, left, right) = (at(0, -1), at(0, 1), at(-1, 0), at(1, 0));
        let own = src.px(x, y).to_vec();
        // A staircase step: two adjacent neighbours agree with each other but not with the
        // neighbours opposite them, so this pixel sits in the corner of the step.
        let mut blend: Option<&[f32]> = None;
        for (a, b, a_opp, b_opp) in [(up, left, right, down), (up, right, left, down), (down, left, right, up), (down, right, left, up)] {
            if same(a, b) && !same(a, a_opp) && !same(b, b_opp) && !same(&own, a) {
                blend = Some(a);
                break;
            }
        }
        if let Some(b) = blend {
            for c in 0..n {
                px[c] = own[c] + (b[c] - own[c]) * k;
            }
        }
    }
    res
}

/// Squared distance transform of a boolean grid (Felzenszwalb & Huttenlocher): for every cell
/// the squared distance to the nearest cell where `feature` is true.
fn distance_squared(feature: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1.0e12;
    let mut f: Vec<f32> = feature.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    let line = |vals: &mut [f32]| {
        let n = vals.len();
        let mut v = vec![0usize; n];
        let mut z = vec![0.0f32; n + 1];
        let mut k = 0usize;
        z[0] = -INF;
        z[1] = INF;
        let sep = |a: usize, b: usize, vals: &[f32]| ((vals[a] + (a * a) as f32) - (vals[b] + (b * b) as f32)) / (2.0 * a as f32 - 2.0 * b as f32);
        for q in 1..n {
            let mut s = sep(q, v[k], vals);
            while k > 0 && s <= z[k] {
                k -= 1;
                s = sep(q, v[k], vals);
            }
            if s <= z[k] {
                v[k] = q;
                z[k] = -INF;
                z[k + 1] = INF;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
            }
        }
        let old = vals.to_vec();
        k = 0;
        for (q, slot) in vals.iter_mut().enumerate() {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let d = q as f32 - v[k] as f32;
            *slot = d * d + old[v[k]];
        }
    };
    for x in 0..w {
        let mut col: Vec<f32> = (0..h).map(|y| f[y * w + x]).collect();
        line(&mut col);
        for y in 0..h {
            f[y * w + x] = col[y];
        }
    }
    for y in 0..h {
        line(&mut f[y * w..(y + 1) * w]);
    }
    f
}

/// Distance Map: every pixel becomes its distance to the nearest edge of the shape (inside and
/// outside are told apart by `threshold` %), shown 0 (on the edge) to 1 (at `max_distance` px
/// or more); `outside` draws only the outside, otherwise inside and outside both count.
pub(crate) fn distance_map(src: &Image, out: Rect, ctx: &Ctx, source: DistanceSource, threshold: f32, max_distance: f32, invert: bool) -> Vec<f32> {
    let n = src.ch;
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let thr = (threshold / 100.0).clamp(0.0, 1.0);
    let inside: Vec<bool> = src.data.chunks_exact(n).map(|px| match source {
        DistanceSource::Brightness => luma(ctx, px) > thr,
        DistanceSource::Alpha => rgba(ctx, px)[3] > thr,
    }).collect();
    let outside: Vec<bool> = inside.iter().map(|b| !b).collect();
    // For an inside pixel: distance to the nearest outside pixel, and vice versa.
    let to_outside = distance_squared(&outside, w, h);
    let to_inside = distance_squared(&inside, w, h);
    let reach = max_distance.clamp(1.0, 5000.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let k = (y - src.rect.y0) as usize * w + (x - src.rect.x0) as usize;
        let d = if inside[k] { to_outside[k].sqrt() } else { to_inside[k].sqrt() };
        let mut v = (d / reach).clamp(0.0, 1.0);
        if invert {
            v = 1.0 - v;
        }
        let a = rgba(ctx, px)[3];
        set_rgba(ctx, px, [v, v, v, a]);
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

    #[test]
    fn the_symmetric_mean_smooths_noise_but_keeps_a_step() {
        // A step edge with noise on both sides.
        let (i, ctx) = img(32, 16, |x, y| {
            let noise = (((x * 7 + y * 13) % 5) as f32 - 2.0) * 0.02;
            (if x < 16 { 0.2 } else { 0.8 }) + noise
        });
        let o = symmetric_mean(&i, i.rect, &ctx, 2.0, 2);
        let v = |x: usize, y: usize| o[(y * 32 + x) * 4];
        let spread = |lo: usize, hi: usize| {
            let vals: Vec<f32> = (lo..hi).flat_map(|x| (4..12).map(move |y| (x, y))).map(|(x, y)| v(x, y)).collect();
            vals.iter().copied().fold(0.0, f32::max) - vals.iter().copied().fold(1.0, f32::min)
        };
        let before = |lo: usize, hi: usize| {
            let vals: Vec<f32> = (lo..hi).flat_map(|x| (4..12).map(move |y| (x, y))).map(|(x, y)| i.data[(y * 32 + x) * 4]).collect();
            vals.iter().copied().fold(0.0, f32::max) - vals.iter().copied().fold(1.0, f32::min)
        };
        assert!(spread(3, 12) < before(3, 12), "noise is reduced");
        assert!((v(14, 8) - 0.2).abs() < 0.08 && (v(17, 8) - 0.8).abs() < 0.08, "the step survives: {} {}", v(14, 8), v(17, 8));
    }

    #[test]
    fn smoothing_edges_softens_a_diagonal_staircase_only() {
        let (i, ctx) = img(12, 12, |x, y| if x > y { 1.0 } else { 0.0 });
        let o = smooth_edges(&i, i.rect, &ctx, 5.0, 100.0);
        let changed = o.chunks(4).zip(i.data.chunks(4)).filter(|(p, q)| p != q).count();
        assert!(changed > 5, "{changed}");
        assert!(o.chunks(4).all(|p| (0.0..=1.0).contains(&p[0])));
        let (flat, c2) = img(8, 8, |_, _| 0.4);
        assert_eq!(smooth_edges(&flat, flat.rect, &c2, 5.0, 100.0), flat.data);
        let (bar, c3) = img(12, 12, |x, _| if x > 5 { 1.0 } else { 0.0 });
        assert_eq!(smooth_edges(&bar, bar.rect, &c3, 5.0, 100.0), bar.data, "a straight edge has no steps");
    }

    #[test]
    fn the_distance_map_measures_to_the_nearest_edge() {
        // A white square (x, y in 8..24) on black, 32 × 32.
        let (i, ctx) = img(32, 32, |x, y| if (8..24).contains(&x) && (8..24).contains(&y) { 1.0 } else { 0.0 });
        let o = distance_map(&i, i.rect, &ctx, DistanceSource::Brightness, 50.0, 8.0, false);
        let at = |x: usize, y: usize| o[(y * 32 + x) * 4];
        assert!((at(16, 16) - 1.0).abs() < 1e-6, "deep inside: 8 px from the edge: {}", at(16, 16));
        assert!((at(8, 16) - 1.0 / 8.0).abs() < 0.01, "on the boundary: {}", at(8, 16));
        assert!((at(4, 16) - 4.0 / 8.0).abs() < 0.07, "outside, 4 px away: {}", at(4, 16));
        assert!((at(0, 0) - 1.0).abs() < 1e-6, "far outside saturates");
        let inv = distance_map(&i, i.rect, &ctx, DistanceSource::Brightness, 50.0, 8.0, true);
        assert!((inv[(16 * 32 + 16) * 4]).abs() < 1e-6);
    }

    #[test]
    fn the_distance_transform_matches_brute_force() {
        let (w, h) = (23usize, 17usize);
        let feature: Vec<bool> = (0..w * h).map(|i| (i * 7919) % 31 == 0).collect();
        let fast = distance_squared(&feature, w, h);
        for y in 0..h {
            for x in 0..w {
                let brute = feature.iter().enumerate().filter(|(_, f)| **f).map(|(k, _)| {
                    let (fx, fy) = ((k % w) as f32, (k / w) as f32);
                    (fx - x as f32).powi(2) + (fy - y as f32).powi(2)
                }).fold(f32::MAX, f32::min);
                assert!((fast[y * w + x] - brute).abs() < 1e-3, "({x},{y}): {} vs {brute}", fast[y * w + x]);
            }
        }
    }
}
