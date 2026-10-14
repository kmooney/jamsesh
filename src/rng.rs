//! Tiny xorshift RNG; musical randomness doesn't need more.

use std::time::{SystemTime, UNIX_EPOCH};

pub struct Rng(u64);

impl Rng {
    pub fn from_time() -> Self {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng(t | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.f64() * n as f64) as usize % n.max(1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    /// Sample an index from weights sharpened/flattened by `temperature`
    /// (0 = always the most likely, 1 = sample the distribution as given).
    pub fn weighted(&mut self, weights: &[f64], temperature: f64) -> usize {
        if weights.is_empty() {
            return 0;
        }
        if temperature <= 0.01 {
            return weights
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
        let w: Vec<f64> = weights.iter().map(|p| p.max(1e-9).powf(1.0 / temperature)).collect();
        let total: f64 = w.iter().sum();
        let mut r = self.f64() * total;
        for (i, x) in w.iter().enumerate() {
            r -= x;
            if r <= 0.0 {
                return i;
            }
        }
        // Rounding left a sliver: take the last option that had any weight, never a zero one.
        weights.iter().rposition(|&p| p > 0.0).unwrap_or(w.len() - 1)
    }
}
