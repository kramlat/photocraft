//! Tone mappers and curvature smoothing: Mean Curvature Blur, Reinhard Tone Map, Fattal Tone Map
//! and Contrast Domain Tone Map. The tone mappers compress the range of a high-dynamic-range
//! picture (values may exceed 1 in 32-bit documents) into one a screen can show, working on linear
//! luminance and putting the colour back.
//!
//! * Reinhard: E. Reinhard, K. Devlin, *Dynamic Range Reduction Inspired by Photoreceptor
//!   Physiology*, IEEE TVCG 11(1), 2005.
//! * Fattal: R. Fattal, D. Lischinski, M. Werman, *Gradient Domain High Dynamic Range
//!   Compression*, SIGGRAPH 2002 — gradients of log luminance attenuated over a pyramid of scales,
//!   then integrated by solving Poisson's equation (here with a cosine transform).
//! * Contrast domain: in the spirit of R. Mantiuk, K. Myszkowski, H.-P. Seidel, *A Perceptual
//!   Framework for Contrast Processing of High Dynamic Range Images*, ACM TAP 3(3), 2006 —
//!   contrasts at every scale of log luminance are compressed and recombined. This implementation
//!   uses a blur pyramid and a soft contrast curve, not the paper's constrained optimisation.

use photocraft_geom::Rect;
use rustfft::{FftPlanner, num_complex::Complex};

use crate::Ctx;
use crate::colorops::{linear_to_srgb, srgb_to_linear};
use crate::fxutil::{gauss_blur_n, rgba, set_rgba, xy};
use crate::image::Image;

/// How far Mean Curvature Blur reaches, for the tile margin.
pub(crate) fn curvature_halo(iterations: u32) -> i32 {
    iterations.clamp(1, 200) as i32 + 1
}

/// Mean Curvature Blur: every colour channel flows by its level-set curvature, which rounds off
/// corners and removes small features but leaves long straight edges where they are.
pub(crate) fn mean_curvature_blur(src: &Image, out: Rect, ctx: &Ctx, iterations: u32) -> Vec<f32> {
    let n = src.ch;
    let colours = if ctx.alpha { n - 1 } else { n };
    let (w, h) = (src.rect.width() as i32, src.rect.height() as i32);
    let mut cur = src.data.clone();
    let dt = 0.2f32;
    for _ in 0..iterations.clamp(1, 200) {
        let mut next = cur.clone();
        for c in 0..colours {
            let at = |x: i32, y: i32| cur[(((y.clamp(0, h - 1)) * w + x.clamp(0, w - 1)) as usize) * n + c];
            for y in 0..h {
                for x in 0..w {
                    let (ux, uy) = ((at(x + 1, y) - at(x - 1, y)) / 2.0, (at(x, y + 1) - at(x, y - 1)) / 2.0);
                    let u = at(x, y);
                    let (uxx, uyy) = (at(x + 1, y) - 2.0 * u + at(x - 1, y), at(x, y + 1) - 2.0 * u + at(x, y - 1));
                    let uxy = (at(x + 1, y + 1) - at(x - 1, y + 1) - at(x + 1, y - 1) + at(x - 1, y - 1)) / 4.0;
                    let g2 = ux * ux + uy * uy;
                    if g2 > 1e-8 {
                        let flow = (uxx * uy * uy - 2.0 * ux * uy * uxy + uyy * ux * ux) / g2;
                        next[((y * w + x) as usize) * n + c] = u + dt * flow;
                    }
                }
            }
        }
        cur = next;
    }
    let cols = w as usize;
    let mut res = Vec::with_capacity(out.width() as usize * out.height() as usize * n);
    for y in out.y0..out.y1 {
        let row = ((y - src.rect.y0) as usize * cols + (out.x0 - src.rect.x0) as usize) * n;
        res.extend_from_slice(&cur[row..row + out.width() as usize * n]);
    }
    res
}

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Linear RGB and luminance of every pixel of `src`.
fn linear_pixels(src: &Image, ctx: &Ctx) -> (Vec<[f32; 3]>, Vec<f32>) {
    let rgb: Vec<[f32; 3]> = src.data.chunks_exact(src.ch).map(|px| {
        let c = rgba(ctx, px);
        [srgb_to_linear(c[0]).max(0.0), srgb_to_linear(c[1]).max(0.0), srgb_to_linear(c[2]).max(0.0)]
    }).collect();
    let y: Vec<f32> = rgb.iter().map(|c| c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]).collect();
    (rgb, y)
}

