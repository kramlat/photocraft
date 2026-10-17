//! The canvas the dabs are drawn on (a port of libmypaint's tiled surface): 64 x 64 pixel tiles of
//! premultiplied RGBA in f32, loaded lazily from a layer's pixels, drawn on, and written back.
//! A dab's mask, the blend modes (normal, eraser, lock alpha, colorize, posterize) and the
//! spectral "pigment" versions of them follow `render_dab_mask` and `brushmodes.c`.

use std::collections::HashMap;

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_raster::Surface;

use super::brush::DabSurface;
use super::color::{rgb_to_spectral, spectral_to_rgb};

pub const TILE: i32 = 64;

struct Tile {
    /// Premultiplied RGBA, row-major.
    px: Vec<f32>,
    dirty: bool,
}

pub struct TileSurface {
    base: Surface,
    bounds: Rect,
    tiles: HashMap<(i32, i32), Tile>,
    /// Pixels touched since the last [`Self::take_dirty`].
    dirty: Rect,
    /// A tiny deterministic generator for `get_color`'s sampling (libmypaint uses `rand()`).
    sample_seed: u32,
    /// Sample every pixel under a smudge (libmypaint samples a sparse random subset): for
    /// comparing against the C library with its sampling made exhaustive too.
    pub exhaustive_sampling: bool,
}

impl TileSurface {
    /// Paints over `base` (RGB, gray, with or without alpha) inside `bounds`.
    pub fn new(base: Surface, bounds: Rect) -> Self {
        TileSurface { base, bounds, tiles: HashMap::new(), dirty: Rect::EMPTY, sample_seed: 0x2545_f491, exhaustive_sampling: false }
    }

    fn tile_rect(tx: i32, ty: i32) -> Rect {
        Rect::new(tx * TILE, ty * TILE, (tx + 1) * TILE, (ty + 1) * TILE)
    }

    fn load(&mut self, tx: i32, ty: i32) -> &mut Tile {
        let (base, bounds) = (&self.base, self.bounds);
        self.tiles.entry((tx, ty)).or_insert_with(|| {
            let r = Self::tile_rect(tx, ty);
            let mut px = vec![0.0f32; (TILE * TILE * 4) as usize];
            let inside = r.intersect(&bounds);
            if !inside.is_empty() {
                let ch = base.channels();
                let fmt = base.format();
                let data = base.read_region(inside);
                let w = inside.width() as usize;
                for y in 0..inside.height() as usize {
                    for x in 0..w {
                        let s = &data[(y * w + x) * ch..(y * w + x + 1) * ch];
                        let (rgb, a) = match (ch, fmt.alpha) {
                            (4, true) => ([s[0], s[1], s[2]], s[3]),
                            (3, false) => ([s[0], s[1], s[2]], 1.0),
                            (2, true) => ([s[0]; 3], s[1]),
                            (1, false) => ([s[0]; 3], 1.0),
                            _ => ([s[0]; 3], 1.0),
                        };
                        let o = (((inside.y0 - r.y0) as usize + y) * TILE as usize + (inside.x0 - r.x0) as usize + x) * 4;
                        px[o] = rgb[0] * a;
                        px[o + 1] = rgb[1] * a;
                        px[o + 2] = rgb[2] * a;
                        px[o + 3] = a;
                    }
                }
            }
            Tile { px, dirty: false }
        })
    }

    /// The rectangle painted since the last call (pixel coordinates), emptied.
    pub fn take_dirty(&mut self) -> Rect {
        std::mem::replace(&mut self.dirty, Rect::EMPTY)
    }

