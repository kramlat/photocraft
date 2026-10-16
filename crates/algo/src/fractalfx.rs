//! Fractals and interference: Fractal Explorer, Fractal Trace, Recursive Transform (Droste) and
//! Diffraction Pattern.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::colorops::from_hsv;
use crate::fxutil::{set_rgba, xy};
use crate::image::{Edge, Image};

/// The fractal drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FractalKind {
    #[default]
    Mandelbrot,
    /// A Julia set for the constant `(julia_x, julia_y)`.
    Julia,
    /// The Mandelbrot iteration with absolute values: a burning ship.
    BurningShip,
}

/// What Fractal Trace shows where the iteration never escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TraceInside {
    #[default]
    Original,
    Transparent,
    Black,
}

/// The view of the complex plane: centre, and how far it reaches from the centre to the
/// shorter edge of the area.
#[derive(Clone, Copy)]
pub(crate) struct View {
    pub center: (f64, f64),
    pub zoom: f64,
}

fn plane(b: Rect, v: View, x: f32, y: f32) -> (f64, f64) {
    let half = f64::from(b.width().min(b.height()).max(1)) / 2.0;
    let (cx, cy) = (f64::from(b.x0) + f64::from(b.width()) / 2.0, f64::from(b.y0) + f64::from(b.height()) / 2.0);
    let scale = 2.0 / v.zoom.clamp(0.1, 1.0e9) / half;
    (v.center.0 + (f64::from(x) - cx) * scale, v.center.1 + (f64::from(y) - cy) * scale)
}

/// Iterates `kind` at plane point `p`; `Some((smooth iteration count, final z))` once it escapes.
fn iterate(kind: FractalKind, p: (f64, f64), julia: (f64, f64), max: u32) -> Option<(f64, (f64, f64))> {
    let (mut zx, mut zy, cx, cy) = match kind {
        FractalKind::Julia => (p.0, p.1, julia.0, julia.1),
        _ => (0.0, 0.0, p.0, p.1),
    };
    const BAILOUT: f64 = 256.0;
    for n in 0..max {
        let (ax, ay) = if kind == FractalKind::BurningShip { (zx.abs(), zy.abs()) } else { (zx, zy) };
        let (nx, ny) = (ax * ax - ay * ay + cx, 2.0 * ax * ay + cy);
        zx = nx;
        zy = ny;
        let m = zx * zx + zy * zy;
        if m > BAILOUT {
            let smooth = f64::from(n) + 1.0 - (m.ln() / 2.0).ln() / std::f64::consts::LN_2;
            return Some((smooth, (zx, zy)));
        }
    }
    None
}

/// Fractal Explorer: the fractal coloured by how fast each point escapes (`colour_speed` turns
/// the palette faster), black inside the set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn explorer(src: &Image, out: Rect, ctx: &Ctx, kind: FractalKind, center: (f32, f32), zoom: f32, iterations: u32, julia: (f32, f32), colour_speed: f32) -> Vec<f32> {
    let n = src.ch;
    let view = View { center: (f64::from(center.0), f64::from(center.1)), zoom: f64::from(zoom) };
    let max = iterations.clamp(1, 2000);
    let speed = f64::from(colour_speed.clamp(0.1, 20.0));
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let p = plane(ctx.bounds, view, x as f32 + 0.5, y as f32 + 0.5);
        match iterate(kind, p, (f64::from(julia.0), f64::from(julia.1)), max) {
            Some((nu, _)) => {
                let hue = ((0.62 + nu * 0.025 * speed).rem_euclid(1.0)) as f32;
                let rgb = from_hsv(hue, 0.85, 1.0);
                set_rgba(ctx, px, [rgb[0], rgb[1], rgb[2], 1.0]);
            }
            None => set_rgba(ctx, px, [0.0, 0.0, 0.0, 1.0]),
        }
    }
    res
}

/// Fractal Trace: the picture mapped through the fractal. Each pixel follows the iteration until
/// it escapes and shows the picture at where it ended up.
#[allow(clippy::too_many_arguments)]
pub(crate) fn trace(src: &Image, out: Rect, ctx: &Ctx, kind: FractalKind, center: (f32, f32), zoom: f32, iterations: u32, julia: (f32, f32), inside: TraceInside) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let view = View { center: (f64::from(center.0), f64::from(center.1)), zoom: f64::from(zoom) };
    let max = iterations.clamp(1, 1000);
    let (w, h) = (b.width().max(1) as f32, b.height().max(1) as f32);
    let mut res = src.crop(out);
    let mut tmp = vec![0.0f32; n];
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let p = plane(b, view, x as f32 + 0.5, y as f32 + 0.5);
        match iterate(kind, p, (f64::from(julia.0), f64::from(julia.1)), max) {
            Some((_, (zx, zy))) => {
                // Final z lies beyond the bailout radius; its direction picks the picture point.
                let m = (zx * zx + zy * zy).sqrt().max(1e-9);
                let (u, v) = (0.5 + zx / m * 0.5 * (1.0 - 1.0 / (1.0 + m.ln())), 0.5 + zy / m * 0.5 * (1.0 - 1.0 / (1.0 + m.ln())));
                src.sample(b.x0 as f32 + u as f32 * w, b.y0 as f32 + v as f32 * h, Edge::Repeat, b, ctx.alpha, &mut tmp);
                px.copy_from_slice(&tmp);
            }
            None => match inside {
                TraceInside::Original => {}
                TraceInside::Transparent if ctx.alpha => px.iter_mut().for_each(|v| *v = 0.0),
                _ => set_rgba(ctx, px, [0.0, 0.0, 0.0, 1.0]),
            },
        }
    }
    res
}