/// Writes the part of the tone-mapped linear image inside `out` (alpha kept).
fn finish(src: &Image, out: Rect, ctx: &Ctx, linear: &[[f32; 3]]) -> Vec<f32> {
    let n = src.ch;
    let w = src.rect.width() as usize;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let c = linear[(y - src.rect.y0) as usize * w + (x - src.rect.x0) as usize];
        let a = rgba(ctx, px)[3];
        set_rgba(ctx, px, [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2]), a]);
    }
    res
}

/// Rebuilds colour from the original linear RGB `rgb` with luminance `y` changed to `y_new`;
/// `saturation` below 1 pulls colours toward grey, above 1 pushes them out.
fn recolour(rgb: &[[f32; 3]], y: &[f32], y_new: &[f32], saturation: f32) -> Vec<[f32; 3]> {
    rgb.iter().zip(y).zip(y_new).map(|((c, y), yn)| {
        if *y <= 1e-9 {
            return [*yn; 3];
        }
        let f = |v: f32| ((v / y).max(0.0).powf(saturation) * yn).clamp(0.0, 1.0);
        [f(c[0]), f(c[1]), f(c[2])]
    }).collect()
}

/// Reinhard Tone Map: each channel `I` becomes `I / (I + (f·Ia)^m)`, where the adaptation level
/// `Ia` mixes the pixel's own channel and luminance (`chromatic`, 0–100 %) and then the pixel's
/// and the whole picture's average (`light`, 0–100 %), `m` follows how contrasty the picture is
/// and `f = exp(−brightness)`.
pub(crate) fn reinhard(src: &Image, out: Rect, ctx: &Ctx, brightness: f32, chromatic: f32, light: f32) -> Vec<f32> {
    let (rgb, y) = linear_pixels(src, ctx);
    let count = y.len().max(1) as f32;
    let eps = 1e-6f32;
    let log_avg = (y.iter().map(|v| (v + eps).ln()).sum::<f32>() / count).exp();
    let (lo, hi) = y.iter().fold((f32::MAX, 0.0f32), |(l, h), v| (l.min(*v + eps), h.max(*v + eps)));
    let key = if hi > lo { ((log_avg.ln() - lo.ln()) / (hi.ln() - lo.ln())).clamp(0.0, 1.0) } else { 0.5 };
    let m = 0.3 + 0.7 * key.powf(1.4);
    let f = (-brightness.clamp(-8.0, 8.0)).exp();
    let (c, l) = ((chromatic / 100.0).clamp(0.0, 1.0), (light / 100.0).clamp(0.0, 1.0));
    let mean_rgb = {
        let mut acc = [0.0f32; 3];
        for p in &rgb {
            for k in 0..3 {
                acc[k] += p[k];
            }
        }
        acc.map(|v| v / count)
    };
    let mapped: Vec<[f32; 3]> = rgb.iter().zip(&y).map(|(p, yy)| {
        let mut o = [0.0f32; 3];
        for k in 0..3 {
            let local = c * p[k] + (1.0 - c) * yy;
            let ia = l * local + (1.0 - l) * mean_rgb[k];
            o[k] = p[k] / (p[k] + (f * ia).max(1e-9).powf(m));
        }
        o
    }).collect();
    // Normalise so the brightest tone reaches white.
    let peak = mapped.iter().flat_map(|c| c.iter().copied()).fold(0.0f32, f32::max).max(1e-6);
    let scaled: Vec<[f32; 3]> = mapped.iter().map(|c| [c[0] / peak, c[1] / peak, c[2] / peak]).collect();
    finish(src, out, ctx, &scaled)
}

/// Downsamples `values` (`w × h`) by whole-pixel box averaging to fit `longest` px; returns the
/// small grid, its size and the factor.
fn shrink(values: &[f32], w: usize, h: usize, longest: usize) -> (Vec<f32>, usize, usize, usize) {
    let k = w.max(h).div_ceil(longest).max(1);
    if k == 1 {
        return (values.to_vec(), w, h, 1);
    }
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let mut out = vec![0.0f32; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let (mut sum, mut cnt) = (0.0f32, 0.0f32);
            for yy in y * k..((y + 1) * k).min(h) {
                for xx in x * k..((x + 1) * k).min(w) {
                    sum += values[yy * w + xx];
                    cnt += 1.0;
                }
            }
            out[y * sw + x] = sum / cnt;
        }
    }
    (out, sw, sh, k)
}