    /// Writes the painted pixels of `area` (inside the surface bounds) from the tiles into `target`,
    /// converting back to straight alpha and the target's channels. `selection` (a coverage mask)
    /// limits where paint lands, and `lock_alpha` keeps the target's alpha (and leaves fully
    /// transparent pixels alone). Tiles never painted leave `target` as it is.
    pub fn write_into(&self, target: &mut Surface, area: Rect, selection: Option<&Surface>, lock_alpha: bool) {
        let fmt: PixelFormat = target.format();
        let ch = target.channels();
        let area = area.intersect(&self.bounds);
        if area.is_empty() {
            return;
        }
        let mut data = target.read_region(area);
        let sel = selection.map(|m| m.read_region(area));
        let sel_ch = selection.map_or(1, Surface::channels);
        let w = area.width() as usize;
        for (&(tx, ty), tile) in &self.tiles {
            if !tile.dirty {
                continue;
            }
            let r = Self::tile_rect(tx, ty).intersect(&area);
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    let i = (y - area.y0) as usize * w + (x - area.x0) as usize;
                    let s = &tile.px[(((y - ty * TILE) * TILE + (x - tx * TILE)) * 4) as usize..][..4];
                    let a = s[3];
                    let rgb = if a > 0.0 { [(s[0] / a).clamp(0.0, 1.0), (s[1] / a).clamp(0.0, 1.0), (s[2] / a).clamp(0.0, 1.0)] } else { [0.0; 3] };
                    let d = &mut data[i * ch..][..ch];
                    // What the pixel becomes, then the selection's share of that change.
                    let old: Vec<f32> = d.to_vec();
                    let old_a = if fmt.alpha { old[ch - 1] } else { 1.0 };
                    if lock_alpha && old_a <= 0.0 {
                        continue;
                    }
                    let a_new = if lock_alpha { old_a } else { a };
                    let mut new = old.clone();
                    match (ch, fmt.alpha) {
                        (4, true) => new.copy_from_slice(&[rgb[0], rgb[1], rgb[2], a_new]),
                        (3, false) => new.copy_from_slice(&rgb),
                        (2, true) => new.copy_from_slice(&[(rgb[0] + rgb[1] + rgb[2]) / 3.0, a_new]),
                        (1, false) => new[0] = (rgb[0] + rgb[1] + rgb[2]) / 3.0,
                        _ => {}
                    }
                    let k = sel.as_ref().map_or(1.0, |m| m[i * sel_ch]);
                    for c in 0..ch {
                        d[c] = old[c] + (new[c] - old[c]) * k;
                    }
                }
            }
        }
        target.write_region(area, &data);
    }

    /// Straight RGBA of the pixel at `(x, y)` after painting (for tests).
    pub fn pixel(&mut self, x: i32, y: i32) -> [f32; 4] {
        let (tx, ty) = (x.div_euclid(TILE), y.div_euclid(TILE));
        let t = self.load(tx, ty);
        let s = &t.px[(((y - ty * TILE) * TILE + (x - tx * TILE)) * 4) as usize..][..4];
        let a = s[3];
        if a > 0.0 { [s[0] / a, s[1] / a, s[2] / a, a] } else { [0.0; 4] }
    }

    fn next_rand(&mut self) -> f32 {
        self.sample_seed = self.sample_seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.sample_seed >> 8) as f32 / (1u32 << 24) as f32
    }
}

#[allow(clippy::too_many_arguments)]
fn calculate_rr(xp: i32, yp: i32, x: f32, y: f32, aspect_ratio: f32, sn: f32, cs: f32, one_over_radius2: f32) -> f32 {
    let yy = yp as f32 + 0.5 - y;
    let xx = xp as f32 + 0.5 - x;
    let yyr = (yy * cs - xx * sn) * aspect_ratio;
    let xxr = yy * sn + xx * cs;
    (yyr * yyr + xxr * xxr) * one_over_radius2
}

fn calculate_r_sample(x: f32, y: f32, aspect_ratio: f32, sn: f32, cs: f32) -> f32 {
    let yyr = (y * cs - x * sn) * aspect_ratio;
    let xxr = y * sn + x * cs;
    yyr * yyr + xxr * xxr
}

fn sign_point_in_line(px: f32, py: f32, vx: f32, vy: f32) -> f32 {
    (px - vx) * (-vy) - vx * (py - vy)
}

fn closest_point_to_line(lx: f32, ly: f32, px: f32, py: f32) -> (f32, f32) {
    let l2 = lx * lx + ly * ly;
    let t = (px * lx + py * ly) / l2;
    (lx * t, ly * t)
}

