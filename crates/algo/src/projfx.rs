//! Projections and tiling: Little Planet, Panorama View, Lens Refraction, Make Seamless and
//! Cubism. The projections treat the image as an equirectangular panorama (width = 360°,
//! height = 180°) or as a flat picture seen through a lens.

use photocraft_geom::Rect;

use crate::Ctx;
use crate::distort::remap;
use crate::fxutil::{smoothstep, xy};
use crate::image::{Edge, Image};
use crate::noise::hash01;

/// Little Planet: a stereographic projection of a panorama, the horizon wrapped into a ball.
/// `scale` 10–500 % zooms, `rotation` turns the planet; the bottom row of the panorama sits at
/// the centre, the top row at infinity.
pub(crate) fn little_planet(src: &Image, out: Rect, ctx: &Ctx, scale: f32, rotation: f32) -> Vec<f32> {
    let b = ctx.bounds;
    let (w, h) = (b.width().max(1) as f32, b.height().max(1) as f32);
    let (cx, cy) = (b.x0 as f32 + w / 2.0, b.y0 as f32 + h / 2.0);
    let radius = w.min(h) / 2.0;
    let zoom = (scale / 100.0).clamp(0.1, 5.0);
    let spin = rotation.to_radians();
    remap(src, out, ctx, Edge::Wrap, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let rho = (dx * dx + dy * dy).sqrt() / radius / zoom;
        let u = ((dy.atan2(dx) + spin) / std::f32::consts::TAU).rem_euclid(1.0);
        let v = 1.0 - 2.0 * rho.atan() / std::f32::consts::PI;
        (b.x0 as f32 + u * w, b.y0 as f32 + v * h)
    })
}

/// Panorama View: a rectilinear window (field of view `fov`°, looking `pan`° around and `tilt`°
/// up) onto an equirectangular panorama.
pub(crate) fn panorama_view(src: &Image, out: Rect, ctx: &Ctx, pan: f32, tilt: f32, fov: f32) -> Vec<f32> {
    let b = ctx.bounds;
    let (w, h) = (b.width().max(1) as f32, b.height().max(1) as f32);
    let (cx, cy) = (b.x0 as f32 + w / 2.0, b.y0 as f32 + h / 2.0);
    let f = (w / 2.0) / (fov.clamp(10.0, 170.0).to_radians() / 2.0).tan();
    let (sp, cp) = pan.to_radians().sin_cos();
    let (st, ct) = tilt.clamp(-90.0, 90.0).to_radians().sin_cos();
    remap(src, out, ctx, Edge::Wrap, |x, y| {
        // A ray through the pixel, then tilted about the x axis and panned about the y axis.
        let (rx, ry, rz) = (x - cx, y - cy, f);
        let (ty, tz) = (ry * ct - rz * st, ry * st + rz * ct);
        let (px, pz) = (rx * cp + tz * sp, -rx * sp + tz * cp);
        let lon = px.atan2(pz);
        let lat = (-ty).atan2((px * px + pz * pz).sqrt());
        (b.x0 as f32 + (lon / std::f32::consts::TAU + 0.5) * w, b.y0 as f32 + (0.5 - lat / std::f32::consts::PI) * h)
    })
}

/// Lens Refraction: a glass ball of `radius` px (`index` 1–2.5) at `center` (fractions of the
/// bounds) refracts what lies behind it; the picture outside the ball is untouched.
pub(crate) fn lens(src: &Image, out: Rect, ctx: &Ctx, radius: f32, index: f32, center: (f32, f32)) -> Vec<f32> {
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let r = radius.clamp(2.0, 20_000.0);
    let n = index.clamp(1.0, 2.5);
    remap(src, out, ctx, Edge::Repeat, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let d2 = dx * dx + dy * dy;
        if d2 >= r * r {
            return (x, y);
        }
        // The ball's outward surface normal here, and a ray travelling straight down (-z) bent
        // by Snell's law into the glass: v = eta * d + m * N.
        let z = (r * r - d2).sqrt();
        let (nx, ny, nz) = (dx / r, dy / r, z / r);
        let eta = 1.0 / n;
        let k = 1.0 - eta * eta * (1.0 - nz * nz);
        if k <= 0.0 {
            return (x, y);
        }
        let m = eta * nz - k.sqrt();
        let (vx, vy, vz) = (m * nx, m * ny, -eta + m * nz);
        if vz >= -1e-4 {
            return (x, y);
        }
        // Follow the ray from the surface point down to the backdrop plane r below the centre.
        let travel = (z + r) / -vz;
        (x + vx * travel, y + vy * travel)
    })
}

