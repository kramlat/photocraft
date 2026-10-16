//! Relief and light: Bump Map, Normal Map and Sparkle.

use photocraft_geom::Rect;

use crate::Ctx;
use crate::fxutil::{luma, rgba, set_rgba, xy};
use crate::image::Image;

/// Height at `(x, y)`: the pixel's luma, clamped at the edges of the area read.
fn height(src: &Image, ctx: &Ctx, x: i32, y: i32) -> f32 {
    let (x, y) = (x.clamp(src.rect.x0, src.rect.x1 - 1), y.clamp(src.rect.y0, src.rect.y1 - 1));
    luma(ctx, src.px(x, y))
}

/// Slope `(dh/dx, dh/dy)` by Sobel, per pixel of height (height runs 0..1).
fn slope(src: &Image, ctx: &Ctx, x: i32, y: i32) -> (f32, f32) {
    let h = |dx: i32, dy: i32| height(src, ctx, x + dx, y + dy);
    let gx = (h(1, -1) + 2.0 * h(1, 0) + h(1, 1)) - (h(-1, -1) + 2.0 * h(-1, 0) + h(-1, 1));
    let gy = (h(-1, 1) + 2.0 * h(0, 1) + h(1, 1)) - (h(-1, -1) + 2.0 * h(0, -1) + h(1, -1));
    (gx / 8.0, gy / 8.0)
}

/// Bump Map: the picture lit as if its brightness were a relief, light coming from `azimuth`°
/// (0 = right, 90 = above) at `elevation`° above the surface; `depth` scales the relief and
/// `ambient` % keeps the unlit part visible. Flat areas keep their colour.
pub(crate) fn bump_map(src: &Image, out: Rect, ctx: &Ctx, azimuth: f32, elevation: f32, depth: f32, ambient: f32) -> Vec<f32> {
    let n = src.ch;
    let (sa, ca) = azimuth.to_radians().sin_cos();
    let (se, ce) = elevation.clamp(1.0, 90.0).to_radians().sin_cos();
    let light = (ce * ca, -ce * sa, se);
    let depth = depth.clamp(0.0, 200.0);
    let amb = (ambient / 100.0).clamp(0.0, 1.0);
    let flat = light.2;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (gx, gy) = slope(src, ctx, x, y);
        let (nx, ny, nz) = (-gx * depth, -gy * depth, 1.0);
        let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
        let diffuse = ((nx * light.0 + ny * light.1 + nz * light.2) * inv).max(0.0);
        // Normalised so a flat pixel is unchanged, with the ambient floor.
        let shade = amb + (1.0 - amb) * diffuse / flat;
        let c = rgba(ctx, px);
        set_rgba(ctx, px, [c[0] * shade, c[1] * shade, c[2] * shade, c[3]]);
    }
    res
}

/// Normal Map: a tangent-space normal map (RGB = XYZ·½ + ½) from the picture's brightness
/// read as height; `strength` scales the slopes, `flip_y` suits DirectX-style maps.
pub(crate) fn normal_map(src: &Image, out: Rect, ctx: &Ctx, strength: f32, flip_y: bool) -> Vec<f32> {
    let n = src.ch;
    let k = strength.clamp(0.01, 100.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (gx, gy) = slope(src, ctx, x, y);
        let (nx, ny, nz) = (-gx * k, -gy * k * if flip_y { -1.0 } else { 1.0 }, 1.0);
        let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
        let a = rgba(ctx, px)[3];
        set_rgba(ctx, px, [nx * inv * 0.5 + 0.5, ny * inv * 0.5 + 0.5, nz * inv * 0.5 + 0.5, a]);
    }
    res
}

/// How far Sparkle's spikes reach, for the tile margin.
pub(crate) fn sparkle_halo(length: f32) -> i32 {
    length.clamp(1.0, 400.0).ceil() as i32 + 1
}