/// Recursive Transform (Droste): the picture shrunk by `scale` % and turned by `angle`°
/// toward `center` again and again, `depth` times, each copy on top of the larger one.
pub(crate) fn droste(src: &Image, out: Rect, ctx: &Ctx, scale: f32, angle: f32, depth: u32, center: (f32, f32)) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let s = (scale / 100.0).clamp(0.1, 0.95);
    let depth = depth.clamp(1, 16);
    let (sa, ca) = angle.to_radians().sin_cos();
    let mut res = src.crop(out);
    let mut tmp = vec![0.0f32; n];
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (mut dx, mut dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        for _ in 0..depth {
            // One level inward: undo the shrink and the turn.
            let (rx, ry) = ((dx * ca + dy * sa) / s, (-dx * sa + dy * ca) / s);
            dx = rx;
            dy = ry;
            let (px_, py_) = (cx + dx, cy + dy);
            if px_ < b.x0 as f32 || py_ < b.y0 as f32 || px_ >= b.x1 as f32 || py_ >= b.y1 as f32 {
                break;
            }
            src.sample(px_, py_, Edge::Repeat, b, ctx.alpha, &mut tmp);
            // The inner copy lies on top of everything seen so far.
            let top = if ctx.alpha { tmp[n - 1] } else { 1.0 };
            let colours = if ctx.alpha { n - 1 } else { n };
            for k in 0..colours {
                px[k] = tmp[k] * top + px[k] * (1.0 - top);
            }
            if ctx.alpha {
                px[n - 1] = top + px[n - 1] * (1.0 - top);
            }
        }
    }
    res
}

fn sinc2(u: f32) -> f32 {
    if u.abs() < 1e-4 { 1.0 } else { (u.sin() / u).powi(2) }
}

