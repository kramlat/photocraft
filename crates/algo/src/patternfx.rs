//! Pattern renders: Plasma, Spiral, Maze and Sine Waves. Like the other renders they fill the
//! whole reference area; colours are straight sRGB RGBA.

use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::extrafx::{NoiseKind, octave};
use crate::fxutil::{set_rgba, xy};
use crate::image::Image;

/// The shape of the waves Sine Waves draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WaveShape {
    /// Straight bands at an angle.
    #[default]
    Linear,
    /// Rings around the centre.
    Radial,
    /// Bands in both directions added together (an egg-box).
    Cross,
}

fn mix(bg: [f32; 4], fg: [f32; 4], t: f32) -> [f32; 4] {
    [bg[0] + (fg[0] - bg[0]) * t, bg[1] + (fg[1] - bg[1]) * t, bg[2] + (fg[2] - bg[2]) * t, 1.0]
}

/// Plasma: smooth, swirling colour from layered noise. `turbulence` 0.1–7 sets how many
/// swirls fit the area (and how fine they get); `saturation` 0–100 how vivid.
pub(crate) fn plasma(src: &Image, out: Rect, ctx: &Ctx, turbulence: f32, saturation: f32, seed: u32) -> Vec<f32> {
    let n = src.ch;
    let b = ctx.bounds;
    let side = b.width().max(b.height()).max(1) as f32;
    let size = side / (1.0 + turbulence.clamp(0.1, 7.0));
    let sat = (saturation / 100.0).clamp(0.0, 1.0);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (fx, fy) = ((x - b.x0) as f32 / size, (y - b.y0) as f32 / size);
        let mut t = 0.0;
        let (mut amp, mut norm, mut freq) = (1.0, 0.0, 1.0);
        for o in 0..4u32 {
            t += octave(NoiseKind::Simplex, fx * freq, fy * freq, seed.wrapping_add(o * 4057)) * amp;
            norm += amp;
            amp *= 0.55;
            freq *= 2.0;
        }
        let t = t / norm;
        let wave = |k: f32| 0.5 + 0.5 * (std::f32::consts::TAU * (t * 1.5 + k)).sin();
        let rgb = [wave(0.0), wave(1.0 / 3.0), wave(2.0 / 3.0)];
        let grey = 0.5;
        set_rgba(ctx, px, [grey + (rgb[0] - grey) * sat, grey + (rgb[1] - grey) * sat, grey + (rgb[2] - grey) * sat, 1.0]);
    }
    res
}

/// Spiral: an Archimedean spiral whose arms are `spacing` px apart and `thickness` px wide,
/// turning clockwise or counter-clockwise from the centre of the reference area.
pub(crate) fn spiral(src: &Image, out: Rect, ctx: &Ctx, spacing: f32, thickness: f32, clockwise: bool, colours: ([f32; 4], [f32; 4])) -> Vec<f32> {
    let (fg, bg) = colours;
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 / 2.0, b.y0 as f32 + b.height() as f32 / 2.0);
    let spacing = spacing.clamp(2.0, 10_000.0);
    let thickness = thickness.clamp(1.0, spacing);
    let dir = if clockwise { 1.0 } else { -1.0 };
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let r = (dx * dx + dy * dy).sqrt();
        let turns = dy.atan2(dx) * dir / std::f32::consts::TAU;
        let along = (r - turns * spacing).rem_euclid(spacing);
        // The arm, with a one-pixel soft edge on both sides so it doesn't alias.
        let t = (thickness - along).clamp(0.0, 1.0).max((along - (spacing - 1.0)).clamp(0.0, 1.0));
        let c = mix(bg, fg, t);
        set_rgba(ctx, px, c);
    }
    res
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// A perfect maze on a `cols × rows` grid (depth-first, iterative): bit 0 = open to the east,
/// bit 1 = open to the south.
fn carve(cols: usize, rows: usize, seed: u32) -> Vec<u8> {
    let mut open = vec![0u8; cols * rows];
    let mut seen = vec![false; cols * rows];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (u64::from(seed) << 1 | 1));
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some(&(x, y)) = stack.last() {
        let mut next: Vec<(usize, usize, u8)> = Vec::with_capacity(4);
        if x + 1 < cols && !seen[y * cols + x + 1] {
            next.push((x + 1, y, 0));
        }
        if y + 1 < rows && !seen[(y + 1) * cols + x] {
            next.push((x, y + 1, 1));
        }
        if x > 0 && !seen[y * cols + x - 1] {
            next.push((x - 1, y, 2));
        }
        if y > 0 && !seen[(y - 1) * cols + x] {
            next.push((x, y - 1, 3));
        }
        if next.is_empty() {
            stack.pop();
            continue;
        }
        let (nx, ny, dir) = next[(rng.next() % next.len() as u64) as usize];
        match dir {
            0 => open[y * cols + x] |= 1,
            1 => open[y * cols + x] |= 2,
            2 => open[ny * cols + nx] |= 1,
            _ => open[ny * cols + nx] |= 2,
        }
        seen[ny * cols + nx] = true;
        stack.push((nx, ny));
    }
    open
}