/// Sparkle: bright points (above `threshold` % brightness) grow `spikes` crossing spikes
/// `length` px long, turned by `angle`°, added to the picture at `intensity` % in white.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sparkle(src: &Image, out: Rect, ctx: &Ctx, threshold: f32, length: f32, spikes: u32, angle: f32, intensity: f32) -> Vec<f32> {
    let n = src.ch;
    let thr = (threshold / 100.0).clamp(0.0, 0.999);
    let len = length.clamp(1.0, 400.0);
    let spikes = spikes.clamp(2, 16);
    let gain = (intensity / 100.0).clamp(0.0, 1.0) * 2.0;
    let dirs: Vec<(f32, f32)> = (0..spikes).map(|k| (angle.to_radians() + std::f32::consts::PI * k as f32 / spikes as f32).sin_cos()).collect();
    let steps = len.ceil() as i32;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let mut glow = 0.0f32;
        for (s, c) in &dirs {
            for sign in [-1.0f32, 1.0] {
                for t in 0..=steps {
                    let (sx, sy) = (x + (sign * c * t as f32).round() as i32, y + (sign * s * t as f32).round() as i32);
                    if !src.rect.contains(sx, sy) {
                        break;
                    }
                    let l = luma(ctx, src.px(sx, sy));
                    if l > thr {
                        let power = (l - thr) / (1.0 - thr);
                        // A spike thins out with distance from its bright point.
                        glow += power * (1.0 - t as f32 / len).max(0.0) / (1.0 + 0.05 * t as f32);
                    }
                }
            }
        }
        let g = (glow * gain).clamp(0.0, 1.0);
        if g > 0.0 {
            let c = rgba(ctx, px);
            let mix = |v: f32| 1.0 - (1.0 - v) * (1.0 - g);
            set_rgba(ctx, px, [mix(c[0]), mix(c[1]), mix(c[2]), c[3]]);
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

    #[test]
    fn bump_map_leaves_flat_areas_and_lights_slopes_facing_the_light() {
        // A ramp rising to the right: with light from the right the slope faces it.
        let (i, ctx) = img(20, 10, |x, _| x as f32 / 19.0);
        let from_right = bump_map(&i, i.rect, &ctx, 0.0, 30.0, 20.0, 0.0);
        let from_left = bump_map(&i, i.rect, &ctx, 180.0, 30.0, 20.0, 0.0);
        let mid = |o: &[f32]| o[(5 * 20 + 10) * 4] / i.data[(5 * 20 + 10) * 4];
        assert!(mid(&from_left) > mid(&from_right), "{} {}", mid(&from_left), mid(&from_right));
        let (flat, c2) = img(8, 8, |_, _| 0.6);
        let o = bump_map(&flat, flat.rect, &c2, 135.0, 45.0, 30.0, 20.0);
        assert!(o.chunks(4).all(|p| (p[0] - 0.6).abs() < 1e-5), "a flat area keeps its colour");
    }

    #[test]
    fn a_normal_map_of_a_flat_area_is_the_neutral_blue_and_tilts_with_slopes() {
        let (flat, ctx) = img(8, 8, |_, _| 0.3);
        let o = normal_map(&flat, flat.rect, &ctx, 4.0, false);
        assert!(o.chunks(4).all(|p| (p[0] - 0.5).abs() < 1e-5 && (p[1] - 0.5).abs() < 1e-5 && (p[2] - 1.0).abs() < 1e-5));
        let (ramp, c2) = img(16, 8, |x, _| x as f32 / 15.0);
        let r = normal_map(&ramp, ramp.rect, &c2, 4.0, false);
        let px = &r[(4 * 16 + 8) * 4..(4 * 16 + 8) * 4 + 3];
        assert!(px[0] < 0.5 && (px[1] - 0.5).abs() < 1e-4, "{px:?}: a rise to the right tilts the normal left");
        let flipped = normal_map(&ramp, ramp.rect, &c2, 4.0, true);
        assert_eq!(flipped[(4 * 16 + 8) * 4], px[0]);
    }

    #[test]
    fn sparkle_grows_spikes_from_bright_points_only() {
        let (mut i, ctx) = img(41, 41, |_, _| 0.1);
        let k = (20 * 41 + 20) * 4;
        i.data[k..k + 3].copy_from_slice(&[1.0, 1.0, 1.0]);
        let o = sparkle(&i, i.rect, &ctx, 80.0, 15.0, 4, 0.0, 80.0);
        let at = |x: usize, y: usize| o[(y * 41 + x) * 4];
        assert!(at(26, 20) > 0.3, "along a spike: {}", at(26, 20));
        assert!(at(26, 20) > at(26, 24), "off the spike it is dimmer: {} vs {}", at(26, 20), at(26, 24));
        assert!((at(2, 2) - 0.1).abs() < 0.02, "far from the point nothing changes");
        let none = sparkle(&i, i.rect, &ctx, 99.9, 15.0, 4, 0.0, 0.0);
        assert_eq!(none, i.data);
    }
}