/// Bilinear sample of a small grid at full-resolution pixel `(x, y)`.
fn upsample(grid: &[f32], sw: usize, sh: usize, k: usize, x: usize, y: usize) -> f32 {
    let (fx, fy) = ((x as f32 + 0.5) / k as f32 - 0.5, (y as f32 + 0.5) / k as f32 - 0.5);
    let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
    let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
    let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
    let (x0, y0) = (x0.min(sw - 1), y0.min(sh - 1));
    let top = grid[y0 * sw + x0] * (1.0 - tx) + grid[y0 * sw + x1] * tx;
    let bot = grid[y1 * sw + x0] * (1.0 - tx) + grid[y1 * sw + x1] * tx;
    top * (1.0 - ty) + bot * ty
}

/// Applies a log-luminance gain computed on a reduced grid to the full picture, then stretches
/// the result so its 0.5–99.5 percentile spans 0–1.
#[allow(clippy::too_many_arguments)]
fn apply_gain(src: &Image, out: Rect, ctx: &Ctx, rgb: &[[f32; 3]], y: &[f32], gain: &[f32], dims: (usize, usize, usize), saturation: f32) -> Vec<f32> {
    let (sw, sh, k) = dims;
    let w = src.rect.width() as usize;
    let y_new: Vec<f32> = y.iter().enumerate().map(|(i, v)| v * upsample(gain, sw, sh, k, i % w, i / w).exp()).collect();
    let mut sorted = y_new.clone();
    sorted.sort_by(f32::total_cmp);
    let pick = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let (lo, hi) = (pick(0.005), pick(0.995).max(pick(0.005) + 1e-6));
    let stretched: Vec<f32> = y_new.iter().map(|v| ((v - lo) / (hi - lo)).clamp(0.0, 1.0)).collect();
    finish(src, out, ctx, &recolour(rgb, y, &stretched, saturation.clamp(0.0, 2.0)))
}

/// Solves `Δu = div` on a `w × h` grid with zero-flux borders (a cosine transform by mirroring
/// into a `2w × 2h` periodic grid and using the FFT). The constant of `u` is arbitrary (0 mean).
fn poisson_neumann(div: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (bw, bh) = (2 * w, 2 * h);
    let mut grid = vec![Complex::new(0.0f32, 0.0); bw * bh];
    for y in 0..bh {
        let sy = if y < h { y } else { 2 * h - 1 - y };
        for x in 0..bw {
            let sx = if x < w { x } else { 2 * w - 1 - x };
            grid[y * bw + x] = Complex::new(div[sy * w + sx], 0.0);
        }
    }
    let mut planner = FftPlanner::<f32>::new();
    let (row_fwd, row_inv) = (planner.plan_fft_forward(bw), planner.plan_fft_inverse(bw));
    let (col_fwd, col_inv) = (planner.plan_fft_forward(bh), planner.plan_fft_inverse(bh));
    let transform = |g: &mut Vec<Complex<f32>>, rows: &std::sync::Arc<dyn rustfft::Fft<f32>>, cols: &std::sync::Arc<dyn rustfft::Fft<f32>>| {
        for row in g.chunks_exact_mut(bw) {
            rows.process(row);
        }
        let mut col = vec![Complex::new(0.0f32, 0.0); bh];
        for x in 0..bw {
            for y in 0..bh {
                col[y] = g[y * bw + x];
            }
            cols.process(&mut col);
            for y in 0..bh {
                g[y * bw + x] = col[y];
            }
        }
    };
    transform(&mut grid, &row_fwd, &col_fwd);
    for ky in 0..bh {
        for kx in 0..bw {
            let lambda = 2.0 * (std::f32::consts::TAU * kx as f32 / bw as f32).cos() + 2.0 * (std::f32::consts::TAU * ky as f32 / bh as f32).cos() - 4.0;
            let c = &mut grid[ky * bw + kx];
            *c = if lambda.abs() < 1e-9 { Complex::new(0.0, 0.0) } else { *c / lambda };
        }
    }
    transform(&mut grid, &row_inv, &col_inv);
    let norm = 1.0 / (bw * bh) as f32;
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| grid[y * bw + x].re * norm).collect()
}

