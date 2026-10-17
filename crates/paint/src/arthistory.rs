//! The Art History Brush's paint: a painterly restyling of a history state, drawn as a field of
//! short strokes that depends only on position, the picture and a few numbers. A stroke's
//! place, colour and bending come from a hash of its grid cell, so any region of the field can be
//! rendered on its own (the live preview paints one dab at a time) and the pieces agree with the
//! whole.
//!
//! Style (Photoshop's names): Tight strokes follow the picture's edges, Loose ones run any way;
//! Short, Medium and Long set the length; Curl bends them; Dab paints round dots. Fidelity is how
//! closely colours and directions follow the source (low values wander); Area is the size of the
//! picture the strokes cover, in pixels, and so how large the strokes are.

use crate::retouch::Region;
use crate::rng::hash2;
use photocraft_geom::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtStyle {
    TightShort,
    TightMedium,
    TightLong,
    LooseMedium,
    LooseLong,
    LooseCurl,
    Dab,
    TightCurl,
    TightCurlLong,
    LooseCurlLong,
}

impl ArtStyle {
    pub const ALL: [(ArtStyle, &'static str); 10] = [
        (ArtStyle::TightShort, "tightShort"),
        (ArtStyle::TightMedium, "tightMedium"),
        (ArtStyle::TightLong, "tightLong"),
        (ArtStyle::LooseMedium, "looseMedium"),
        (ArtStyle::LooseLong, "looseLong"),
        (ArtStyle::LooseCurl, "looseCurl"),
        (ArtStyle::Dab, "dab"),
        (ArtStyle::TightCurl, "tightCurl"),
        (ArtStyle::TightCurlLong, "tightCurlLong"),
        (ArtStyle::LooseCurlLong, "looseCurlLong"),
    ];

    pub fn parse(s: &str) -> Option<ArtStyle> {
        Self::ALL.iter().find(|(_, n)| *n == s).map(|(v, _)| *v)
    }

    fn tight(self) -> bool {
        matches!(self, Self::TightShort | Self::TightMedium | Self::TightLong | Self::TightCurl | Self::TightCurlLong)
    }

    fn curl(self) -> f32 {
        match self {
            Self::LooseCurl | Self::TightCurl => 1.0,
            Self::TightCurlLong | Self::LooseCurlLong => 1.0,
            _ => 0.0,
        }
    }

    /// Stroke length in stroke widths.
    fn length(self) -> f32 {
        match self {
            Self::Dab => 0.0,
            Self::TightShort => 3.0,
            Self::TightMedium | Self::LooseMedium | Self::LooseCurl | Self::TightCurl => 5.0,
            Self::TightLong | Self::LooseLong | Self::TightCurlLong | Self::LooseCurlLong => 9.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtParams {
    pub style: ArtStyle,
    /// 0..1: how closely colours and directions follow the source.
    pub fidelity: f32,
    /// Pixels: the size of the area each stroke serves.
    pub area: f32,
    pub seed: u32,
}

impl ArtParams {
    fn width(&self) -> f32 {
        let f = if self.style.tight() { 0.06 } else { 0.12 };
        (self.area * f).clamp(1.5, 40.0)
    }
    fn spacing(&self) -> f32 {
        (self.width() * 0.9).max(1.5)
    }
    /// How far a stroke reaches from its cell: the region read for a render must extend this far
    /// beyond the area painted.
    pub fn margin(&self) -> i32 {
        let w = self.width();
        (w * self.style.length() + w + self.spacing() * 2.0).ceil() as i32 + 2
    }
}

struct Stroke {
    key: f32,
    pts: Vec<[f32; 2]>,
    color: Vec<f32>,
    width: f32,
}

fn luma(px: &[f32]) -> f32 {
    if px.len() >= 3 { 0.299 * px[0] + 0.587 * px[1] + 0.114 * px[2] } else { px[0] }
}

fn sample(src: &Region, bounds: Rect, x: f32, y: f32) -> Option<&[f32]> {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    (bounds.contains(xi, yi) && src.rect.contains(xi, yi)).then(|| src.px(xi, yi))
}

/// The painterly picture for `r`: `src` is the source state read over `r` grown by
/// [`ArtParams::margin`] (anything outside `bounds`, the document, carries no stroke).
pub fn art_paint(src: &Region, bounds: Rect, r: Rect, p: &ArtParams) -> Region {
    let mut out = Region::new(r, src.ch);
    // The base is the source itself, so no pixel is left without paint.
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            if src.rect.contains(x, y) {
                let i = out.index(x, y) * src.ch;
                out.data[i..i + src.ch].copy_from_slice(src.px(x, y));
            }
        }
    }
    let (w, s, ch) = (p.width(), p.spacing(), src.ch);
    let m = p.margin() as f32;
    let colour_ch = if ch == 2 || ch == 4 { ch - 1 } else { ch };
    let seed = u64::from(p.seed);
    let (cx0, cx1) = (((r.x0 as f32 - m) / s).floor() as i32, ((r.x1 as f32 + m) / s).ceil() as i32);
    let (cy0, cy1) = (((r.y0 as f32 - m) / s).floor() as i32, ((r.y1 as f32 + m) / s).ceil() as i32);
    let mut strokes: Vec<Stroke> = Vec::new();
    for j in cy0..=cy1 {
        for i in cx0..=cx1 {
            let h = |salt: u64| hash2(i, j, seed.wrapping_mul(0x9e37).wrapping_add(salt));
            let c = [(i as f32 + 0.5 + (h(1) - 0.5)) * s, (j as f32 + 0.5 + (h(2) - 0.5)) * s];
            let Some(px) = sample(src, bounds, c[0], c[1]) else { continue };
            // Direction: along the edge for Tight strokes (perpendicular to the gradient), any way
            // for Loose ones; low fidelity adds a wobble.
            let mut angle = h(3) * std::f32::consts::PI;
            if p.style.tight() {
                let g = |dx: f32, dy: f32| sample(src, bounds, c[0] + dx, c[1] + dy).map_or(luma(px), luma);
                let (gx, gy) = (g(1.5, 0.0) - g(-1.5, 0.0), g(0.0, 1.5) - g(0.0, -1.5));
                if gx.hypot(gy) > 0.02 {
                    angle = gy.atan2(gx) + std::f32::consts::FRAC_PI_2 + (h(4) - 0.5) * (1.0 - p.fidelity) * 2.0;
                }
            }
            let mut color = px.to_vec();
            for (k, v) in color.iter_mut().enumerate().take(colour_ch) {
                *v = (*v + (hash2(i, j, seed.wrapping_add(10 + k as u64)) - 0.5) * (1.0 - p.fidelity) * 0.6).clamp(0.0, 1.0);
            }
            let len = w * p.style.length() * (0.7 + 0.6 * h(5));
            let mut pts = Vec::new();
            if len <= 0.0 {
                pts.push(c);
            } else {
                // A polyline of 6 pieces that bends by up to ~1.3 rad over its length when curling.
                let bend = p.style.curl() * (h(6) - 0.5) * 2.6 / 6.0;
                let (mut a, mut q) = (angle, [c[0] - angle.cos() * len / 2.0, c[1] - angle.sin() * len / 2.0]);
                pts.push(q);
                for _ in 0..6 {
                    a += bend;
                    q = [q[0] + a.cos() * len / 6.0, q[1] + a.sin() * len / 6.0];
                    pts.push(q);
                }
            }
            strokes.push(Stroke { key: h(7), pts, color, width: w });
        }
    }
    strokes.sort_by(|a, b| a.key.total_cmp(&b.key));
    for st in &strokes {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for q in &st.pts {
            x0 = x0.min(q[0]);
            y0 = y0.min(q[1]);
            x1 = x1.max(q[0]);
            y1 = y1.max(q[1]);
        }
        let pad = st.width / 2.0 + 1.0;
        let bx = Rect::new((x0 - pad).floor() as i32, (y0 - pad).floor() as i32, (x1 + pad).ceil() as i32 + 1, (y1 + pad).ceil() as i32 + 1).intersect(&r);
        for y in bx.y0..bx.y1 {
            for x in bx.x0..bx.x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = distance(&st.pts, px, py);
                let cov = (st.width / 2.0 - d + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let o = out.px_mut(x, y);
                for k in 0..ch {
                    o[k] += (st.color[k] - o[k]) * cov;
                }
            }
        }
    }
    out
}

fn distance(pts: &[[f32; 2]], x: f32, y: f32) -> f32 {
    if pts.len() == 1 {
        return (pts[0][0] - x).hypot(pts[0][1] - y);
    }
    pts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - a[0]) * dx + (y - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (a[0] + dx * t - x).hypot(a[1] + dy * t - y)
        })
        .fold(f32::MAX, f32::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: i32, h: i32) -> (Region, Rect) {
        let rect = Rect::new(-40, -40, w + 40, h + 40);
        let mut r = Region::new(rect, 3);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                // Left half red, right half blue: one vertical edge down the middle.
                let px = r.px_mut(x, y);
                px.copy_from_slice(if x < w / 2 { &[1.0, 0.0, 0.0] } else { &[0.0, 0.0, 1.0] });
            }
        }
        (r, Rect::new(0, 0, w, h))
    }

    fn params(style: ArtStyle, fidelity: f32) -> ArtParams {
        ArtParams { style, fidelity, area: 40.0, seed: 7 }
    }

    #[test]
    fn a_region_painted_alone_matches_the_same_region_of_the_whole() {
        let (src, bounds) = picture(80, 60);
        for style in [ArtStyle::TightShort, ArtStyle::LooseCurl, ArtStyle::Dab] {
            let p = params(style, 0.5);
            let whole = art_paint(&src, bounds, bounds, &p);
            let part = Rect::new(20, 10, 55, 40);
            let piece = art_paint(&src, bounds, part, &p);
            for y in part.y0..part.y1 {
                for x in part.x0..part.x1 {
                    assert_eq!(whole.px(x, y), piece.px(x, y), "{style:?} at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn full_fidelity_keeps_the_colours_and_low_fidelity_wanders() {
        let (src, bounds) = picture(80, 60);
        let exact = art_paint(&src, bounds, bounds, &params(ArtStyle::LooseMedium, 1.0));
        // Away from the edge every stroke paints the colour it sampled: the source's.
        assert_eq!(exact.px(10, 30), &[1.0, 0.0, 0.0]);
        assert_eq!(exact.px(70, 30), &[0.0, 0.0, 1.0]);
        let loose = art_paint(&src, bounds, bounds, &params(ArtStyle::LooseMedium, 0.0));
        let off: f32 = (0..60).map(|y| (loose.px(10, y)[1] - 0.0).abs() + (loose.px(10, y)[2]).abs()).sum();
        assert!(off > 1.0, "colours stayed on the source: {off}");
    }

    #[test]
    fn tight_strokes_follow_the_edge_and_loose_ones_cross_it() {
        let (src, bounds) = picture(120, 60);
        let along = |style| {
            // How much the red/blue boundary column mixes: strokes crossing it smear colours across.
            let out = art_paint(&src, bounds, bounds, &params(style, 1.0));
            (0..60).map(|y| (out.px(50, y)[0] - 1.0).abs().min(out.px(50, y)[0]) + (out.px(70, y)[2] - 1.0).abs().min(out.px(70, y)[2])).sum::<f32>()
        };
        assert!(along(ArtStyle::TightLong) <= along(ArtStyle::LooseLong));
    }

    #[test]
    fn styles_parse_and_a_tiny_area_still_paints() {
        for (s, n) in ArtStyle::ALL {
            assert_eq!(ArtStyle::parse(n), Some(s));
        }
        assert_eq!(ArtStyle::parse("nope"), None);
        let (src, bounds) = picture(30, 30);
        let out = art_paint(&src, bounds, bounds, &ArtParams { style: ArtStyle::Dab, fidelity: 0.9, area: 1.0, seed: 1 });
        assert_eq!(out.rect, bounds);
    }
}
