// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
//! Kernel counters
//!
//! Kernel counters are per-hart, unsigned integer counters that facilitate diagnostics across the
//! whole kernel. Questions like "how many times has X happened over N seconds?", "has X ever happened?"
//! can be answered using this API.
//!
//! Counters are declared in their respective modules like so:
//! ```rust
//! use crate::metrics::{Counter, counter};
//!
//! static TEST_CNT: Counter = counter!("test-event");
//!
//! fn some_function() {
//!     TEST_CNT.increment(1);
//! }
//! ```
//!
//! Kernel counters are always per-hart, which means each hart keeps an individual counter. Methods
//! on `Counter` can be used to sum events across harts or even get the maximum or minimum value across
//! harts.

use crate::hart_local::HartLocal;
use core::sync::atomic::{AtomicU64, Ordering};

/// Declares a new counter.
#[macro_export]
macro_rules! counter {
    ($name:expr) => {{
        #[unsafe(link_section = concat!(".bss.kcounter.", $name))]
        static ARENA: $crate::hart_local::HartLocal<::core::sync::atomic::AtomicU64> =
            $crate::hart_local::HartLocal::new();

        Counter::new(&ARENA, $name)
    }};
}

/// A kernel counter.
pub struct Counter {
    arena: &'static HartLocal<AtomicU64>,
    name: &'static str,
}

impl Counter {
    #[doc(hidden)]
    pub const fn new(arena: &'static HartLocal<AtomicU64>, name: &'static str) -> Self {
        Self { arena, name }
    }

    /// Increment the counter.
    pub fn increment(&self, value: u64) {
        self.arena
            .get_or_default()
            .fetch_add(value, Ordering::Relaxed);
    }

    /// Decrement the counter.
    pub fn decrement(&self, value: u64) {
        self.arena
            .get_or_default()
            .fetch_sub(value, Ordering::Relaxed);
    }

    /// Set the absolute value of the counter.
    pub fn set(&self, value: u64) {
        self.arena.get_or_default().store(value, Ordering::Relaxed);
    }

    /// Set the absolute value of the counter if the provided value is larger than the current value.
    pub fn max(&self, value: u64) {
        let _ =
            self.arena
                .get_or_default()
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                    (old < value).then_some(value)
                });
    }

    /// Set the absolute value of the counter if the provided value is smaller than the current value.
    pub fn min(&self, value: u64) {
        let _ =
            self.arena
                .get_or_default()
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                    (old > value).then_some(value)
                });
    }

    /// Get the counter value of the calling hart, or `None` if the counter was never written to.
    pub fn get(&self) -> Option<u64> {
        Some(self.arena.get()?.load(Ordering::Relaxed))
    }

    /// Return the sum of all counters across all harts.
    pub fn sum_across_all_harts(&self) -> u64 {
        self.arena.iter().map(|v| v.load(Ordering::Relaxed)).sum()
    }

    /// Return the largest value from across all harts.
    pub fn max_across_all_harts(&self) -> u64 {
        self.arena
            .iter()
            .map(|v| v.load(Ordering::Relaxed))
            .max()
            .unwrap()
    }

    /// Return the smallest value from across all harts.
    pub fn min_across_all_harts(&self) -> u64 {
        self.arena
            .iter()
            .map(|v| v.load(Ordering::Relaxed))
            .min()
            .unwrap()
    }
}
