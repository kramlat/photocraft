//! Colour helpers: HSV and HSL conversions as libmypaint does them, and the ten-band spectral
//! upsampling its "pigment" paint mode mixes with (a weighted geometric mean of reflectances).
#![allow(clippy::excessive_precision)] // the spectral tables are libmypaint's, digit for digit

pub const WGM_EPSILON: f32 = 0.001;

fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x > hi {
        hi
    } else if x < lo {
        lo
    } else {
        x
    }
}

/// `a mod n`, the result taking the sign of `n` (floor modulo).
pub fn mod_arith(a: f32, n: f32) -> f32 {
    a - n * (a / n).floor()
}

pub fn smallest_angular_difference(a: f32, b: f32) -> f32 {
    let mut d = b - a;
    d = mod_arith(d + 180.0, 360.0) - 180.0;
    d += if d > 180.0 {
        -360.0
    } else if d < -180.0 {
        360.0
    } else {
        0.0
    };
    d
}

pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> [f32; 3] {
    let (r, g, b) = (clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta > 0.0001 {
        let s = delta / max;
        let mut h = if r == max {
            let h = (g - b) / delta;
            if h < 0.0 { h + 6.0 } else { h }
        } else if g == max {
            2.0 + (b - r) / delta
        } else {
            4.0 + (r - g) / delta
        };
        h /= 6.0;
        [h, s, max]
    } else {
        [0.0, 0.0, max]
    }
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h - h.floor();
    let (s, v) = (clamp(s, 0.0, 1.0), clamp(v, 0.0, 1.0));
    if s == 0.0 {
        return [v, v, v];
    }
    let mut hue = f64::from(h);
    if hue == 1.0 {
        hue = 0.0;
    }
    hue *= 6.0;
    let i = hue as i32;
    let f = hue - f64::from(i);
    let (v64, s64) = (f64::from(v), f64::from(s));
    let w = v64 * (1.0 - s64);
    let q = v64 * (1.0 - s64 * f);
    let t = v64 * (1.0 - s64 * (1.0 - f));
    let (r, g, b) = match i {
        0 => (v64, t, w),
        1 => (q, v64, w),
        2 => (w, v64, t),
        3 => (w, q, v64),
        4 => (t, w, v64),
        _ => (v64, w, q),
    };
    [r as f32, g as f32, b as f32]
}

pub fn rgb_to_hsl(r: f32, g: f32, b: f32) -> [f32; 3] {
    let (r, g, b) = (clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0));
    let (max, min) = (f64::from(r.max(g).max(b)), f64::from(r.min(g).min(b)));
    let l = ((max + min) / 2.0) as f32;
    if max == min {
        return [0.0, 0.0, l];
    }
    let s = if l <= 0.5 { (max - min) / (max + min) } else { (max - min) / (2.0 - max - min) } as f32;
    let mut delta = max - min;
    if delta == 0.0 {
        delta = 1.0;
    }
    let (rd, gd, bd) = (f64::from(r), f64::from(g), f64::from(b));
    let mut h = if rd == max {
        (gd - bd) / delta
    } else if gd == max {
        2.0 + (bd - rd) / delta
    } else {
        4.0 + (rd - gd) / delta
    };
    h /= 6.0;
    if h < 0.0 {
        h += 1.0;
    }
    [h as f32, s, l]
}

fn hsl_value(n1: f64, n2: f64, mut hue: f64) -> f64 {
    if hue > 6.0 {
        hue -= 6.0;
    } else if hue < 0.0 {
        hue += 6.0;
    }
    if hue < 1.0 {
        n1 + (n2 - n1) * hue
    } else if hue < 3.0 {
        n2
    } else if hue < 4.0 {
        n1 + (n2 - n1) * (4.0 - hue)
    } else {
        n1
    }
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = h - h.floor();
    let (s, l) = (clamp(s, 0.0, 1.0), clamp(l, 0.0, 1.0));
    if s == 0.0 {
        return [l, l, l];
    }
    let (s64, l64) = (f64::from(s), f64::from(l));
    let m2 = if l <= 0.5 { l64 * (1.0 + s64) } else { l64 + s64 - l64 * s64 };
    let m1 = 2.0 * l64 - m2;
    let h6 = f64::from(h) * 6.0;
    [hsl_value(m1, m2, h6 + 2.0) as f32, hsl_value(m1, m2, h6) as f32, hsl_value(m1, m2, h6 - 2.0) as f32]
}

