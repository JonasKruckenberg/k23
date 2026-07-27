// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! How many CPUs there can be, how they are named, and sets of them.
//!
//! Hardware numbers its CPUs in ways that make poor table indices: a RISC-V
//! hart id is an arbitrary `usize`, neither dense nor bounded, so a machine may
//! expose hart 0 and hart 1000 and nothing in between. The kernel assigns each
//! CPU a dense [`LogicalCpuId`] instead, and converts back to the hardware id
//! only at the hardware boundary.
//!
//! [`MAX_CPUS`] is the build-time bound on those ids. It sizes per-CPU state —
//! including [`CpuSet`], which is one bit per possible CPU. Bear in mind that
//! it is a *bound*, not a CPU count: it is whatever the operator configured and
//! says nothing about how many CPUs booted, so work proportional to it is work
//! proportional to a setting rather than to the hardware.
//!
//! # Not a general-purpose vocabulary
//!
//! This crate lives under `//sys` rather than `//lib` on purpose. Reusable
//! crates take a plain `usize` slot index and their own capacity as a const
//! generic: they are as usable driven by host threads as by CPUs, and nothing
//! about their job requires them to believe in a CPU. Translating a
//! `LogicalCpuId` into whatever slot index they want happens on this side.

#![cfg_attr(not(test), no_std)]

/// Build-time configuration, generated from `kcfg.bzl` by the build.
///
/// `include!` rather than a plain `mod`, because the file exists only in the
/// build output — `rustfmt` resolves a `mod` against the source tree and would
/// fail to find it.
mod kcfg {
    include!("kcfg.rs");
}

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

pub use crate::kcfg::MAX_CPUS;

/// Bits per word of a [`CpuSet`].
const WORD_BITS: usize = u64::BITS as usize;

/// Words needed to give every possible CPU a bit.
///
/// Derived here, where `MAX_CPUS` is a concrete constant. A type generic over
/// its capacity could not compute this: an array length derived from a generic
/// parameter needs `generic_const_exprs`.
const WORDS: usize = MAX_CPUS.div_ceil(WORD_BITS);

// A kernel with room for no CPUs cannot run, and would leave `CpuSet` with zero
// words to index. Rejected here so the config error reads as one.
const _: () = assert!(MAX_CPUS > 0, "kernel.max_cpus must be at least 1");

/// A CPU's dense, kernel-assigned identifier.
///
/// Ids run `0..MAX_CPUS`, so one is always a valid index into per-CPU state and
/// a valid member of a [`CpuSet`]. Which CPU gets which id is the kernel's
/// business; holding an id is not a claim that the CPU exists or is online.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LogicalCpuId(u32);

impl LogicalCpuId {
    /// The id with raw value `raw`.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// The id for table index `idx`, or `None` if no CPU can have it.
    #[must_use]
    pub fn from_index(idx: usize) -> Option<Self> {
        if idx >= MAX_CPUS {
            return None;
        }
        Some(Self(u32::try_from(idx).ok()?))
    }

    /// This id as an index into per-CPU state.
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// This id's raw value.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for LogicalCpuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A set of [`LogicalCpuId`]s, one bit each.
///
/// Every mutator takes `&self`, so a set can be a `static` and can be updated
/// from a trap handler: each is a single atomic read-modify-write, with no lock
/// and no allocation. That is what lets an idle or online mask be maintained on
/// paths where invariants 7 and 8 forbid both.
#[derive(Debug)]
pub struct CpuSet {
    words: [AtomicU64; WORDS],
}

impl CpuSet {
    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            words: [const { AtomicU64::new(0) }; WORDS],
        }
    }

    /// Every CPU the kernel can have, `0..MAX_CPUS`.
    ///
    /// Ids past `MAX_CPUS` in the final word stay clear, so a bit scan can
    /// never hand out an id no CPU can have.
    #[must_use]
    pub const fn all() -> Self {
        let mut words = [const { AtomicU64::new(0) }; WORDS];

        let mut idx = 0;
        while idx < WORDS {
            let low = idx * WORD_BITS;
            words[idx] = AtomicU64::new(if MAX_CPUS >= low + WORD_BITS {
                u64::MAX
            } else {
                // A partial final word: `MAX_CPUS - low` is in `1..WORD_BITS`,
                // because `WORDS` is a ceiling division.
                (1u64 << (MAX_CPUS - low)) - 1
            });
            idx += 1;
        }

        Self { words }
    }

    /// Whether `cpu` is a member.
    ///
    /// `Acquire`, so observing a CPU also observes whatever was published
    /// before [`atomic_set`][Self::atomic_set] added it.
    #[must_use]
    pub fn contains(&self, cpu: LogicalCpuId) -> bool {
        let (word, mask) = self.locate(cpu);
        word.load(Ordering::Acquire) & mask != 0
    }

    /// Add `cpu`, returning whether it was already a member.
    ///
    /// `AcqRel`, so writes made before this call are visible to anyone who
    /// observes the CPU in the set.
    pub fn atomic_set(&self, cpu: LogicalCpuId) -> bool {
        let (word, mask) = self.locate(cpu);
        word.fetch_or(mask, Ordering::AcqRel) & mask != 0
    }

    /// Remove `cpu`, returning whether it was a member.
    pub fn atomic_clear(&self, cpu: LogicalCpuId) -> bool {
        let (word, mask) = self.locate(cpu);
        word.fetch_and(!mask, Ordering::AcqRel) & mask != 0
    }

    /// The members, ascending.
    ///
    /// Each word is sampled once as it is reached, so this is a snapshot per
    /// word and not of the set as a whole: a concurrent change may or may not
    /// be seen, and two words may be seen at different instants.
    pub fn iter(&self) -> CpuSetIter<'_> {
        CpuSetIter {
            set: self,
            idx: 0,
            rest: self.words[0].load(Ordering::Acquire),
        }
    }

    /// # Panics
    ///
    /// Panics if `cpu` is past `MAX_CPUS`, which would alias a bit belonging to
    /// no CPU.
    fn locate(&self, cpu: LogicalCpuId) -> (&AtomicU64, u64) {
        let bit = cpu.as_usize();
        assert!(bit < MAX_CPUS, "cpu id past MAX_CPUS");
        (&self.words[bit / WORD_BITS], 1u64 << (bit % WORD_BITS))
    }
}

