// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use fastrand::SplitMix64;
use rand::SeedableRng;

use crate::Callsite;

/// A seed arms `1/ODDS` of sites, and an armed site fires on `1/ODDS` of hits.
const ODDS: u64 = 4;
const THRESHOLD: u64 = u64::MAX / ODDS;

#[derive(Clone, Debug)]
pub struct ControlPlane {
    seed: u64,
    rng: SplitMix64,
    fuel: u32,
}

impl ControlPlane {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            rng: SplitMix64::seed_from_u64(seed),
            fuel: u32::MAX,
        }
    }

    /// Cap the number of decisions.
    ///
    /// Every decision `fuel` is decremented. Once it hits zero,
    /// all method of this struct behave is if chaos mode was disabled.
    pub fn set_fuel(&mut self, fuel: u32) {
        self.fuel = fuel;
    }

    /// Returns `true` if `site` is active.
    pub fn decide_at(&mut self, site: Callsite) -> bool {
        if self.fuel == 0 || !armed(self.seed, site) {
            return false;
        }
        if self.rng.next_u64() >= THRESHOLD {
            return false;
        }
        self.fuel -= 1;
        true
    }

    /// Spin for a brief, random amount if `site` is active.
    pub fn delay_at(&mut self, site: Callsite) {
        if !self.decide_at(site) {
            return;
        }
        for _ in 0..self.rng.next_u64() & 63 {
            core::hint::spin_loop();
        }
    }

    pub fn random_at(&mut self, _site: Callsite) -> u64 {
        self.rng.next_u64()
    }
}

/// Whether `seed` arms `site`. Pure, so a run needs no per-site storage.
pub(crate) fn armed(seed: u64, site: Callsite) -> bool {
    SplitMix64::seed_from_u64(seed ^ site.as_u64()).next_u64() < THRESHOLD
}