/// Make Seamless: the picture is cross-faded with a copy of itself shifted by half its size
/// (wrapping), so the left edge continues into the right and the top into the bottom.
/// `blend` 10–100 % is how much of the picture, from its edges inward, is cross-faded.
pub(crate) fn make_seamless(src: &Image, out: Rect, ctx: &Ctx, blend: f32) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let (w, h) = (b.width().max(1) as i32, b.height().max(1) as i32);
    let band = (blend / 100.0).clamp(0.1, 1.0);
    let weight = |pos: i32, len: i32| {
        // 1 in the middle, 0 at the edges, easing over `band` of the half-length.
        let u = ((pos as f32 + 0.5) / len as f32 * 2.0 - 1.0).abs();
        1.0 - smoothstep(1.0 - band, 1.0, u)
    };
    let mut res = src.crop(out);
    let mut tmp = vec![0.0f32; n];
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (lx, ly) = (x - b.x0, y - b.y0);
        let k = weight(lx, w) * weight(ly, h);
        if k >= 1.0 {
            continue;
        }
        let (sx, sy) = ((lx + w / 2).rem_euclid(w) + b.x0, (ly + h / 2).rem_euclid(h) + b.y0);
        src.sample(sx as f32 + 0.5, sy as f32 + 0.5, Edge::Wrap, b, ctx.alpha, &mut tmp);
        for c in 0..n {
            px[c] = px[c] * k + tmp[c] * (1.0 - k);
        }
    }
    res
}

/// How far Cubism reaches, for the tile margin.
pub(crate) fn cubism_halo(size: f32) -> i32 {
    (size.clamp(2.0, 400.0) * 2.0).ceil() as i32 + 2
}