/// Maze: a random maze of `cell`-px cells with `wall`-px walls (walls in the foreground colour,
/// passages in the background), the same maze for the same seed.
pub(crate) fn maze(src: &Image, out: Rect, ctx: &Ctx, cell: f32, wall: f32, seed: u32, colours: ([f32; 4], [f32; 4])) -> Vec<f32> {
    let (fg, bg) = colours;
    let n = src.ch;
    let b = ctx.bounds;
    let cell = cell.clamp(4.0, 2000.0).round() as i32;
    let wall = (wall.clamp(1.0, cell as f32 / 2.0).round() as i32).max(1);
    let cols = ((b.width() as i32 + cell - 1) / cell).clamp(1, 512) as usize;
    let rows = ((b.height() as i32 + cell - 1) / cell).clamp(1, 512) as usize;
    let open = carve(cols, rows, seed);
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (lx, ly) = (x - b.x0, y - b.y0);
        let (cx, cy) = ((lx / cell) as usize, (ly / cell) as usize);
        let (ix, iy) = (lx % cell, ly % cell);
        let is_wall = if cx >= cols || cy >= rows {
            true
        } else {
            // Each cell owns its top and left walls; the outer right/bottom edge closes the maze.
            let left_wall = ix < wall && (cx == 0 || open[cy * cols + cx - 1] & 1 == 0);
            let top_wall = iy < wall && (cy == 0 || open[(cy - 1) * cols + cx] & 2 == 0);
            let right_edge = cx == cols - 1 && ix >= cell - wall;
            let bottom_edge = cy == rows - 1 && iy >= cell - wall;
            left_wall || top_wall || right_edge || bottom_edge || (ix < wall && iy < wall)
        };
        let c = if is_wall { fg } else { bg };
        set_rgba(ctx, px, [c[0], c[1], c[2], 1.0]);
    }
    res
}

