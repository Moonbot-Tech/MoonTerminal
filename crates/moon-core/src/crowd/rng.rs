//! The dice.
//!
//! Seeded and written out by hand so a replay is bit-for-bit repeatable across machines: the
//! synthetic feed needs a market to invent and the SockJS session needs an id, and neither is
//! worth a dependency.

/// Seeded xorshift64*.
///
/// Small on purpose: what is asked of it is a believable market and a session id, not statistical
/// quality. `db::tuner` carries its own copy of the same generator for its threshold search; the
/// two are deliberately not shared, because that one sits in a numeric hot loop whose shape is
/// tuned for it.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator; zero is remapped, since xorshift is stuck at zero forever.
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[low, high)`; a reversed or empty range returns `low`.
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        if high <= low {
            return low;
        }
        low + self.next_f32() * (high - low)
    }
}

#[cfg(test)]
mod tests;