/// Diffraction Pattern: coloured interference rings round the centre. `size` px is the ring
/// scale, `spread` 0–100 how far apart the red, green and blue wavelengths are, `lobes` adds
/// radial lobes, `brightness` and `contrast` 0–100.
#[allow(clippy::too_many_arguments)]
pub(crate) fn diffraction(src: &Image, out: Rect, ctx: &Ctx, center: (f32, f32), size: f32, spread: f32, lobes: u32, brightness: f32, contrast: f32) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 * center.0, b.y0 as f32 + b.height() as f32 * center.1);
    let size = size.clamp(1.0, 10_000.0);
    let s = (spread / 100.0).clamp(0.0, 1.0) * 0.5;
    let wavelengths = [1.0 + s, 1.0, 1.0 - s];
    let gamma = 0.3 + (contrast / 100.0).clamp(0.0, 1.0) * 2.0;
    let gain = (brightness / 50.0).clamp(0.0, 2.0) * 1.6;
    let lobes = lobes.min(32) as f32;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let r = (dx * dx + dy * dy).sqrt();
        let theta = dy.atan2(dx);
        let angular = if lobes > 0.0 { 0.35 + 0.65 * (lobes * theta).cos().powi(2) } else { 1.0 };
        let mut c = [0.0f32; 3];
        for (k, w) in wavelengths.iter().enumerate() {
            let intensity = sinc2(std::f32::consts::PI * r / (size * w * 0.35)) * angular;
            c[k] = (intensity.powf(gamma) * gain).clamp(0.0, 1.0);
        }
        set_rgba(ctx, px, [c[0], c[1], c[2], 1.0]);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn blank(w: i32, h: i32) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        let mut i = Image::new(rect, 4);
        i.data.iter_mut().for_each(|v| *v = 0.5);
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    #[test]
    fn the_explorer_draws_the_set_black_and_colours_the_outside() {
        let (i, ctx) = blank(60, 60);
        let o = explorer(&i, i.rect, &ctx, FractalKind::Mandelbrot, (-0.5, 0.0), 0.8, 80, (0.0, 0.0), 1.0);
        let at = |x: usize, y: usize| o[(y * 60 + x) * 4..(y * 60 + x) * 4 + 3].to_vec();
        assert_eq!(at(33, 30), [0.0, 0.0, 0.0], "a point of the set is black");
        let outside = at(2, 2);
        assert!(outside.iter().any(|v| *v > 0.2), "{outside:?}");
        assert_eq!(o, explorer(&i, i.rect, &ctx, FractalKind::Mandelbrot, (-0.5, 0.0), 0.8, 80, (0.0, 0.0), 1.0));
        for kind in [FractalKind::Julia, FractalKind::BurningShip] {
            let k = explorer(&i, i.rect, &ctx, kind, (0.0, 0.0), 1.0, 60, (-0.8, 0.156), 1.0);
            assert!(k.chunks(4).any(|p| p[0] > 0.1) && k.chunks(4).any(|p| p[0] == 0.0 && p[1] == 0.0), "{kind:?} has both inside and outside");
        }
    }

    #[test]
    fn a_trace_shows_picture_colours_outside_the_set_and_the_original_inside() {
        let rect = Rect::new(0, 0, 60, 60);
        let mut i = Image::new(rect, 4);
        for y in 0..60 {
            for x in 0..60 {
                let k = (y * 60 + x) * 4;
                i.data[k..k + 4].copy_from_slice(&[x as f32 / 59.0, y as f32 / 59.0, 0.25, 1.0]);
            }
        }
        let ctx = Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true };
        let o = trace(&i, rect, &ctx, FractalKind::Mandelbrot, (-0.5, 0.0), 0.8, 60, (0.0, 0.0), TraceInside::Original);
        let inside_px = (30 * 60 + 33) * 4;
        assert_eq!(&o[inside_px..inside_px + 4], &i.data[inside_px..inside_px + 4], "inside keeps the original");
        let changed = o.chunks(4).zip(i.data.chunks(4)).filter(|(p, q)| p != q).count();
        assert!(changed > 500, "{changed}");
        let t = trace(&i, rect, &ctx, FractalKind::Mandelbrot, (-0.5, 0.0), 0.8, 60, (0.0, 0.0), TraceInside::Transparent);
        assert_eq!(t[inside_px + 3], 0.0);
    }

    #[test]
    fn droste_nests_shrunk_copies_of_the_picture() {
        let rect = Rect::new(0, 0, 64, 64);
        let mut i = Image::new(rect, 4);
        for y in 0..64 {
            for x in 0..64 {
                let k = (y * 64 + x) * 4;
                i.data[k..k + 4].copy_from_slice(&[x as f32 / 63.0, 0.0, y as f32 / 63.0, 1.0]);
            }
        }
        let ctx = Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true };
        let red = |o: &[f32], x: usize, y: usize| o[(y * 64 + x) * 4];
        let one = droste(&i, rect, &ctx, 50.0, 0.0, 1, (0.5, 0.5));
        // Outside the nested copy (the middle half) the picture is untouched.
        assert_eq!(red(&one, 4, 32), red(&i.data, 4, 32));
        // Inside it, x = 24 shows what lies at x = 16 of the original (twice as far from the centre).
        assert!((red(&one, 24, 32) - 16.5 / 64.0).abs() < 0.03, "{}", red(&one, 24, 32));
        // The centre is a fixed point, and deeper levels leave the outside alone.
        assert!((red(&one, 32, 32) - red(&i.data, 32, 32)).abs() < 0.02);
        let three = droste(&i, rect, &ctx, 50.0, 0.0, 3, (0.5, 0.5));
        assert_eq!(red(&three, 4, 32), red(&i.data, 4, 32));
        assert_ne!(three, one);
    }

    #[test]
    fn diffraction_is_brightest_at_the_centre_with_rings_and_colour_fringes() {
        let (i, ctx) = blank(101, 101);
        let o = diffraction(&i, i.rect, &ctx, (0.5, 0.5), 20.0, 60.0, 0, 50.0, 50.0);
        let at = |x: usize, y: usize| [o[(y * 101 + x) * 4], o[(y * 101 + x) * 4 + 1], o[(y * 101 + x) * 4 + 2]];
        let c = at(50, 50);
        assert!(c.iter().all(|v| *v > 0.7), "{c:?}");
        let profile: Vec<f32> = (0..50).map(|d| at(50 + d, 50)[1]).collect();
        let minima = profile.windows(3).filter(|w| w[1] < w[0] && w[1] < w[2]).count();
        assert!(minima >= 2, "rings: {minima}");
        assert!((0..50).any(|d| (at(50 + d, 50)[0] - at(50 + d, 50)[2]).abs() > 0.1), "the colours separate");
        let lobed = diffraction(&i, i.rect, &ctx, (0.5, 0.5), 20.0, 60.0, 6, 50.0, 50.0);
        assert_ne!(lobed, o);
    }
}