/// Iterator over the members of a [`CpuSet`], ascending.
#[derive(Debug)]
pub struct CpuSetIter<'a> {
    set: &'a CpuSet,
    idx: usize,
    /// Members of word `idx` not yet yielded.
    rest: u64,
}

impl Iterator for CpuSetIter<'_> {
    type Item = LogicalCpuId;

    fn next(&mut self) -> Option<LogicalCpuId> {
        while self.rest == 0 {
            self.idx += 1;
            if self.idx >= WORDS {
                return None;
            }
            self.rest = self.set.words[self.idx].load(Ordering::Acquire);
        }

        let bit = self.rest.trailing_zeros() as usize;
        // Clear the one we're about to yield.
        self.rest &= self.rest - 1;

        LogicalCpuId::from_index(self.idx * WORD_BITS + bit)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::vec::Vec;

    use super::*;

    fn cpu(idx: usize) -> LogicalCpuId {
        LogicalCpuId::from_index(idx).unwrap()
    }

    #[test]
    fn an_id_past_the_bound_is_not_a_cpu() {
        assert_eq!(LogicalCpuId::from_index(MAX_CPUS), None);
        assert!(LogicalCpuId::from_index(MAX_CPUS - 1).is_some());
    }

    #[test]
    fn empty_set_has_no_members() {
        let set = CpuSet::empty();

        assert_eq!(set.iter().next(), None);
        assert!(!set.contains(cpu(0)));
        assert!(!set.contains(cpu(MAX_CPUS - 1)));
    }

    #[test]
    fn all_holds_every_cpu_and_nothing_above() {
        let set = CpuSet::all();

        // The tail of the final word must stay clear, or a bit scan would hand
        // out an id no CPU can have.
        assert_eq!(set.iter().count(), MAX_CPUS);
        assert_eq!(set.iter().last(), Some(cpu(MAX_CPUS - 1)));
    }

    #[test]
    fn set_and_clear_report_the_previous_state() {
        let set = CpuSet::empty();
        let last = cpu(MAX_CPUS - 1);

        assert!(!set.atomic_set(last));
        assert!(set.atomic_set(last));
        assert!(set.contains(last));

        assert!(set.atomic_clear(last));
        assert!(!set.atomic_clear(last));
        assert!(!set.contains(last));
    }

    #[test]
    fn iter_yields_members_ascending() {
        let set = CpuSet::empty();

        // Spread across words where there are several, but stay valid down to a
        // uniprocessor `MAX_CPUS`.
        let mut members: Vec<_> = [0, 1, MAX_CPUS / 2, MAX_CPUS - 1]
            .into_iter()
            .filter(|&idx| idx < MAX_CPUS)
            .collect();
        members.sort_unstable();
        members.dedup();
        let members: Vec<_> = members.into_iter().map(cpu).collect();

        for &c in &members {
            set.atomic_set(c);
        }

        assert_eq!(set.iter().collect::<Vec<_>>(), members);
    }

    #[test]
    #[should_panic(expected = "cpu id past MAX_CPUS")]
    fn a_cpu_past_the_bound_cannot_join_a_set() {
        CpuSet::empty().atomic_set(LogicalCpuId::new(MAX_CPUS as u32));
    }
}