/// Fattal Tone Map: gradients of log luminance are shrunk where large (`alpha` sets the size of
/// gradient left alone, `beta` < 1 how strongly bigger ones are reduced) over a pyramid of scales,
/// and the result integrated back. Solved on a picture reduced to at most `PROXY` px and applied
/// as a smooth gain, so the cost doesn't grow with the megapixels.
pub(crate) fn fattal(src: &Image, out: Rect, ctx: &Ctx, alpha: f32, beta: f32, saturation: f32) -> Vec<f32> {
    const PROXY: usize = 768;
    let (rgb, y) = linear_pixels(src, ctx);
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let log_y: Vec<f32> = y.iter().map(|v| (v + 1e-6).ln()).collect();
    let (small, sw, sh, k) = shrink(&log_y, w, h, PROXY);
    // Gaussian pyramid of log luminance, level 0 the proxy.
    let mut levels: Vec<(Vec<f32>, usize, usize)> = vec![(small.clone(), sw, sh)];
    while levels.last().is_some_and(|(_, lw, lh)| (*lw).min(*lh) > 32) {
        let (prev, pw, ph) = levels.last().cloned().unwrap_or_default();
        let mut blurred = prev.clone();
        gauss_blur_n(&mut blurred, pw, ph, 1, 1.0);
        let (next, nw, nh, _) = shrink(&blurred, pw, ph, pw.max(ph).div_ceil(2));
        levels.push((next, nw, nh));
    }
    let beta = beta.clamp(0.5, 1.0);
    // Attenuation per level, combined from the coarsest to the finest.
    let mut phi: Vec<f32> = Vec::new();
    let mut phi_dims = (0usize, 0usize);
    for (lvl, (vals, lw, lh)) in levels.iter().enumerate().rev() {
        let grad = |x: usize, y: usize| {
            let dx = vals[y * lw + (x + 1).min(lw - 1)] - vals[y * lw + x];
            let dy = vals[(y + 1).min(lh - 1) * lw + x] - vals[y * lw + x];
            (dx * dx + dy * dy).sqrt()
        };
        let mean = (0..*lh).flat_map(|y| (0..*lw).map(move |x| (x, y))).map(|(x, y)| grad(x, y)).sum::<f32>() / (lw * lh) as f32;
        let a = (alpha.clamp(0.05, 4.0) * 0.1 * mean).max(1e-6);
        let mut cur = vec![1.0f32; lw * lh];
        for y in 0..*lh {
            for x in 0..*lw {
                let g = grad(x, y).max(1e-6);
                let own = (g / a).powf(beta - 1.0);
                let coarser = if phi.is_empty() {
                    1.0
                } else {
                    let (pw, ph) = phi_dims;
                    let (cx, cy) = ((x / 2).min(pw - 1), (y / 2).min(ph - 1));
                    phi[cy * pw + cx]
                };
                cur[y * lw + x] = own * coarser;
            }
        }
        let _ = lvl;
        phi = cur;
        phi_dims = (*lw, *lh);
    }
    // Attenuated gradient field of the proxy, its divergence, and the Poisson solution.
    let (vals, lw, lh) = (&levels[0].0, levels[0].1, levels[0].2);
    let mut gx = vec![0.0f32; lw * lh];
    let mut gy = vec![0.0f32; lw * lh];
    for y in 0..lh {
        for x in 0..lw {
            let p = phi[y * lw + x];
            if x + 1 < lw {
                gx[y * lw + x] = (vals[y * lw + x + 1] - vals[y * lw + x]) * p;
            }
            if y + 1 < lh {
                gy[y * lw + x] = (vals[(y + 1) * lw + x] - vals[y * lw + x]) * p;
            }
        }
    }
    let mut div = vec![0.0f32; lw * lh];
    for y in 0..lh {
        for x in 0..lw {
            let dx = gx[y * lw + x] - if x > 0 { gx[y * lw + x - 1] } else { 0.0 };
            let dy = gy[y * lw + x] - if y > 0 { gy[(y - 1) * lw + x] } else { 0.0 };
            div[y * lw + x] = dx + dy;
        }
    }
    let solved = poisson_neumann(&div, lw, lh);
    // The log-gain per proxy pixel: new log luminance (up to a constant) minus the old.
    let gain: Vec<f32> = solved.iter().zip(vals.iter()).map(|(s, v)| s - v).collect();
    apply_gain(src, out, ctx, &rgb, &y, &gain, (sw, sh, k), saturation)
}