const T_MATRIX_SMALL: [[f32; 10]; 3] = [
    [0.026595621243689, 0.049779426257903, 0.022449850859496, -0.218453689278271, -0.256894883201278, 0.445881722194840, 0.772365886289756, 0.194498761382537, 0.014038157587820, 0.007687264480513],
    [-0.032601672674412, -0.061021043498478, -0.052490001018404, 0.206659098273522, 0.572496335158169, 0.317837248815438, -0.021216624031211, -0.019387668756117, -0.001521339050858, -0.000835181622534],
    [0.339475473216284, 0.635401374177222, 0.771520797089589, 0.113222640692379, -0.055251113343776, -0.048222578468680, -0.012966666339586, -0.001523814504223, -0.000094718948810, -0.000051604594741],
];
const SPECTRAL_R: [f32; 10] = [0.009281362787953, 0.009732627042016, 0.011254252737167, 0.015105578649573, 0.024797924177217, 0.083622585502406, 0.977865045723212, 1.000000000000000, 0.999961046144372, 0.999999992756822];
const SPECTRAL_G: [f32; 10] = [0.002854127435775, 0.003917589679914, 0.012132151699187, 0.748259205918013, 1.000000000000000, 0.865695937531795, 0.037477469241101, 0.022816789725717, 0.021747419446456, 0.021384940572308];
const SPECTRAL_B: [f32; 10] = [0.537052150373386, 0.546646402401469, 0.575501819073983, 0.258778829633924, 0.041709923751716, 0.012662638828324, 0.007485593127390, 0.006766900622462, 0.006699764779016, 0.006676219883241];

/// Adds the spectral reflectance of an RGB colour into `out` (which starts zeroed).
pub fn rgb_to_spectral(r: f32, g: f32, b: f32, out: &mut [f32; 10]) {
    let offset = 1.0 - WGM_EPSILON;
    let (r, g, b) = (r * offset + WGM_EPSILON, g * offset + WGM_EPSILON, b * offset + WGM_EPSILON);
    for i in 0..10 {
        out[i] += SPECTRAL_R[i] * r + SPECTRAL_G[i] * g + SPECTRAL_B[i] * b;
    }
}

pub fn spectral_to_rgb(s: &[f32; 10]) -> [f32; 3] {
    let offset = 1.0 - WGM_EPSILON;
    let mut tmp = [0.0f32; 3];
    for i in 0..10 {
        for (c, t) in tmp.iter_mut().enumerate() {
            *t += T_MATRIX_SMALL[c][i] * s[i];
        }
    }
    tmp.map(|v| clamp((v - WGM_EPSILON) / offset, 0.0, 1.0))
}

/// Mixes two RGBA colours, `a` (weight `fac`) and `b`: weighted geometric mean of their spectra
/// by `paint_mode`, plain additive mixing by `1 - paint_mode`.
pub fn mix_colors(a: [f32; 4], b: [f32; 4], fac: f32, paint_mode: f32) -> [f32; 4] {
    let opa_a = fac;
    let opa_b = 1.0 - opa_a;
    let mut result = [0.0f32; 4];
    result[3] = clamp(opa_a * a[3] + opa_b * b[3], 0.0, 1.0);
    let sfac_a = if a[3] == 0.0 { 0.0 } else { opa_a * a[3] / (a[3] + b[3] * opa_b) };
    let sfac_b = 1.0 - sfac_a;
    if paint_mode > 0.0 {
        let (mut sa, mut sb) = ([0.0f32; 10], [0.0f32; 10]);
        rgb_to_spectral(a[0], a[1], a[2], &mut sa);
        rgb_to_spectral(b[0], b[1], b[2], &mut sb);
        let mut mix = [0.0f32; 10];
        for i in 0..10 {
            mix[i] = sa[i].powf(sfac_a) * sb[i].powf(sfac_b);
        }
        let rgb = spectral_to_rgb(&mix);
        result[..3].copy_from_slice(&rgb);
    }
    if paint_mode < 1.0 {
        for i in 0..3 {
            result[i] = result[i] * paint_mode + (1.0 - paint_mode) * (a[i] * opa_a + b[i] * opa_b);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_and_hsl_round_trip() {
        for c in [[0.9, 0.2, 0.1], [0.1, 0.8, 0.3], [0.3, 0.3, 0.9], [0.5, 0.5, 0.5], [1.0, 1.0, 0.0]] {
            let h = rgb_to_hsv(c[0], c[1], c[2]);
            let back = hsv_to_rgb(h[0], h[1], h[2]);
            let l = rgb_to_hsl(c[0], c[1], c[2]);
            let back_l = hsl_to_rgb(l[0], l[1], l[2]);
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-5 && (back_l[k] - c[k]).abs() < 1e-5, "{c:?} {back:?} {back_l:?}");
            }
        }
    }

    #[test]
    fn pigment_mixing_blue_and_yellow_makes_green() {
        // The point of the spectral mode: subtractive mixing, where additive would give grey.
        let spectral = mix_colors([0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0], 0.5, 1.0);
        let additive = mix_colors([0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0], 0.5, 0.0);
        assert!(spectral[1] > spectral[0] && spectral[1] > spectral[2], "{spectral:?}");
        assert!((additive[0] - 0.5).abs() < 1e-6 && (additive[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn angles_wrap() {
        assert_eq!(smallest_angular_difference(170.0, -170.0), 20.0);
        assert!((mod_arith(-10.0, 360.0) - 350.0).abs() < 1e-4);
    }
}