/// Sine Waves: bands of `wavelength` px blending the background (troughs) and foreground
/// (crests); `angle` turns linear bands, `phase` slides them; `contrast` 0–100 how deep.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sine_waves(src: &Image, out: Rect, ctx: &Ctx, shape: WaveShape, wavelength: f32, angle: f32, phase: f32, contrast: f32, colours: ([f32; 4], [f32; 4])) -> Vec<f32> {
    let (fg, bg) = colours;
    let n = src.ch;
    let b = ctx.bounds;
    let (cx, cy) = (b.x0 as f32 + b.width() as f32 / 2.0, b.y0 as f32 + b.height() as f32 / 2.0);
    let wl = wavelength.clamp(2.0, 10_000.0);
    let (sa, ca) = angle.to_radians().sin_cos();
    let ph = phase.to_radians();
    let k = (contrast / 100.0).clamp(0.0, 1.0);
    let tau = std::f32::consts::TAU;
    let mut res = src.crop(out);
    for (i, px) in res.chunks_exact_mut(n).enumerate() {
        let (x, y) = xy(out, i);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let s = match shape {
            WaveShape::Linear => (tau * (dx * ca + dy * sa) / wl + ph).sin(),
            WaveShape::Radial => (tau * (dx * dx + dy * dy).sqrt() / wl + ph).sin(),
            WaveShape::Cross => ((tau * dx / wl + ph).sin() + (tau * dy / wl + ph).sin()) / 2.0,
        };
        let t = 0.5 + 0.5 * s * k;
        set_rgba(ctx, px, mix(bg, fg, t));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::ColorMode;

    fn blank(w: i32, h: i32) -> (Image, Ctx) {
        let rect = Rect::new(0, 0, w, h);
        (Image::new(rect, 4), Ctx { bounds: rect, mode: ColorMode::Rgb, alpha: true })
    }
    const WHITE: [f32; 4] = [1.0; 4];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn plasma_fills_with_colour_and_follows_the_seed_and_saturation() {
        let (i, ctx) = blank(40, 40);
        let a = plasma(&i, i.rect, &ctx, 3.0, 100.0, 1);
        assert_eq!(a, plasma(&i, i.rect, &ctx, 3.0, 100.0, 1));
        assert_ne!(a, plasma(&i, i.rect, &ctx, 3.0, 100.0, 2));
        assert!(a.chunks(4).all(|p| p[3] == 1.0));
        let colourful = a.chunks(4).any(|p| (p[0] - p[1]).abs() > 0.3);
        assert!(colourful);
        let grey = plasma(&i, i.rect, &ctx, 3.0, 0.0, 1);
        assert!(grey.chunks(4).all(|p| (p[0] - p[1]).abs() < 1e-6 && (p[1] - p[2]).abs() < 1e-6));
    }

    #[test]
    fn a_spiral_has_both_colours_and_turns_the_requested_way() {
        let (i, ctx) = blank(61, 61);
        let cw = spiral(&i, i.rect, &ctx, 10.0, 4.0, true, (BLACK, WHITE));
        let ccw = spiral(&i, i.rect, &ctx, 10.0, 4.0, false, (BLACK, WHITE));
        let dark = cw.chunks(4).filter(|p| p[0] < 0.5).count();
        assert!(dark > 500 && dark < 3000, "{dark}");
        assert_ne!(cw, ccw);
        // Counter-clockwise is the mirror image of clockwise about the horizontal axis.
        for y in [10usize, 25, 40] {
            for x in [5usize, 17, 33] {
                assert_eq!(cw[(y * 61 + x) * 4], ccw[((60 - y) * 61 + x) * 4], "({x},{y})");
            }
        }
    }

    #[test]
    fn a_maze_is_perfect_and_deterministic() {
        // Perfect: every cell reachable and exactly cells - 1 passages.
        for seed in [1u32, 2, 3] {
            let open = carve(9, 7, seed);
            let passages: usize = open.iter().map(|o| (o & 1) as usize + ((o >> 1) & 1) as usize).sum();
            assert_eq!(passages, 9 * 7 - 1, "a spanning tree");
        }
        let (i, ctx) = blank(80, 60);
        let a = maze(&i, i.rect, &ctx, 10.0, 2.0, 5, (BLACK, WHITE));
        assert_eq!(a, maze(&i, i.rect, &ctx, 10.0, 2.0, 5, (BLACK, WHITE)));
        assert_ne!(a, maze(&i, i.rect, &ctx, 10.0, 2.0, 6, (BLACK, WHITE)));
        let walls = a.chunks(4).filter(|p| p[0] < 0.5).count();
        assert!(walls > 800 && walls < 3500, "{walls}");
        assert!(a[..80 * 4].chunks(4).all(|p| p[0] < 0.5), "closed top edge");
    }

    #[test]
    fn sine_waves_repeat_at_their_wavelength() {
        let (i, ctx) = blank(64, 8);
        for shape in [WaveShape::Linear, WaveShape::Radial, WaveShape::Cross] {
            let o = sine_waves(&i, i.rect, &ctx, shape, 16.0, 0.0, 0.0, 100.0, (WHITE, BLACK));
            assert!(o.chunks(4).any(|p| p[0] > 0.9) && o.chunks(4).any(|p| p[0] < 0.1), "{shape:?}");
        }
        let o = sine_waves(&i, i.rect, &ctx, WaveShape::Linear, 16.0, 0.0, 0.0, 100.0, (WHITE, BLACK));
        assert!((o[3 * 4] - o[19 * 4]).abs() < 1e-4, "period 16");
        let flat = sine_waves(&i, i.rect, &ctx, WaveShape::Linear, 16.0, 0.0, 0.0, 0.0, (WHITE, BLACK));
        assert!(flat.chunks(4).all(|p| (p[0] - 0.5).abs() < 1e-6));
    }
}