#[allow(clippy::too_many_arguments)]
fn calculate_rr_antialiased(xp: i32, yp: i32, x: f32, y: f32, aspect_ratio: f32, sn: f32, cs: f32, one_over_radius2: f32, r_aa_start: f32) -> f32 {
    let pixel_right = x - xp as f32;
    let pixel_bottom = y - yp as f32;
    let pixel_center_x = pixel_right - 0.5;
    let pixel_center_y = pixel_bottom - 0.5;
    let pixel_left = pixel_right - 1.0;
    let pixel_top = pixel_bottom - 1.0;
    let (nearest_x, nearest_y, rr_near);
    if pixel_left < 0.0 && pixel_right > 0.0 && pixel_top < 0.0 && pixel_bottom > 0.0 {
        nearest_x = 0.0;
        nearest_y = 0.0;
        rr_near = 0.0;
    } else {
        let (nx, ny) = closest_point_to_line(cs, sn, pixel_center_x, pixel_center_y);
        nearest_x = nx.clamp(pixel_left, pixel_right);
        nearest_y = ny.clamp(pixel_top, pixel_bottom);
        rr_near = calculate_r_sample(nearest_x, nearest_y, aspect_ratio, sn, cs) * one_over_radius2;
    }
    if rr_near > 1.0 {
        return rr_near;
    }
    let center_sign = sign_point_in_line(pixel_center_x, pixel_center_y, cs, -sn);
    let rad_area_1 = (1.0 / std::f32::consts::PI).sqrt();
    let (farthest_x, farthest_y) = if center_sign < 0.0 { (nearest_x - sn * rad_area_1, nearest_y + cs * rad_area_1) } else { (nearest_x + sn * rad_area_1, nearest_y - cs * rad_area_1) };
    let r_far = calculate_r_sample(farthest_x, farthest_y, aspect_ratio, sn, cs);
    let rr_far = r_far * one_over_radius2;
    if r_far < r_aa_start {
        return (rr_far + rr_near) * 0.5;
    }
    let mut visibility_near = 1.0 - rr_near;
    let delta = rr_far - rr_near;
    visibility_near /= 1.0 + delta;
    1.0 - visibility_near
}

#[allow(clippy::too_many_arguments)]
fn calculate_opa(rr: f32, hardness: f32, s1_off: f32, s1_slope: f32, s2_off: f32, s2_slope: f32) -> f32 {
    let fac = if rr <= hardness { s1_slope } else { s2_slope };
    let mut opa = if rr <= hardness { s1_off } else { s2_off };
    opa += rr * fac;
    if rr > 1.0 {
        opa = 0.0;
    }
    opa
}

