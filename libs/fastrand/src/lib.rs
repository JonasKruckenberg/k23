// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Very small, fast, non-cryptographic random number generator.
//!
//! This random number generator implements the `xorshift64+` algorithm, generating random numbers
//! adding two 32-bit `xorshift` sequences together.

#![cfg_attr(not(test), no_std)]

/// Fast, non-cryptographic random number generator.
///
/// Implement `xorshift64+`: 2 32-bit `xorshift` sequences added together.
/// Shift triplet `[17,7,16]` was calculated as indicated in Marsaglia's
/// `Xorshift` paper: <https://www.jstatsoft.org/article/view/v008i14/xorshift.pdf>
/// This generator passes the SmallCrush suite, part of TestU01 framework:
/// <http://simul.iro.umontreal.ca/testu01/tu01.html>
#[derive(Clone, Copy, Debug)]
pub struct FastRand {
    one: u32,
    two: u32,
}

impl FastRand {
    /// Initializes a new, thread-local, fast random number generator from the provided seed.
    pub fn from_seed(seed: u64) -> FastRand {
        let one = (seed >> 32) as u32;
        let mut two = seed as u32;

        if two == 0 {
            // This value cannot be zero
            two = 1;
        }

        FastRand { one, two }
    }

    /// Generate a random `u32` between `0` and `n`.
    pub fn fastrand_n(&mut self, n: u32) -> u32 {
        // This is similar to fastrand() % n, but faster.
        // See https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
        let mul = u64::from(self.fastrand()).wrapping_mul(u64::from(n));
        (mul >> 32) as u32
    }

    /// Generate a random `u32` number.
    pub fn fastrand(&mut self) -> u32 {
        let mut s1 = self.one;
        let s0 = self.two;

        s1 ^= s1 << 17_u32;
        s1 = s1 ^ s0 ^ s1 >> 7_u32 ^ s0 >> 16_u32;

        self.one = s0;
        self.two = s1;

        s0.wrapping_add(s1)
    }
}
