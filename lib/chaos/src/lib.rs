// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Fault and delay injection, keyed by call site.
//!
//! ```ignore
//! if chaos::decide!() { return Err(AllocError); }
//! chaos::delay!();
//! free_slots.shuffle(chaos::site!());
//! chaos::assert_stable!(|| self.refs());
//! ```
//!
//! A seed arms a fixed subset of sites for a whole run, and an armed site fires
//! on a fraction of hits: most of a run stays healthy and reaches deep paths
//! while the armed sites stay hot. Without the `chaos` feature every macro
//! folds to a constant and no symbol is referenced.
//!
//! As in `critical-section`, this crate declares the operations and a binary
//! supplies them via [`set_impl!`]; only primitives cross the boundary, so an
//! implementation may use per-CPU storage, a global, or a fuzzer tape.
//! [`ControlPlane`] is a ready-made one to delegate to. Using a macro with
//! `chaos` on and no [`set_impl!`] anywhere is a link error.

#![cfg_attr(not(any(test, feature = "__test-impl")), no_std)]

mod default_impl;

use cfg_if::cfg_if;

pub use self::default_impl::ControlPlane;

/// An opaque call-site identifier.
///
/// One source location yields one value within a build. It is not stable across
/// builds and carries no ordering.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Callsite(u64);

impl Callsite {
    #[doc(hidden)]
    #[must_use]
    pub const fn __at(line: u32, col: u32, file_len: usize) -> Self {
        Self(((line as u64) << 32) ^ ((col as u64) << 16) ^ file_len as u64)
    }

    /// The identifier as an integer, for implementations that key on it.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// The operations a chaos implementation supplies. Install one with
/// [`set_impl!`].
///
/// # Safety
///
/// Implementations must tolerate any context, including re-entrancy from a trap
/// handler, and must not unwind.
pub unsafe trait Impl {
    /// Should this site fire?
    fn decide(site: Callsite) -> bool;
    /// Spin for a brief amount.
    fn delay(site: Callsite);
    /// Generate a random number for this callsite.
    fn random(site: Callsite) -> u64;
}

/// Install the chaos implementation for this program.
///
/// Exactly one crate in the final binary may call this: two produce a duplicate
/// symbol, none an undefined reference.
#[macro_export]
macro_rules! set_impl {
    ($t:ty) => {
        #[unsafe(no_mangle)]
        extern "C" fn __chaos_decide(site: $crate::Callsite) -> bool {
            <$t as $crate::Impl>::decide(site)
        }
        #[unsafe(no_mangle)]
        extern "C" fn __chaos_delay(site: $crate::Callsite) {
            <$t as $crate::Impl>::delay(site);
        }
        #[unsafe(no_mangle)]
        extern "C" fn __chaos_random(site: $crate::Callsite) -> u64 {
            <$t as $crate::Impl>::random(site)
        }
    };
}

/// The identity of this call site. All inputs are constants, so it folds.
#[macro_export]
macro_rules! site {
    () => {
        $crate::Callsite::__at(line!(), column!(), file!().len())
    };
}

/// Reorders a slice in place.
pub trait SliceExt {
    /// Randomly permutes this slice.
    fn shuffle(&mut self, site: Callsite);
}

/// Reorders an iterator without allocating.
pub trait IteratorExt: Iterator + Sized {
    /// The reordered iterator. Literally `Self` when chaos is disabled, so the
    /// adapter is not merely optimised away — it does not exist.
    type Shuffled: Iterator<Item = Self::Item>;

    /// Reorders this iterator within a fixed window.
    fn shuffled(self, site: Callsite) -> Self::Shuffled;
}

cfg_if! {
    if #[cfg(feature = "chaos")] {
        mod chaos;
        #[doc(hidden)]
        pub use self::chaos::{Shuffled, __assert_stable, __decide, __delay, __random, sym};
    } else {
        mod no_chaos;
        #[doc(hidden)]
        pub use self::no_chaos::{__assert_stable, __decide, __delay, __random};
    }
}

/// `true` if this site should take the unusual path.
#[macro_export]
macro_rules! decide {
    () => {
        $crate::__decide($crate::site!())
    };
}

/// Busy waits for a brief, random amount.
#[macro_export]
macro_rules! delay {
    () => {
        $crate::__delay($crate::site!())
    };
}

/// Asserts the closure returns the same value twice in a row.
///
/// With chaos enabled this will spin for an undetermined amount of time between
/// calls. This can help to catch race conditions.
///
/// # Panics
///
/// Panics if the function does not return the same value twice.
#[macro_export]
macro_rules! assert_stable {
    ($f:expr) => {
        $crate::__assert_stable($crate::site!(), $f)
    };
}

#[cfg(any(test, feature = "__test-impl"))]
mod tests;
