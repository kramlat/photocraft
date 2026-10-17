//! Cage Transform: deform an image by dragging the corners of a closed polygon drawn around it.
//!
//! A point inside the cage is written as a weighted sum of the cage's corners (mean value
//! coordinates, Floater 2003); moving the corners then moves the point by the same weights, so
//! the image follows the cage smoothly. The weights are exact for any simple polygon, convex or
//! not. Points outside the cage use the same signed formula, which keeps the map continuous
//! across the cage edge.

/// A cage: the polygon before (`src`) and after (`dst`) the user dragged its corners.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cage {
    pub src: Vec<[f64; 2]>,
    pub dst: Vec<[f64; 2]>,
}

impl Cage {
    /// At least three corners, matching counts, finite coordinates, non-zero area.
    pub fn validate(&self) -> Result<(), String> {
        if self.src.len() < 3 {
            return Err("a cage needs at least 3 corners".into());
        }
        if self.src.len() != self.dst.len() {
            return Err("`src` and `dst` must have the same number of corners".into());
        }
        if self.src.iter().chain(&self.dst).flatten().any(|v| !v.is_finite()) {
            return Err("cage corners must be finite".into());
        }
        if area(&self.src).abs() < 1e-9 {
            return Err("the cage has no area".into());
        }
        Ok(())
    }

    /// True when no corner moved.
    pub fn is_identity(&self) -> bool {
        self.src == self.dst
    }

    /// Where the source point `(x, y)` ends up.
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let w = mean_value_weights(&self.src, [x, y]);
        let (mut ox, mut oy) = (0.0, 0.0);
        for (wi, d) in w.iter().zip(&self.dst) {
            ox += wi * d[0];
            oy += wi * d[1];
        }
        (ox, oy)
    }
}

fn area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f64>() / 2.0
}

/// Mean value coordinates of `p` with respect to the polygon `v`; they sum to 1.
pub fn mean_value_weights(v: &[[f64; 2]], p: [f64; 2]) -> Vec<f64> {
    let n = v.len();
    let mut w = vec![0.0; n];
    let d: Vec<[f64; 2]> = v.iter().map(|c| [c[0] - p[0], c[1] - p[1]]).collect();
    let r: Vec<f64> = d.iter().map(|c| c[0].hypot(c[1])).collect();
    const EPS: f64 = 1e-9;
    // On a corner: that corner takes it all.
    if let Some(i) = r.iter().position(|&r| r < EPS) {
        w[i] = 1.0;
        return w;
    }
    // tan(half the signed angle at `p` between corner i and i+1).
    let mut t = vec![0.0; n];
    for i in 0..n {
        let j = (i + 1) % n;
        let cross = d[i][0] * d[j][1] - d[i][1] * d[j][0];
        let dot = d[i][0] * d[j][0] + d[i][1] * d[j][1];
        // On an edge: interpolate between its two ends.
        if cross.abs() < EPS * r[i] * r[j] && dot < 0.0 {
            let s = r[i] + r[j];
            w[i] = r[j] / s;
            w[j] = r[i] / s;
            return w;
        }
        t[i] = cross / (r[i] * r[j] + dot);
    }
    let mut sum = 0.0;
    for i in 0..n {
        let prev = (i + n - 1) % n;
        w[i] = (t[prev] + t[i]) / r[i];
        sum += w[i];
    }
    if sum.abs() < EPS {
        // Far outside, where the formula degenerates: fall back to the centroid weights.
        return vec![1.0 / n as f64; n];
    }
    for x in &mut w {
        *x /= sum;
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn weights_sum_to_one_and_reproduce_the_point() {
        let v = square();
        for p in [[5.0, 5.0], [1.0, 9.0], [3.3, 0.2]] {
            let w = mean_value_weights(&v, p);
            assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
            let x: f64 = w.iter().zip(&v).map(|(w, c)| w * c[0]).sum();
            let y: f64 = w.iter().zip(&v).map(|(w, c)| w * c[1]).sum();
            assert!((x - p[0]).abs() < 1e-9 && (y - p[1]).abs() < 1e-9, "{p:?} -> {x},{y}");
        }
    }

    #[test]
    fn corners_and_edges_are_exact() {
        let v = square();
        assert_eq!(mean_value_weights(&v, [10.0, 0.0]), vec![0.0, 1.0, 0.0, 0.0]);
        let w = mean_value_weights(&v, [5.0, 0.0]);
        assert!((w[0] - 0.5).abs() < 1e-9 && (w[1] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn dragging_a_corner_moves_the_middle_by_a_quarter() {
        let c = Cage { src: square(), dst: vec![[0.0, 0.0], [10.0, 0.0], [18.0, 10.0], [0.0, 10.0]] };
        let (x, y) = c.map(5.0, 5.0);
        assert!((x - 7.0).abs() < 1e-9 && (y - 5.0).abs() < 1e-9, "{x},{y}");
    }

    #[test]
    fn concave_cage_is_finite_everywhere() {
        let v = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [5.0, 4.0], [0.0, 10.0]];
        let c = Cage { src: v.clone(), dst: v };
        for y in -5..16 {
            for x in -5..16 {
                let (mx, my) = c.map(f64::from(x), f64::from(y));
                assert!(mx.is_finite() && my.is_finite());
            }
        }
        let (mx, my) = c.map(2.0, 3.0);
        assert!((mx - 2.0).abs() < 1e-9 && (my - 3.0).abs() < 1e-9, "identity cage is identity");
    }

    #[test]
    fn validation() {
        assert!(Cage { src: vec![[0.0, 0.0], [1.0, 0.0]], dst: vec![[0.0, 0.0], [1.0, 0.0]] }.validate().is_err());
        assert!(Cage { src: square(), dst: square()[..3].to_vec() }.validate().is_err());
        assert!(Cage { src: vec![[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]], dst: vec![[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]] }.validate().is_err());
        assert!(Cage { src: square(), dst: square() }.validate().is_ok());
    }
}
