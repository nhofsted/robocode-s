// SPDX-License-Identifier: EPL-2.0

//! Bit-exact port of `java.util.Random` and `java.util.Collections.shuffle`.
//!
//! Classic Robocode uses `java.util.Random` (seeded through `RANDOMSEED`) for
//! initial robot placement and for the order in which robots are processed each
//! turn, so the engine must reproduce it exactly (design §4.1). Tested against
//! vectors produced by a real JDK (`tests/data/java_random.txt`).
//!
//! `nextGaussian` is not ported yet: it needs `StrictMath.log` from the fdlibm port.

const MULTIPLIER: i64 = 0x5_DEEC_E66D;
const ADDEND: i64 = 0xB;
const MASK: i64 = (1 << 48) - 1;
const FLOAT_UNIT: f32 = 1.0 / (1u32 << 24) as f32;
const DOUBLE_UNIT: f64 = 1.0 / (1u64 << 53) as f64;

/// The 48-bit linear congruential generator of `java.util.Random`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRandom {
    seed: i64,
}

impl JavaRandom {
    /// `new Random(seed)`.
    pub fn new(seed: i64) -> Self {
        Self {
            seed: scramble(seed),
        }
    }

    /// `setSeed(seed)`.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = scramble(seed);
    }

    /// `next(bits)`: the next `bits` (1..=32) pseudorandom bits, as Java's `int`.
    pub fn next(&mut self, bits: u32) -> i32 {
        debug_assert!((1..=32).contains(&bits));
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        // The seed is non-negative, so the arithmetic shift equals Java's `>>>`.
        (self.seed >> (48 - bits)) as i32
    }

    /// `nextInt()`.
    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// `nextInt(bound)`.
    ///
    /// # Panics
    ///
    /// If `bound` is not positive, where Java throws `IllegalArgumentException`.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        let mut r = self.next(31);
        let m = bound - 1;
        if bound & m == 0 {
            // Power of two.
            ((i64::from(bound) * i64::from(r)) >> 31) as i32
        } else {
            // Reject values from the incomplete last interval. Java relies on int overflow
            // making `u - r + m` negative.
            let mut u = r;
            loop {
                r = u % bound;
                if u.wrapping_sub(r).wrapping_add(m) >= 0 {
                    return r;
                }
                u = self.next(31);
            }
        }
    }

    /// `nextLong()`.
    pub fn next_long(&mut self) -> i64 {
        let high = i64::from(self.next(32)) << 32;
        high.wrapping_add(i64::from(self.next(32)))
    }

    /// `nextBoolean()`.
    pub fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    /// `nextFloat()`.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * FLOAT_UNIT
    }

    /// `nextDouble()`.
    pub fn next_double(&mut self) -> f64 {
        let high = i64::from(self.next(26)) << 27;
        (high + i64::from(self.next(27))) as f64 * DOUBLE_UNIT
    }

    /// `nextBytes(bytes)`.
    pub fn next_bytes(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(4) {
            let mut rnd = self.next_int();
            for byte in chunk {
                *byte = rnd as u8;
                rnd >>= 8;
            }
        }
    }
}

fn scramble(seed: i64) -> i64 {
    (seed ^ MULTIPLIER) & MASK
}

/// `Collections.shuffle(list, random)`.
pub fn shuffle<T>(list: &mut [T], random: &mut JavaRandom) {
    let size = i32::try_from(list.len()).expect("Java lists hold at most i32::MAX elements");
    for i in (2..=size).rev() {
        let j = random.next_int_bounded(i);
        list.swap((i - 1) as usize, j as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_values() {
        // new Random(42).nextInt() is a widely quoted Java value.
        assert_eq!(JavaRandom::new(42).next_int(), -1_170_105_035);
        assert_eq!(JavaRandom::new(0).next_int(), -1_155_484_576);
    }

    #[test]
    #[should_panic(expected = "bound must be positive")]
    fn non_positive_bound_panics() {
        JavaRandom::new(1).next_int_bounded(0);
    }

    #[test]
    fn set_seed_resets() {
        let mut a = JavaRandom::new(7);
        a.next_long();
        a.set_seed(7);
        assert_eq!(a, JavaRandom::new(7));
    }
}
