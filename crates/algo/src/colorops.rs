//! Per-pixel colour filters: Value Invert, Linear Invert, Color Exchange, Alien Map and Component
//! Extract. They work on straight sRGB RGBA (`fxutil::rgba` / `set_rgba`), so every colour model
//! and depth goes through the same maths; alpha is never changed.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::fxutil::{rgba, set_rgba};
use crate::image::Image;

/// What Component Extract turns into a grey image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Component {
    #[default]
    Red,
    Green,
    Blue,
    /// HSV hue (0° black, 360° white).
    Hue,
    /// HSV saturation.
    Saturation,
    /// HSV value (the largest of R, G, B).
    Value,
    /// HSL lightness, `(max + min) / 2`.
    Lightness,
    /// Rec. 601 luma.
    Luma,
    Cyan,
    Magenta,
    Yellow,
    Black,
    Alpha,
}

fn map_rgba(src: &Image, out: Rect, ctx: &Ctx, f: impl Fn([f32; 4]) -> [f32; 4]) -> Vec<f32> {
    let n = src.ch;
    let mut res = src.crop(out);
    for px in res.chunks_exact_mut(n) {
        let c = rgba(ctx, px);
        let mut o = f(c);
        o[3] = c[3];
        set_rgba(ctx, px, o);
    }
    res
}

/// `(hue 0..1, saturation, value)` of a straight RGB colour.
pub(crate) fn to_hsv(c: [f32; 3]) -> [f32; 3] {
    let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let d = mx - mn;
    let h = if d <= 0.0 {
        0.0
    } else if mx == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if mx == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    } / 6.0;
    [h, if mx > 0.0 { d / mx } else { 0.0 }, mx]
}

pub(crate) fn from_hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let k = |n: f32| {
        let p = (n + h * 6.0).rem_euclid(6.0);
        v - v * s * p.min(4.0 - p).clamp(0.0, 1.0)
    };
    [k(5.0), k(3.0), k(1.0)]
}

/// Value Invert: HSV value `v → 1 − v`, hue and saturation kept.
pub(crate) fn value_invert(src: &Image, out: Rect, ctx: &Ctx) -> Vec<f32> {
    map_rgba(src, out, ctx, |c| {
        let [h, s, v] = to_hsv([c[0], c[1], c[2]]);
        let r = from_hsv(h, s, 1.0 - v);
        [r[0], r[1], r[2], c[3]]
    })
}

pub(crate) fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub(crate) fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Linear Invert: inverts in linear light (`1 − v` on the linear values), so mid-grey stays mid-grey
/// perceptually where a plain Invert shifts it.
pub(crate) fn linear_invert(src: &Image, out: Rect, ctx: &Ctx) -> Vec<f32> {
    map_rgba(src, out, ctx, |c| {
        let f = |v: f32| linear_to_srgb(1.0 - srgb_to_linear(v.clamp(0.0, 1.0)));
        [f(c[0]), f(c[1]), f(c[2]), c[3]]
    })
}

/// Color Exchange: a pixel whose R, G and B are each within `threshold` of `from` moves by the
/// offset `to − from`, so a gradient of near-`from` colours becomes the matching gradient around `to`.
pub(crate) fn color_exchange(src: &Image, out: Rect, ctx: &Ctx, from: [f32; 3], to: [f32; 3], threshold: [f32; 3]) -> Vec<f32> {
    map_rgba(src, out, ctx, |c| {
        let near = (0..3).all(|i| (c[i] - from[i]).abs() <= threshold[i]);
        if near {
            let m = |i: usize| (c[i] - from[i] + to[i]).clamp(0.0, 1.0);
            [m(0), m(1), m(2), c[3]]
        } else {
            c
        }
    })
}

/// Alien Map: each selected channel becomes `0.5 + 0.5·sin(2π·frequency·v + phase)`, a repeating
/// colour wave over the original value (frequency 0.5 maps 0..1 across one half period).
pub(crate) fn alien_map(src: &Image, out: Rect, ctx: &Ctx, frequency: [f32; 3], phase_degrees: [f32; 3], modify: [bool; 3]) -> Vec<f32> {
    map_rgba(src, out, ctx, |c| {
        let w = |i: usize| {
            if modify[i] {
                0.5 + 0.5 * (std::f32::consts::TAU * frequency[i] * c[i] + phase_degrees[i].to_radians()).sin()
            } else {
                c[i]
            }
        };
        [w(0), w(1), w(2), c[3]]
    })
}