/// The dab's opacity at every pixel of `area` (row-major), 0 outside the dab.
#[allow(clippy::too_many_arguments)]
fn render_dab_mask(area: Rect, x: f32, y: f32, radius: f32, hardness: f32, softness: f32, aspect_ratio: f32, angle: f32) -> Vec<f32> {
    let hardness = hardness.clamp(0.0, 1.0);
    let aspect_ratio = aspect_ratio.max(1.0);
    let s1_off = 1.0 - softness;
    let s1_slope = -(1.0 / hardness - 1.0) * (1.0 - softness);
    let s2_off = hardness / (1.0 - hardness) * (1.0 - softness);
    let s2_slope = -hardness / (1.0 - hardness) * (1.0 - softness);
    let angle_rad = angle / 360.0 * 2.0 * std::f32::consts::PI;
    let (cs, sn) = (angle_rad.cos(), angle_rad.sin());
    let r_fringe = radius + 1.0;
    let (x0, y0) = ((x - r_fringe).floor() as i32, (y - r_fringe).floor() as i32);
    let (x1, y1) = ((x + r_fringe).floor() as i32, (y + r_fringe).floor() as i32);
    let one_over_radius2 = 1.0 / (radius * radius);
    let w = area.width() as usize;
    let mut out = vec![0.0f32; w * area.height() as usize];
    let small = radius < 3.0;
    let mut r_aa_start = if radius > 1.0 { radius - 1.0 } else { 0.0 };
    r_aa_start *= r_aa_start / aspect_ratio;
    for yp in y0.max(area.y0)..=y1.min(area.y1 - 1) {
        for xp in x0.max(area.x0)..=x1.min(area.x1 - 1) {
            let rr = if small {
                calculate_rr_antialiased(xp, yp, x, y, aspect_ratio, sn, cs, one_over_radius2, r_aa_start)
            } else {
                calculate_rr(xp, yp, x, y, aspect_ratio, sn, cs, one_over_radius2)
            };
            let opa = calculate_opa(rr, hardness, s1_off, s1_slope, s2_off, s2_slope);
            // libmypaint stores 15-bit integers: anything below one step is no dab at all.
            if (opa * 32768.0) as u32 > 0 {
                out[(yp - area.y0) as usize * w + (xp - area.x0) as usize] = opa;
            }
        }
    }
    out
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// `top` with the luminosity of `bot` (libmypaint's `set_rgb16_lum_from_rgb16`, in floats).
fn set_lum(top: [f32; 3], bot: [f32; 3]) -> [f32; 3] {
    let diff = luma(bot) - luma(top);
    let mut c = [top[0] + diff, top[1] + diff, top[2] + diff];
    let lum = luma(c);
    let cmin = c[0].min(c[1]).min(c[2]);
    let cmax = c[0].max(c[1]).max(c[2]);
    if cmin < 0.0 {
        for v in &mut c {
            *v = lum + ((*v - lum) * lum) / (lum - cmin);
        }
    }
    if cmax > 1.0 {
        for v in &mut c {
            *v = lum + ((*v - lum) * (1.0 - lum)) / (cmax - lum);
        }
    }
    c
}

fn spectral_blend_factor(x: f32) -> f32 {
    let (ver_fac, hor_fac, hor_offs) = (1.65f32, 8.0f32, 3.0f32);
    let b = x * hor_fac - hor_offs;
    0.5 + b / (1.0 + b.abs() * ver_fac)
}

/// Blends `color` into the premultiplied pixel `px` through mask value `m` the way `op` says.
struct Dab {
    color: [f32; 3],
    spectral: [f32; 10],
    color_a: f32,
    opaque: f32,
    paint: f32,
    normal: f32,
    lock_alpha: f32,
    colorize: f32,
    posterize: f32,
    posterize_num: f32,
}

impl Dab {
    fn apply(&self, px: &mut [f32], m: f32) {
        if self.paint < 1.0 {
            if self.normal != 0.0 {
                let opacity = self.normal * self.opaque * (1.0 - self.paint);
                if self.color_a == 1.0 {
                    normal(px, m, opacity, self.color);
                } else {
                    normal_eraser(px, m, opacity, self.color, self.color_a);
                }
            }
            if self.lock_alpha != 0.0 && self.color_a != 0.0 {
                lock_alpha(px, m, self.lock_alpha * self.opaque * (1.0 - self.colorize) * (1.0 - self.posterize) * (1.0 - self.paint), self.color);
            }
        }
        if self.paint > 0.0 {
            if self.normal != 0.0 {
                let opacity = self.normal * self.opaque * self.paint;
                if self.color_a == 1.0 {
                    normal_paint(px, m, opacity, self.color, &self.spectral);
                } else {
                    normal_eraser_paint(px, m, opacity, self.color, &self.spectral, self.color_a);
                }
            }
            if self.lock_alpha != 0.0 && self.color_a != 0.0 {
                lock_alpha_paint(px, m, self.lock_alpha * self.opaque * (1.0 - self.colorize) * (1.0 - self.posterize) * self.paint, self.color, &self.spectral);
            }
        }
        if self.colorize != 0.0 {
            colorize(px, m, self.colorize * self.opaque, self.color);
        }
        if self.posterize != 0.0 {
            posterize(px, m, self.posterize * self.opaque, self.posterize_num);
        }
    }
}

fn normal(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3]) {
    let opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    px[3] = opa_a + opa_b * px[3];
    for i in 0..3 {
        px[i] = opa_a * c[i] + opa_b * px[i];
    }
}

