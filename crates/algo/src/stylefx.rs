//! Engrave, Pixel Display and Supernova.

use photocraft_geom::Rect;

use crate::Ctx;
use crate::fxutil::{rgba, set_rgba, smoothstep, xy};
use crate::image::Image;
use crate::noise::hash01;

/// Engrave: the picture redrawn as parallel black lines on white whose thickness follows the
/// darkness (spacing in px, `angle` of the lines in degrees, `contrast` 0–100 how quickly
/// lines thicken in the shadows). Alpha is kept.
pub(crate) fn engrave(src: &Image, out: Rect, ctx: &Ctx, spacing: f32, angle: f32, contrast: f32) -> Vec<f32> {
    let n = src.ch;
    let spacing = spacing.clamp(2.0, 200.0);
    let (sa, ca) = angle.to_radians().sin_cos();
    let gamma = 0.5 + (contrast / 100.0).clamp(0.0, 1.0) * 1.5;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let c = rgba(ctx, px);
        let luma = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        let width = (1.0 - luma).clamp(0.0, 1.0).powf(gamma) * spacing;
        let u = ((x as f32 + 0.5) * ca + (y as f32 + 0.5) * sa).rem_euclid(spacing);
        let d = (u - spacing / 2.0).abs();
        let ink = 1.0 - smoothstep(width / 2.0 - 0.5, width / 2.0 + 0.5, d);
        let v = 1.0 - ink * (width > 0.0) as u8 as f32;
        set_rgba(ctx, px, [v, v, v, c[3]]);
    }
    res
}

/// Pixel Display: the picture as seen on a screen made of red, green and blue stripes
/// `cell` px wide (`strength` 0–100 how much the other two colours are held back), with
/// every third row of cells darkened by `scanlines` %.
pub(crate) fn pixel_display(src: &Image, out: Rect, ctx: &Ctx, cell: f32, strength: f32, scanlines: f32) -> Vec<f32> {
    let n = src.ch;
    let cell = cell.clamp(1.0, 50.0).round() as i32;
    let hold = (1.0 - (strength / 100.0).clamp(0.0, 1.0)).max(0.0);
    let dark = 1.0 - (scanlines / 100.0).clamp(0.0, 1.0);
    let b = ctx.bounds;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let stripe = ((x - b.x0).div_euclid(cell)).rem_euclid(3) as usize;
        let row = ((y - b.y0).div_euclid(cell)).rem_euclid(3);
        let c = rgba(ctx, px);
        let line = if row == 2 { dark } else { 1.0 };
        let f = |k: usize| c[k] * if k == stripe { 1.0 } else { hold } * line;
        set_rgba(ctx, px, [f(0), f(1), f(2), c[3]]);
    }
    res
}

/// Supernova: a bright star centred at `center` (fractions of the bounds) with `rays` spikes
/// reaching about `radius` px, `randomness` 0–100 uneven, added to the picture in `color`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn supernova(src: &Image, out: Rect, ctx: &Ctx, center: (f32, f32), radius: f32, rays: u32, randomness: f32, color: [f32; 3], seed: u32) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let radius = radius.clamp(2.0, 20_000.0);
    let rays = rays.clamp(3, 200);
    let wild = (randomness / 100.0).clamp(0.0, 1.0);
    let weights: Vec<f32> = (0..rays).map(|k| 1.0 - wild * hash01(k as i32, 0, 101, seed)).collect();
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let d = (dx * dx + dy * dy).sqrt() / radius;
        if d > 6.0 {
            continue;
        }
        // Spike strength by direction: neighbouring ray weights, smoothly mixed.
        let t = (dy.atan2(dx) / std::f32::consts::TAU).rem_euclid(1.0) * rays as f32;
        let (k0, f) = (t.floor() as usize % rays as usize, t.fract());
        let k1 = (k0 + 1) % rays as usize;
        // Spikes peak on the ray and fall off between rays.
        let spike = 1.0 - (f - 0.5).abs() * 2.0;
        let along = lerp(weights[k0], weights[k1], f) * (0.35 + 0.65 * spike * spike);
        let glow = (-d * 2.5).exp() * 1.4 + along * (-d * 0.9).exp() * 0.55 / (1.0 + d);
        let g = glow.clamp(0.0, 1.0);
        let c = rgba(ctx, px);
        let mix = |v: f32, tint: f32| 1.0 - (1.0 - v) * (1.0 - g * tint);
        set_rgba(ctx, px, [mix(c[0], color[0]), mix(c[1], color[1]), mix(c[2], color[2]), c[3]]);
    }
    res
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn flat(w: i32, h: i32, v: f32) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        let mut i = Image::new(rect, 4);
        for p in i.data.chunks_mut(4) {
            p.copy_from_slice(&[v, v, v, 1.0]);
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    #[test]
    fn engraving_draws_thicker_lines_in_the_shadows() {
        let ink = |v: f32| {
            let (i, ctx) = flat(48, 48, v);
            let o = engrave(&i, i.rect, &ctx, 6.0, 0.0, 70.0);
            assert!(o.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2] && p[3] == 1.0));
            o.chunks(4).map(|p| 1.0 - p[0]).sum::<f32>() / 2304.0
        };
        let (dark, mid, light) = (ink(0.1), ink(0.5), ink(0.95));
        assert!(dark > mid && mid > light, "{dark} {mid} {light}");
        assert!(ink(1.0) < 0.01, "white paper stays white");
        assert!(dark > 0.5);
    }

    #[test]
    fn pixel_display_keeps_one_channel_per_stripe_and_darkens_scanlines() {
        let (i, ctx) = flat(18, 18, 0.8);
        let o = pixel_display(&i, i.rect, &ctx, 2.0, 100.0, 50.0);
        let at = |x: usize, y: usize| [o[(y * 18 + x) * 4], o[(y * 18 + x) * 4 + 1], o[(y * 18 + x) * 4 + 2]];
        assert_eq!(at(0, 0), [0.8, 0.0, 0.0]);
        assert_eq!(at(2, 0), [0.0, 0.8, 0.0]);
        assert_eq!(at(4, 0), [0.0, 0.0, 0.8]);
        assert!((at(0, 4)[0] - 0.4).abs() < 1e-6, "every third cell row is darkened");
        let off = pixel_display(&i, i.rect, &ctx, 2.0, 0.0, 0.0);
        assert_eq!(off, i.data);
    }

    #[test]
    fn supernova_lights_the_centre_most_and_leaves_the_far_corners() {
        let (i, ctx) = flat(81, 81, 0.1);
        let o = supernova(&i, i.rect, &ctx, (0.5, 0.5), 20.0, 24, 50.0, [1.0, 1.0, 1.0], 4);
        let at = |x: usize, y: usize| o[(y * 81 + x) * 4];
        assert!(at(40, 40) > 0.9, "{}", at(40, 40));
        assert!(at(40, 40) > at(50, 40) && at(50, 40) > at(75, 5));
        assert!((at(0, 0) - 0.1).abs() < 0.02, "far corner: {}", at(0, 0));
        assert_eq!(o, supernova(&i, i.rect, &ctx, (0.5, 0.5), 20.0, 24, 50.0, [1.0, 1.0, 1.0], 4));
    }
}
