//! Seedable PRNGs. Never seed from time or thread state inside a scene.

/// SplitMix64, for every random choice of a scene except the start points.
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo)
    }

    pub fn log_range(&mut self, lo: f64, hi: f64) -> f64 {
        self.range(lo.ln(), hi.ln()).exp()
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// Uniform in 0..n; n must be positive.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }
}

/// mulberry32 from flow.js, used for start points so a scene's trajectories are the ones
/// the web app draws for the same seed.
#[derive(Clone, Debug)]
pub struct Mulberry32(u32);

impl Mulberry32 {
    pub fn new(seed: u32) -> Self {
        Mulberry32(seed)
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let a = self.0;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_is_deterministic_and_in_range() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..1000 {
            let x = a.next_f64();
            assert_eq!(x, b.next_f64());
            assert!((0.0..1.0).contains(&x));
            assert!(a.below(7) < 7);
            b.below(7);
        }
    }

    #[test]
    fn log_range_stays_in_bounds() {
        let mut r = SplitMix64::new(1);
        for _ in 0..1000 {
            let v = r.log_range(6.0, 30.0);
            assert!((6.0..=30.0).contains(&v));
        }
    }
}