fn normal_eraser(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3], color_a: f32) {
    let mut opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    opa_a *= color_a;
    px[3] = opa_a + opa_b * px[3];
    for i in 0..3 {
        px[i] = opa_a * c[i] + opa_b * px[i];
    }
}

fn pigment(px: &mut [f32], opa_a: f32, opa_b: f32, a_spectral: &[f32; 10]) {
    let fac_a = opa_a / (opa_a + opa_b * px[3]);
    let fac_b = 1.0 - fac_a;
    let mut sb = [0.0f32; 10];
    rgb_to_spectral(px[0] / px[3], px[1] / px[3], px[2] / px[3], &mut sb);
    let mut res = [0.0f32; 10];
    for i in 0..10 {
        res[i] = a_spectral[i].powf(fac_a) * sb[i].powf(fac_b);
    }
    let rgb = spectral_to_rgb(&res);
    px[3] = opa_a + opa_b * px[3];
    for i in 0..3 {
        px[i] = rgb[i] * px[3];
    }
}

fn normal_paint(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3], spectral: &[f32; 10]) {
    let opacity = opacity.max(150.0 / 32768.0);
    let opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    if px[3] <= 0.0 {
        px[3] = opa_a + opa_b * px[3];
        for i in 0..3 {
            px[i] = opa_a * c[i] + opa_b * px[i];
        }
        return;
    }
    pigment(px, opa_a, opa_b, spectral);
}

fn normal_eraser_paint(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3], spectral: &[f32; 10], color_a: f32) {
    let opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    let opa_a2 = opa_a * color_a;
    let opa_out = opa_a2 + opa_b * px[3];
    let mut rgb = [0.0f32; 3];
    let spectral_factor = spectral_blend_factor(px[3]).clamp(0.0, 1.0);
    let additive_factor = 1.0 - spectral_factor;
    if additive_factor != 0.0 {
        for i in 0..3 {
            rgb[i] = opa_a2 * c[i] + opa_b * px[i];
        }
    }
    if spectral_factor != 0.0 && px[3] != 0.0 {
        let mut sb = [0.0f32; 10];
        rgb_to_spectral(px[0] / px[3], px[1] / px[3], px[2] / px[3], &mut sb);
        let mut fac_a = opa_a / (opa_a + opa_b * px[3]);
        fac_a *= color_a;
        let fac_b = 1.0 - fac_a;
        let mut res = [0.0f32; 10];
        for i in 0..10 {
            res[i] = spectral[i].powf(fac_a) * sb[i].powf(fac_b);
        }
        let out = spectral_to_rgb(&res);
        for i in 0..3 {
            rgb[i] = additive_factor * rgb[i] + spectral_factor * out[i] * opa_out;
        }
    }
    px[3] = opa_out;
    px[..3].copy_from_slice(&rgb);
}

fn lock_alpha(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3]) {
    let mut opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    opa_a *= px[3];
    for i in 0..3 {
        px[i] = opa_a * c[i] + opa_b * px[i];
    }
}

fn lock_alpha_paint(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3], spectral: &[f32; 10]) {
    let opacity = opacity.max(150.0 / 32768.0);
    let mut opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    opa_a *= px[3];
    if px[3] <= 0.0 {
        for i in 0..3 {
            px[i] = opa_a * c[i] + opa_b * px[i];
        }
        return;
    }
    let fac_a = opa_a / (opa_a + opa_b * px[3]);
    let fac_b = 1.0 - fac_a;
    let mut sb = [0.0f32; 10];
    rgb_to_spectral(px[0] / px[3], px[1] / px[3], px[2] / px[3], &mut sb);
    let mut res = [0.0f32; 10];
    for i in 0..10 {
        res[i] = spectral[i].powf(fac_a) * sb[i].powf(fac_b);
    }
    let rgb = spectral_to_rgb(&res);
    for i in 0..3 {
        px[i] = rgb[i] * px[3];
    }
}

fn colorize(px: &mut [f32], m: f32, opacity: f32, c: [f32; 3]) {
    let a = px[3];
    let bot = if a != 0.0 { [px[0] / a, px[1] / a, px[2] / a] } else { [0.0; 3] };
    let set = set_lum(c, bot);
    let rgb = [set[0] * a, set[1] * a, set[2] * a];
    let opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    for i in 0..3 {
        px[i] = opa_a * rgb[i] + opa_b * px[i];
    }
}

