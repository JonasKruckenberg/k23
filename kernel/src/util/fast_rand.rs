// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

/// A seed for random number generation.
///
/// In order to make certain functions within a runtime deterministic, a seed
/// can be specified at the time of creation.
#[derive(Clone, Debug)]
pub struct RngSeed {
    s: u32,
    r: u32,
}

/// Fast random number generate.
///
/// Implement `xorshift64+`: 2 32-bit `xorshift` sequences added together.
/// Shift triplet `[17,7,16]` was calculated as indicated in Marsaglia's
/// `Xorshift` paper: <https://www.jstatsoft.org/article/view/v008i14/xorshift.pdf>
/// This generator passes the SmallCrush suite, part of TestU01 framework:
/// <http://simul.iro.umontreal.ca/testu01/tu01.html>
#[derive(Clone, Copy, Debug)]
pub(crate) struct FastRand {
    one: u32,
    two: u32,
}

impl RngSeed {
    /// Creates a random seed using loom internally.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "we're truncating on purpose"
    )]
    pub(crate) fn new(seed: u64) -> Self {
        let one = (seed >> 32_u32) as u32;
        let mut two = seed as u32;

        if two == 0 {
            // This value cannot be zero
            two = 1;
        }

        Self::from_pair(one, two)
    }

    fn from_pair(s: u32, r: u32) -> Self {
        Self { s, r }
    }
}

impl FastRand {
    /// Initialize a new fast random number generator using the default source of entropy.
    pub(crate) fn new(seed: u64) -> FastRand {
        FastRand::from_seed(RngSeed::new(seed))
    }

    /// Initializes a new, thread-local, fast random number generator.
    pub(crate) fn from_seed(seed: RngSeed) -> FastRand {
        FastRand {
            one: seed.s,
            two: seed.r,
        }
    }

    pub(crate) fn fastrand_n(&mut self, n: u32) -> u32 {
        // This is similar to fastrand() % n, but faster.
        // See https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
        let mul = u64::from(self.fastrand()).wrapping_mul(u64::from(n));
        (mul >> 32) as u32
    }

    fn fastrand(&mut self) -> u32 {
        let mut s1 = self.one;
        let s0 = self.two;

        s1 ^= s1 << 17_u32;
        s1 = s1 ^ s0 ^ s1 >> 7_u32 ^ s0 >> 16_u32;

        self.one = s0;
        self.two = s1;

        s0.wrapping_add(s1)
    }
}