/// Cubism: the picture repainted with randomly turned squares about `size` px wide, each
/// filled with the colour at its centre; where no square lands the original shows.
pub(crate) fn cubism(src: &Image, out: Rect, ctx: &Ctx, size: f32, seed: u32) -> Vec<f32> {
    let n = src.ch;
    let size = size.clamp(2.0, 400.0);
    let cell = size;
    let b = ctx.bounds;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (fx, fy) = ((x - b.x0) as f32 + 0.5, (y - b.y0) as f32 + 0.5);
        let (gx, gy) = ((fx / cell).floor() as i32, (fy / cell).floor() as i32);
        let mut best: Option<(f32, i32, i32)> = None;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (cx, cy) = (gx + dx, gy + dy);
                let half = size * (0.45 + 0.35 * hash01(cx, cy, 111, seed));
                let centre = ((cx as f32 + 0.15 + 0.7 * hash01(cx, cy, 112, seed)) * cell, (cy as f32 + 0.15 + 0.7 * hash01(cx, cy, 113, seed)) * cell);
                let a = hash01(cx, cy, 114, seed) * std::f32::consts::FRAC_PI_2;
                let (s, c) = a.sin_cos();
                let (ux, uy) = (fx - centre.0, fy - centre.1);
                let (rx, ry) = (ux * c + uy * s, -ux * s + uy * c);
                if rx.abs() <= half && ry.abs() <= half {
                    let rank = hash01(cx, cy, 115, seed);
                    if best.is_none_or(|(r, ..)| rank > r) {
                        best = Some((rank, (centre.0 as i32) + b.x0, (centre.1 as i32) + b.y0));
                    }
                }
            }
        }
        if let Some((_, sx, sy)) = best {
            let (sx, sy) = (sx.clamp(src.rect.x0, src.rect.x1 - 1), sy.clamp(src.rect.y0, src.rect.y1 - 1));
            px.copy_from_slice(src.px(sx, sy));
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

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
    fn the_little_planet_centre_shows_the_bottom_row_and_the_edge_the_sky() {
        let (i, ctx) = ramp(64, 32);
        let o = little_planet(&i, i.rect, &ctx, 100.0, 0.0);
        let green = |x: usize, y: usize| o[(y * 64 + x) * 4 + 1];
        assert!(green(32, 16) > 0.9, "centre ← bottom row: {}", green(32, 16));
        assert!(green(32, 0) < green(32, 8) && green(32, 8) < green(32, 16), "sky toward the edge");
        assert_eq!(o, little_planet(&i, i.rect, &ctx, 100.0, 0.0));
    }

    #[test]
    fn a_panorama_view_straight_ahead_shows_the_middle_and_pans_sideways() {
        let (i, ctx) = ramp(128, 64);
        let ahead = panorama_view(&i, i.rect, &ctx, 0.0, 0.0, 90.0);
        let red = |o: &[f32], x: usize, y: usize| o[(y * 128 + x) * 4];
        assert!((red(&ahead, 64, 32) - 0.5).abs() < 0.02, "the middle of the view is the middle of the panorama");
        let right = panorama_view(&i, i.rect, &ctx, 90.0, 0.0, 90.0);
        assert!((red(&right, 64, 32) - 0.75).abs() < 0.02, "panning 90° right looks a quarter turn on: {}", red(&right, 64, 32));
        let up = panorama_view(&i, i.rect, &ctx, 0.0, 45.0, 90.0);
        assert!(up[(32 * 128 + 64) * 4 + 1] < 0.4, "tilting up looks toward the top rows");
    }

    #[test]
    fn a_lens_magnifies_its_centre_and_leaves_the_outside_alone() {
        let (i, ctx) = ramp(61, 61);
        let o = lens(&i, i.rect, &ctx, 20.0, 1.6, (0.5, 0.5));
        let at = |x: usize, y: usize| o[(y * 61 + x) * 4];
        assert_eq!(at(0, 0), i.data[0]);
        assert_eq!(at(60, 30), i.data[(30 * 61 + 60) * 4]);
        assert!((at(30, 30) - 0.5).abs() < 0.01, "the centre stays put");
        // Magnified: a point halfway out shows something nearer the centre than itself.
        let src_x = 40.0 / 60.0;
        assert!((at(40, 30) - 0.5).abs() < (src_x - 0.5f32).abs(), "{} vs {src_x}", at(40, 30));
        assert_eq!(lens(&i, i.rect, &ctx, 20.0, 1.0, (0.5, 0.5)).len(), i.data.len());
    }

    #[test]
    fn make_seamless_matches_opposite_edges() {
        let (i, ctx) = ramp(64, 64);
        let o = make_seamless(&i, i.rect, &ctx, 100.0);
        let px = |x: usize, y: usize| [o[(y * 64 + x) * 4], o[(y * 64 + x) * 4 + 1]];
        let step = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
        // Across the left/right seam the jump is no bigger than between ordinary neighbours.
        let seam = step(px(63, 20), px(0, 20));
        let normal = step(px(30, 20), px(31, 20));
        assert!(seam < normal * 3.0 + 0.05, "seam {seam} vs neighbour {normal}");
        let raw = step([i.data[(20 * 64 + 63) * 4], 0.0], [i.data[(20 * 64) * 4], 0.0]);
        assert!(seam < raw * 0.2, "the original jumps {raw}, the result {seam}");
    }

    #[test]
    fn cubism_paints_flat_squares_deterministically() {
        let (i, ctx) = ramp(60, 60);
        let o = cubism(&i, i.rect, &ctx, 12.0, 3);
        assert_eq!(o, cubism(&i, i.rect, &ctx, 12.0, 3));
        assert_ne!(o, cubism(&i, i.rect, &ctx, 12.0, 4));
        let distinct: std::collections::BTreeSet<u32> = o.chunks(4).map(|p| (p[0] * 1000.0) as u32).collect();
        let original: std::collections::BTreeSet<u32> = i.data.chunks(4).map(|p| (p[0] * 1000.0) as u32).collect();
        assert!(distinct.len() < original.len(), "flat squares reduce the number of distinct colours");
    }
}