/// The component of a straight RGBA colour as a `0..=1` value.
fn component_of(kind: Component, c: [f32; 4]) -> f32 {
    let rgb = [c[0], c[1], c[2]];
    let mx = rgb[0].max(rgb[1]).max(rgb[2]);
    let mn = rgb[0].min(rgb[1]).min(rgb[2]);
    let k = 1.0 - mx;
    let ink = |v: f32| if k >= 1.0 { 0.0 } else { (1.0 - v - k) / (1.0 - k) };
    match kind {
        Component::Red => c[0],
        Component::Green => c[1],
        Component::Blue => c[2],
        Component::Hue => to_hsv(rgb)[0],
        Component::Saturation => to_hsv(rgb)[1],
        Component::Value => mx,
        Component::Lightness => (mx + mn) / 2.0,
        Component::Luma => 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2],
        Component::Cyan => ink(c[0]),
        Component::Magenta => ink(c[1]),
        Component::Yellow => ink(c[2]),
        Component::Black => k,
        Component::Alpha => c[3],
    }
}

/// Component Extract: the chosen component as a grey image (`invert` flips it). Alpha is kept, except
/// that extracting Alpha leaves the pixel opaque.
pub(crate) fn component_extract(src: &Image, out: Rect, ctx: &Ctx, kind: Component, invert: bool) -> Vec<f32> {
    let n = src.ch;
    let mut res = src.crop(out);
    for px in res.chunks_exact_mut(n) {
        let c = rgba(ctx, px);
        let mut v = component_of(kind, c).clamp(0.0, 1.0);
        if invert {
            v = 1.0 - v;
        }
        let a = if kind == Component::Alpha { 1.0 } else { c[3] };
        set_rgba(ctx, px, [v, v, v, a]);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn img(px: &[[f32; 4]]) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, px.len() as i32, 1);
        let mut i = Image::new(rect, 4);
        for (k, p) in px.iter().enumerate() {
            i.data[k * 4..k * 4 + 4].copy_from_slice(p);
        }
        (i, Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }

    fn near(a: &[f32], b: &[f32]) {
        assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4), "{a:?} vs {b:?}");
    }

    #[test]
    fn value_invert_keeps_hue_and_saturation() {
        let (i, ctx) = img(&[[1.0, 0.0, 0.0, 1.0], [0.2, 0.4, 0.6, 0.5], [0.0, 0.0, 0.0, 1.0]]);
        let o = value_invert(&i, i.rect, &ctx);
        near(&o[0..4], &[0.0, 0.0, 0.0, 1.0]);
        // value 0.6 → 0.4 with the same hue/saturation: the colour scales by 0.4/0.6.
        near(&o[4..8], &[0.2 * 0.4 / 0.6, 0.4 * 0.4 / 0.6, 0.4, 0.5]);
        near(&o[8..12], &[1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn linear_invert_is_an_involution_and_flips_the_ends() {
        let (i, ctx) = img(&[[0.0, 1.0, 0.5, 1.0]]);
        let once = linear_invert(&i, i.rect, &ctx);
        near(&once[0..3], &[1.0, 0.0, linear_to_srgb(1.0 - srgb_to_linear(0.5))]);
        let twice = linear_invert(&Image { data: once, ..i.clone() }, i.rect, &ctx);
        near(&twice, &i.data);
    }

    #[test]
    fn color_exchange_moves_only_colours_within_the_threshold() {
        let (i, ctx) = img(&[[0.5, 0.5, 0.5, 1.0], [0.55, 0.5, 0.5, 1.0], [0.9, 0.5, 0.5, 1.0]]);
        let o = color_exchange(&i, i.rect, &ctx, [0.5; 3], [0.0, 1.0, 0.0], [0.1; 3]);
        near(&o[0..4], &[0.0, 1.0, 0.0, 1.0]);
        near(&o[4..8], &[0.05, 1.0, 0.0, 1.0]);
        near(&o[8..12], &i.data[8..12]);
    }

    #[test]
    fn alien_map_leaves_unselected_channels_and_alpha() {
        let (i, ctx) = img(&[[0.25, 0.3, 0.4, 0.7]]);
        let o = alien_map(&i, i.rect, &ctx, [1.0; 3], [0.0; 3], [true, false, false]);
        near(&o, &[0.5 + 0.5 * (std::f32::consts::TAU * 0.25).sin(), 0.3, 0.4, 0.7]);
    }

    #[test]
    fn component_extract_reads_each_component() {
        let c = [0.2, 0.4, 0.6, 0.5];
        for (kind, want) in [
            (Component::Red, 0.2),
            (Component::Blue, 0.6),
            (Component::Value, 0.6),
            (Component::Lightness, 0.4),
            (Component::Black, 0.4),
            (Component::Alpha, 0.5),
        ] {
            assert!((component_of(kind, c) - want).abs() < 1e-5, "{kind:?}");
        }
        let (i, ctx) = img(&[c]);
        let o = component_extract(&i, i.rect, &ctx, Component::Red, true);
        near(&o, &[0.8, 0.8, 0.8, 0.5]);
        let a = component_extract(&i, i.rect, &ctx, Component::Alpha, false);
        near(&a, &[0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn hsv_round_trips() {
        for c in [[0.1, 0.5, 0.9], [1.0, 0.0, 0.0], [0.3, 0.3, 0.3], [0.0, 0.7, 0.2]] {
            let [h, s, v] = to_hsv(c);
            near(&from_hsv(h, s, v), &c);
        }
    }
}
