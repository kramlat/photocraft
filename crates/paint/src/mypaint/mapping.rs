//! A setting's value from the brush inputs: a base value plus, for every input that has a
//! curve, the value of that piecewise-linear curve at the input.

use super::tables::N_INPUTS;

#[derive(Clone, Debug, Default)]
pub struct Mapping {
    pub base: f32,
    /// One curve per input (`x`, `y` points); empty = the input does not affect the setting.
    curves: Vec<Vec<(f32, f32)>>,
    used: usize,
}

impl Mapping {
    pub fn new() -> Self {
        Mapping { base: 0.0, curves: vec![Vec::new(); N_INPUTS], used: 0 }
    }

    pub fn is_constant(&self) -> bool {
        self.used == 0
    }

    pub fn set_curve(&mut self, input: usize, points: Vec<(f32, f32)>) {
        let had = !self.curves[input].is_empty();
        let has = points.len() >= 2;
        // One point cannot make a line; libmypaint refuses such a curve, so it is ignored.
        self.curves[input] = if has { points } else { Vec::new() };
        match (had, has) {
            (false, true) => self.used += 1,
            (true, false) => self.used -= 1,
            _ => {}
        }
    }

    pub fn curve(&self, input: usize) -> &[(f32, f32)] {
        &self.curves[input]
    }

    pub fn calculate(&self, data: &[f32; N_INPUTS]) -> f32 {
        let mut result = self.base;
        if self.used == 0 {
            return result;
        }
        for (j, p) in self.curves.iter().enumerate() {
            if p.is_empty() {
                continue;
            }
            let x = data[j];
            let (mut x0, mut y0, mut x1, mut y1) = (p[0].0, p[0].1, p[1].0, p[1].1);
            let mut i = 2;
            while i < p.len() && x > x1 {
                x0 = x1;
                y0 = y1;
                x1 = p[i].0;
                y1 = p[i].1;
                i += 1;
            }
            let y = if x0 == x1 || y0 == y1 { y0 } else { (y1 * (x - x0) + y0 * (x1 - x)) / (x1 - x0) };
            result += y;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curve_is_followed_and_its_end_segments_continue() {
        let mut m = Mapping::new();
        m.base = 1.0;
        assert!(m.is_constant());
        m.set_curve(0, vec![(0.0, 0.0), (0.5, 1.0), (1.0, 1.0)]);
        let mut inputs = [0.0f32; N_INPUTS];
        assert_eq!(m.calculate(&inputs), 1.0);
        inputs[0] = 0.25;
        assert!((m.calculate(&inputs) - 1.5).abs() < 1e-6);
        inputs[0] = 5.0;
        assert_eq!(m.calculate(&inputs), 2.0);
        m.set_curve(0, vec![(0.0, 0.0)]);
        assert!(m.is_constant());
    }
}
