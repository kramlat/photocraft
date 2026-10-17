//! The random number generator libmypaint uses: Knuth's lagged Fibonacci generator
//! (TAOCP 3.6, exercise 3.6-15) with the reduced "low quality" constants MyPaint chose, so a
//! stroke with the same seed paints the same dabs as the C library.

const QUALITY: usize = 19;
const TT: i32 = 7;
const KK: usize = 10;
const LL: usize = 7;

fn mod_sum(x: f64, y: f64) -> f64 {
    (x + y) - (x + y).trunc()
}

#[derive(Clone, Debug)]
pub struct Rng {
    ran_u: [f64; KK],
    buf: [f64; QUALITY],
    /// The next unread entry of `buf`; `None` before the first cycle.
    ptr: Option<usize>,
}

impl Rng {
    pub fn new(seed: i64) -> Self {
        let mut r = Rng { ran_u: [0.0; KK], buf: [0.0; QUALITY], ptr: None };
        r.set_seed(seed);
        r
    }

    fn get_array(&mut self, aa: &mut [f64], n: usize) {
        aa[..KK].copy_from_slice(&self.ran_u);
        for j in KK..n {
            aa[j] = mod_sum(aa[j - KK], aa[j - LL]);
        }
        let mut j = n;
        let mut i = 0;
        while i < LL {
            self.ran_u[i] = mod_sum(aa[j - KK], aa[j - LL]);
            i += 1;
            j += 1;
        }
        while i < KK {
            self.ran_u[i] = mod_sum(aa[j - KK], self.ran_u[i - LL]);
            i += 1;
            j += 1;
        }
    }

    pub fn set_seed(&mut self, seed: i64) {
        let mut u = [0.0f64; KK + KK - 1];
        let ulp = (1.0 / (1u64 << 30) as f64) / (1u64 << 22) as f64;
        let mut ss = 2.0 * ulp * (((seed & 0x3fff_ffff) + 2) as f64);
        for v in u.iter_mut().take(KK) {
            *v = ss;
            ss += ss;
            if ss >= 1.0 {
                ss -= 1.0 - 2.0 * ulp;
            }
        }
        u[1] += ulp;
        let mut s = seed & 0x3fff_ffff;
        let mut t = TT - 1;
        while t != 0 {
            for j in (1..KK).rev() {
                u[j + j] = u[j];
                u[j + j - 1] = 0.0;
            }
            for j in (KK..=KK + KK - 2).rev() {
                u[j - (KK - LL)] = mod_sum(u[j - (KK - LL)], u[j]);
                u[j - KK] = mod_sum(u[j - KK], u[j]);
            }
            if s & 1 == 1 {
                for j in (1..=KK).rev() {
                    u[j] = u[j - 1];
                }
                u[0] = u[KK];
                u[LL] = mod_sum(u[LL], u[KK]);
            }
            if s != 0 {
                s >>= 1;
            } else {
                t -= 1;
            }
        }
        self.ran_u[KK - LL..].copy_from_slice(&u[..LL]);
        self.ran_u[..KK - LL].copy_from_slice(&u[LL..KK]);
        let mut warm = [0.0f64; KK + KK - 1];
        for _ in 0..10 {
            self.get_array(&mut warm, KK + KK - 1);
        }
        self.ptr = None;
    }

    fn cycle(&mut self) -> f64 {
        let mut buf = [0.0f64; QUALITY];
        self.get_array(&mut buf, QUALITY);
        buf[KK] = -1.0;
        self.buf = buf;
        self.ptr = Some(1);
        self.buf[0]
    }

    /// Uniform in [0, 1).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        if let Some(p) = self.ptr
            && self.buf[p] >= 0.0
        {
            self.ptr = Some(p + 1);
            return self.buf[p];
        }
        self.cycle()
    }

    /// Roughly normal with standard deviation 1 (the sum of four uniforms).
    pub fn gauss(&mut self) -> f32 {
        let sum = self.next() + self.next() + self.next() + self.next();
        (sum * 1.73205080757 - 3.46410161514) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first draws of `rng_double_new(1000)` in libmypaint: ours are the same numbers, across
    /// more than one refill of the buffer.
    #[test]
    fn matches_the_c_generator() {
        let mut r = Rng::new(1000);
        let got: Vec<f64> = (0..45).map(|_| r.next()).collect();
        let want = [0.9370603463286893, 0.9654422751419376, 0.876434444690962, 0.5065075899005305, 0.15187652227073833, 0.16705344436999092, 0.842150123488026];
        for (k, i) in [0usize, 1, 2, 10, 11, 43, 44].into_iter().enumerate() {
            assert!((got[i] - want[k]).abs() < 1e-15, "{i}: {} vs {}", got[i], want[k]);
        }
    }

    #[test]
    fn same_seed_same_numbers_and_all_in_range() {
        let (mut a, mut b) = (Rng::new(1000), Rng::new(1000));
        for _ in 0..500 {
            let v = a.next();
            assert_eq!(v, b.next());
            assert!((0.0..1.0).contains(&v));
        }
        assert_ne!(Rng::new(1).next(), Rng::new(2).next());
        let g: Vec<f32> = (0..2000).map(|_| a.gauss()).collect();
        let mean = g.iter().sum::<f32>() / g.len() as f32;
        assert!(mean.abs() < 0.15, "{mean}");
    }
}