fn posterize(px: &mut [f32], m: f32, opacity: f32, num: f32) {
    let opa_a = m * opacity;
    let opa_b = 1.0 - opa_a;
    for v in px.iter_mut().take(3) {
        let post = ((*v * num + 0.5) as i32) as f32 / num;
        *v = opa_a * post + opa_b * *v;
    }
}

impl DabSurface for TileSurface {
    fn draw_dab(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        color: [f32; 3],
        opaque: f32,
        hardness: f32,
        softness: f32,
        alpha_eraser: f32,
        aspect_ratio: f32,
        angle: f32,
        lock_alpha: f32,
        colorize: f32,
        posterize: f32,
        posterize_num: f32,
        paint: f32,
    ) -> bool {
        let opaque = opaque.clamp(0.0, 1.0);
        let hardness = hardness.clamp(0.0, 1.0);
        let softness = softness.clamp(0.0, 1.0);
        let lock_alpha = lock_alpha.clamp(0.0, 1.0);
        let colorize = colorize.clamp(0.0, 1.0);
        let posterize = posterize.clamp(0.0, 1.0);
        let posterize_num = ((posterize_num * 100.0 + 0.5) as i32).clamp(1, 128) as f32;
        let paint = paint.clamp(0.0, 1.0);
        if radius < 0.1 || hardness == 0.0 || softness == 1.0 || opaque == 0.0 {
            return false;
        }
        let color = color.map(|c| c.clamp(0.0, 1.0));
        let mut spectral = [0.0f32; 10];
        rgb_to_spectral(color[0], color[1], color[2], &mut spectral);
        let normal = (1.0 - lock_alpha) * (1.0 - colorize) * (1.0 - posterize);
        let dab = Dab { color, spectral, color_a: alpha_eraser.clamp(0.0, 1.0), opaque, paint, normal, lock_alpha, colorize, posterize, posterize_num };
        let r_fringe = radius + 1.0;
        let (x0, y0) = ((x - r_fringe).floor() as i32, (y - r_fringe).floor() as i32);
        let (x1, y1) = ((x + r_fringe).floor() as i32, (y + r_fringe).floor() as i32);
        let area = Rect::new(x0, y0, x1 + 1, y1 + 1).intersect(&self.bounds);
        if area.is_empty() {
            return true;
        }
        let mask = render_dab_mask(area, x, y, radius, hardness, softness, aspect_ratio, angle);
        let w = area.width() as usize;
        for ty in area.y0.div_euclid(TILE)..=(area.y1 - 1).div_euclid(TILE) {
            for tx in area.x0.div_euclid(TILE)..=(area.x1 - 1).div_euclid(TILE) {
                let r = Self::tile_rect(tx, ty).intersect(&area);
                let tile = self.load(tx, ty);
                for yp in r.y0..r.y1 {
                    for xp in r.x0..r.x1 {
                        let m = mask[(yp - area.y0) as usize * w + (xp - area.x0) as usize];
                        if m == 0.0 {
                            continue;
                        }
                        let o = (((yp - ty * TILE) * TILE + (xp - tx * TILE)) * 4) as usize;
                        dab.apply(&mut tile.px[o..o + 4], m);
                    }
                }
                tile.dirty = true;
            }
        }
        self.dirty = self.dirty.union(&area);
        true
    }

