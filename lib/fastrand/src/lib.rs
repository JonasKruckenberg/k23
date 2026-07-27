// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Very small, fast, non-cryptographic random number generators.

#![cfg_attr(not(test), no_std)]

use core::convert::Infallible;

use rand::{Rng, SeedableRng, TryRng};

/// Fast, non-cryptographic random number generator.
///
/// Implement `xorshift64+`: 2 32-bit `xorshift` sequences added together.
/// Shift triplet `[17,7,16]` was calculated as indicated in Marsaglia's
/// `Xorshift` paper: <https://www.jstatsoft.org/article/view/v008i14/xorshift.pdf>
/// This generator passes the SmallCrush suite, part of TestU01 framework:
/// <http://simul.iro.umontreal.ca/testu01/tu01.html>
#[derive(Clone, Copy, Debug)]
pub struct Xorshift64 {
    one: u32,
    two: u32,
}

impl Xorshift64 {
    /// Initialize from a scalar seed.
    ///
    /// Mixed through [`SplitMix64`] rather than sliced in half: nearby seeds
    /// would otherwise produce correlated streams. Use [`Self::from_state`]
    /// when the caller already holds entropy.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "splitting the halves is the point"
    )]
    pub fn from_seed(seed: u64) -> Self {
        let state = SplitMix64::from_seed(seed).next_u64();
        // 1-in-2^64, but the all-zero state would make the generator a constant.
        let state = if state == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            state
        };
        Self::from_state([(state >> 32) as u32, state as u32])
    }

    /// Initialize from raw state.
    ///
    /// # Panics
    ///
    /// Panics in debug builds on all-zero state, which is absorbing: every
    /// subsequent output would be `0`.
    #[must_use]
    pub fn from_state([one, two]: [u32; 2]) -> Self {
        debug_assert!(
            (one | two) != 0,
            "all-zero xorshift state only produces zeroes"
        );
        Self { one, two }
    }

    /// Generate a random `u32` in `0..n`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the high half is the result"
    )]
    pub fn next_range(&mut self, n: u32) -> u32 {
        // <https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/>
        let mul = u64::from(self.next_u32()).wrapping_mul(u64::from(n));
        (mul >> 32) as u32
    }

    /// Generate a random `u32` number.
    pub fn next_u32(&mut self) -> u32 {
        let mut s1 = self.one;
        let s0 = self.two;

        s1 ^= s1 << 17_u32;
        s1 = s1 ^ s0 ^ s1 >> 7_u32 ^ s0 >> 16_u32;

        self.one = s0;
        self.two = s1;

        s0.wrapping_add(s1)
    }
}

/// A fixed-increment counter run through a bit-mixing finalizer.
///
/// Visits every 64-bit state once per period, so unlike [`Xorshift64`] it has
/// no absorbing state and every seed is valid — hence its use as a seeder.
/// Reference: <https://prng.di.unimi.it/splitmix64.c>
#[derive(Clone, Copy, Debug)]
pub struct SplitMix64 {
    x: u64,
}

impl SplitMix64 {
    /// Initialize from a seed. Every `u64` is valid.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        Self { x: seed }
    }

    /// Generate a random `u64` in `0..n`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the high half is the result"
    )]
    pub fn next_range(&mut self, n: u64) -> u64 {
        let mul = u128::from(self.next_u64()).wrapping_mul(u128::from(n));
        (mul >> 64) as u64
    }

    /// Generate a random `u64` number.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        Rng::next_u64(self)
    }
}

impl TryRng for SplitMix64 {
    type Error = Infallible;

    #[expect(
        clippy::cast_possible_truncation,
        reason = "any 32 bits of a uniform u64 are uniform"
    )]
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok((self.try_next_u64()? >> 32) as u32)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        self.x = self.x.wrapping_add(0x9E37_79B9_7F4A_7C15);

        let mut z = self.x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Ok(z ^ (z >> 31))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        for chunk in dst.chunks_mut(8) {
            let bytes = self.try_next_u64()?.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        Ok(())
    }
}

impl SeedableRng for SplitMix64 {
    type Seed = [u8; 8];

    fn from_seed(seed: Self::Seed) -> Self {
        Self {
            x: u64::from_le_bytes(seed),
        }
    }

    /// The default expands the seed into `Seed` bytes via two `pcg32` rounds and
    /// re-parses them; this generator's state is already a `u64`.
    fn seed_from_u64(state: u64) -> Self {
        Self { x: state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference C implementation, seeded with 0.
    #[test]
    fn splitmix64_matches_reference_vector() {
        let mut rng = SplitMix64::from_seed(0);
        let got: [u64; 5] = core::array::from_fn(|_| rng.next_u64());
        assert_eq!(
            got,
            [
                0xe220_a839_7b1d_cdaf,
                0x6e78_9e6a_a1b9_65f4,
                0x06c4_5d18_8009_454f,
                0xf88b_b8a8_724c_81ec,
                0x1b39_896a_51a8_749b,
            ]
        );
    }

    /// The wrong half of the product still looks random; only the bound catches
    /// it.
    #[test]
    fn next_range_is_bounded() {
        let mut xs = Xorshift64::from_seed(1);
        let mut sm = SplitMix64::from_seed(1);
        for n in [1_u32, 2, 3, 7, 10, 64, 1000, u32::MAX] {
            for _ in 0..1000 {
                assert!(xs.next_range(n) < n, "Xorshift64 {n}");
                assert!(sm.next_range(u64::from(n)) < u64::from(n), "SplitMix64 {n}");
            }
        }
    }

    /// Slicing a seed in half zeroes the high word below 2^32 and collides 0
    /// with 1.
    #[test]
    fn nearby_scalar_seeds_are_uncorrelated() {
        let stream = |seed| {
            let mut rng = Xorshift64::from_seed(seed);
            core::array::from_fn::<u32, 8, _>(|_| rng.next_u32())
        };
        for a in 0..64_u64 {
            for b in (a + 1)..64 {
                assert_ne!(stream(a), stream(b), "seeds {a} and {b} share a stream");
            }
        }
    }

    #[test]
    fn scalar_seeds_never_reach_the_absorbing_state() {
        for seed in 0..1000_u64 {
            let mut rng = Xorshift64::from_seed(seed);
            assert!(
                (0..16).any(|_| rng.next_u32() != 0),
                "seed {seed} only produced zeroes"
            );
        }
    }

    #[test]
    #[should_panic(expected = "all-zero xorshift state")]
    fn all_zero_state_is_rejected() {
        let _ = Xorshift64::from_state([0, 0]);
    }
}