/// Contrast Domain Tone Map: log luminance is split into detail bands at doubling scales plus a
/// base; each band's contrast goes through a soft compression curve (`detail` 0–200 % scales the
/// bands' gain), the base's overall contrast is reduced to `contrast` %, and the parts recombined.
pub(crate) fn contrast_domain(src: &Image, out: Rect, ctx: &Ctx, contrast: f32, detail: f32, saturation: f32) -> Vec<f32> {
    const PROXY: usize = 1024;
    let (rgb, y) = linear_pixels(src, ctx);
    let (w, h) = (src.rect.width() as usize, src.rect.height() as usize);
    let log_y: Vec<f32> = y.iter().map(|v| (v + 1e-6).ln()).collect();
    let (small, sw, sh, k) = shrink(&log_y, w, h, PROXY);
    let scales = ((sw.max(sh) as f32 / 8.0).log2().floor() as u32).clamp(2, 8);
    let g = (detail / 100.0).clamp(0.0, 2.0);
    let base_k = (contrast / 100.0).clamp(0.05, 1.0);
    let mut prev = small.clone();
    let mut processed = vec![0.0f32; small.len()];
    for level in 0..scales {
        let mut cur = prev.clone();
        gauss_blur_n(&mut cur, sw, sh, 1, (1u32 << level) as f32);
        for i in 0..cur.len() {
            let band = prev[i] - cur[i];
            // Soft compression: small contrasts pass (scaled by the gain), big ones saturate.
            let m = 0.35f32;
            processed[i] += g * band / (1.0 + (band.abs() / m).powi(2)).sqrt();
        }
        prev = cur;
    }
    let mean = prev.iter().sum::<f32>() / prev.len() as f32;
    let gain: Vec<f32> = prev.iter().zip(&processed).zip(&small).map(|((base, det), orig)| ((mean + (base - mean) * base_k) + det) - orig).collect();
    apply_gain(src, out, ctx, &rgb, &y, &gain, (sw, sh, k), saturation)
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

    /// A wide-range picture in display-encoded floats: the left half far brighter than the right,
    /// each with a small checker of detail on top.
    fn hdr(x: i32, y: i32) -> f32 {
        let base = if x < 32 { 6.0 } else { 0.02 };
        base * if (x / 4 + y / 4) % 2 == 0 { 1.0 } else { 0.7 }
    }

    fn range(o: &[f32], x0: usize, x1: usize, w: usize, h: usize) -> (f32, f32) {
        let vals: Vec<f32> = (0..h).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| o[(y * w + x) * 4]).collect();
        (vals.iter().copied().fold(1.0, f32::min), vals.iter().copied().fold(0.0, f32::max))
    }

    #[test]
    fn curvature_flow_removes_noise_and_keeps_a_straight_edge() {
        let (i, ctx) = img(32, 32, |x, y| (if x < 16 { 0.2 } else { 0.8 }) + (((x * 7 + y * 11) % 5) as f32 - 2.0) * 0.02);
        let o = mean_curvature_blur(&i, i.rect, &ctx, 20);
        let spread = |d: &[f32]| {
            let v: Vec<f32> = (2..12).flat_map(|x| (4..28).map(move |y| (x, y))).map(|(x, y)| d[(y * 32 + x) * 4]).collect();
            v.iter().copied().fold(0.0, f32::max) - v.iter().copied().fold(1.0, f32::min)
        };
        assert!(spread(&o) < spread(&i.data) * 0.7, "{} vs {}", spread(&o), spread(&i.data));
        let at = |x: usize, y: usize| o[(y * 32 + x) * 4];
        assert!(at(14, 16) < 0.35 && at(17, 16) > 0.65, "the edge stays sharp: {} {}", at(14, 16), at(17, 16));
        let (corner, c2) = img(24, 24, |x, y| if x < 12 && y < 12 { 1.0 } else { 0.0 });
        let r = mean_curvature_blur(&corner, corner.rect, &c2, 15);
        assert!(r[(11 * 24 + 11) * 4] < 1.0, "the corner is rounded off");
        assert_eq!(mean_curvature_blur(&corner, corner.rect, &c2, 15), r);
    }

    #[test]
    fn reinhard_maps_a_wide_range_into_the_display_range_in_order() {
        let (i, ctx) = img(64, 16, hdr);
        let o = reinhard(&i, i.rect, &ctx, 0.0, 0.0, 100.0);
        assert!(o.iter().all(|v| (0.0..=1.0).contains(v)));
        let (dark, bright) = (range(&o, 40, 60, 64, 16), range(&o, 2, 28, 64, 16));
        assert!(bright.0 > dark.1, "the bright half stays above the dark half: {bright:?} {dark:?}");
        let before = (6.0f32 / 0.02f32).ln();
        let after = (bright.1 / dark.0.max(1e-3)).ln();
        assert!(after < before * 0.6, "the range shrinks: {after} vs {before}");
        let brighter = reinhard(&i, i.rect, &ctx, 3.0, 0.0, 100.0);
        assert!(range(&brighter, 40, 60, 64, 16).1 > dark.1, "a higher brightness lifts the shadows");
    }

    #[test]
    fn fattal_compresses_the_range_but_keeps_local_detail() {
        let (i, ctx) = img(64, 64, hdr);
        let o = fattal(&i, i.rect, &ctx, 1.0, 0.8, 1.0);
        assert!(o.chunks(4).all(|p| (0.0..=1.0).contains(&p[0]) && p[3] == 1.0));
        let (dark, bright) = (range(&o, 40, 60, 64, 64), range(&o, 2, 28, 64, 64));
        // The dark half is lifted well off black (it was 300× darker than the bright half).
        assert!(dark.1 > 0.12 && bright.1 > dark.1, "dark {dark:?} bright {bright:?}");
        // The checker's contrast survives in both halves (bright squares stay brighter).
        let px = |x: usize, y: usize| o[(y * 64 + x) * 4];
        assert!(px(2, 2) > px(6, 2) && px(40, 2) > px(36, 2), "{} {} / {} {}", px(2, 2), px(6, 2), px(40, 2), px(36, 2));
        assert_eq!(o, fattal(&i, i.rect, &ctx, 1.0, 0.8, 1.0));
    }

    #[test]
    fn the_poisson_solver_integrates_a_known_field() {
        // u(x, y) = cos(pi x / w) has zero-flux borders; its Laplacian is -(pi / w)^2 u (smooth, discrete-ish).
        let (w, h) = (32usize, 8usize);
        let u: Vec<f32> = (0..h).flat_map(|_| (0..w).map(|x| (std::f32::consts::PI * (x as f32 + 0.5) / w as f32).cos())).collect();
        let mut div = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let at = |xx: i32, yy: i32| u[(yy.clamp(0, h as i32 - 1) as usize) * w + xx.clamp(0, w as i32 - 1) as usize];
                div[y * w + x] = at(x as i32 + 1, y as i32) + at(x as i32 - 1, y as i32) + at(x as i32, y as i32 + 1) + at(x as i32, y as i32 - 1) - 4.0 * at(x as i32, y as i32);
            }
        }
        let solved = poisson_neumann(&div, w, h);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (su, ss) = (mean(&u), mean(&solved));
        for i in [3usize, 40, 100, 200] {
            assert!(((solved[i] - ss) - (u[i] - su)).abs() < 0.02, "{i}: {} vs {}", solved[i] - ss, u[i] - su);
        }
    }

    #[test]
    fn contrast_domain_reduces_global_contrast_and_detail_gain_raises_local_contrast() {
        let (i, ctx) = img(64, 64, hdr);
        let flat = contrast_domain(&i, i.rect, &ctx, 30.0, 100.0, 1.0);
        let (dark, bright) = (range(&flat, 40, 60, 64, 64), range(&flat, 2, 28, 64, 64));
        let before = (6.0f32 / 0.02f32).ln();
        assert!(((bright.1 + 0.01) / (dark.0 + 0.01)).ln() < before * 0.85);
        let local = |o: &[f32]| (0..60usize).map(|x| (o[(8 * 64 + x) * 4] - o[(8 * 64 + x + 4) * 4]).abs()).sum::<f32>();
        let sharp = contrast_domain(&i, i.rect, &ctx, 30.0, 200.0, 1.0);
        let soft = contrast_domain(&i, i.rect, &ctx, 30.0, 20.0, 1.0);
        assert!(local(&sharp) > local(&soft), "{} vs {}", local(&sharp), local(&soft));
    }
}