    fn get_color(&mut self, x: f32, y: f32, radius: f32, paint: f32) -> [f32; 4] {
        let radius = radius.max(1.0);
        let (hardness, softness) = (0.5f32, 0.5f32);
        let (mut sum_weight, mut sum_r, mut sum_g, mut sum_b, mut sum_a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let r_fringe = radius + 1.0;
        let (x0, y0) = ((x - r_fringe).floor() as i32, (y - r_fringe).floor() as i32);
        let (x1, y1) = ((x + r_fringe).floor() as i32, (y + r_fringe).floor() as i32);
        let area = Rect::new(x0, y0, x1 + 1, y1 + 1).intersect(&self.bounds);
        let sample_interval = if radius <= 2.0 || self.exhaustive_sampling { 1 } else { (radius * 7.0) as i32 };
        let random_sample_rate = 1.0 / (7.0 * radius);
        if !area.is_empty() {
            let mask = render_dab_mask(area, x, y, radius, hardness, softness, 1.0, 0.0);
            let w = area.width() as usize;
            for ty in area.y0.div_euclid(TILE)..=(area.y1 - 1).div_euclid(TILE) {
                for tx in area.x0.div_euclid(TILE)..=(area.x1 - 1).div_euclid(TILE) {
                    let r = Self::tile_rect(tx, ty).intersect(&area);
                    // Copy the tile's pixels first: the sampling below also draws random numbers.
                    let px: Vec<f32> = self.load(tx, ty).px.clone();
                    let mut avg_spectral = [0.0f32; 10];
                    let mut avg_rgb = [sum_r, sum_g, sum_b];
                    if paint > 0.0 {
                        rgb_to_spectral(sum_r, sum_g, sum_b, &mut avg_spectral);
                    }
                    let mut interval_counter = 0;
                    for yp in r.y0..r.y1 {
                        for xp in r.x0..r.x1 {
                            let m = mask[(yp - area.y0) as usize * w + (xp - area.x0) as usize];
                            if m == 0.0 {
                                continue;
                            }
                            let o = (((yp - ty * TILE) * TILE + (xp - tx * TILE)) * 4) as usize;
                            let p = &px[o..o + 4];
                            if paint < 0.0 {
                                sum_weight += m;
                                sum_r += m * p[0];
                                sum_g += m * p[1];
                                sum_b += m * p[2];
                                sum_a += m * p[3];
                                continue;
                            }
                            if interval_counter == 0 || self.next_rand() < random_sample_rate {
                                let a = m * p[3];
                                let alpha_sums = a + sum_a;
                                sum_weight += m;
                                let (mut fac_a, mut fac_b) = (1.0f32, 1.0f32);
                                if alpha_sums > 0.0 {
                                    fac_a = a / alpha_sums;
                                    fac_b = 1.0 - fac_a;
                                }
                                if paint > 0.0 && p[3] > 0.0 {
                                    let mut s = [0.0f32; 10];
                                    rgb_to_spectral(p[0] / p[3], p[1] / p[3], p[2] / p[3], &mut s);
                                    for i in 0..10 {
                                        avg_spectral[i] = s[i].powf(fac_a) * avg_spectral[i].powf(fac_b);
                                    }
                                }
                                if paint < 1.0 && p[3] > 0.0 {
                                    for i in 0..3 {
                                        avg_rgb[i] = p[i] * fac_a / p[3] + avg_rgb[i] * fac_b;
                                    }
                                }
                                sum_a += a;
                            }
                            interval_counter = (interval_counter + 1) % sample_interval;
                        }
                    }
                    if paint >= 0.0 {
                        let spec_rgb = spectral_to_rgb(&avg_spectral);
                        sum_r = spec_rgb[0] * paint + (1.0 - paint) * avg_rgb[0];
                        sum_g = spec_rgb[1] * paint + (1.0 - paint) * avg_rgb[1];
                        sum_b = spec_rgb[2] * paint + (1.0 - paint) * avg_rgb[2];
                    }
                }
            }
        }
        if sum_weight <= 0.0 {
            return [0.0, 1.0, 0.0, 0.0];
        }
        sum_a /= sum_weight;
        if paint < 0.0 {
            sum_r /= sum_weight;
            sum_g /= sum_weight;
            sum_b /= sum_weight;
        }
        let a = sum_a.clamp(0.0, 1.0);
        if sum_a > 0.0 {
            let demul = if paint < 0.0 { sum_a } else { 1.0 };
            [(sum_r / demul).clamp(0.0, 1.0), (sum_g / demul).clamp(0.0, 1.0), (sum_b / demul).clamp(0.0, 1.0), a]
        } else {
            [0.0, 1.0, 0.0, a]
        }
    }
}
