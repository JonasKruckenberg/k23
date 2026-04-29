// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]

//! The algorithm we use here - loosely based on Zircon's (Fuchsia's) implementation - is
//! guaranteed to find a spot (if any even exist) with max 2 attempts. Additionally, it works
//! elegantly *with* AND *without* ASLR, picking a random spot or the lowest free spot respectively.
//! Here is how it works:
//! 1. We set up two counters:
//!    - `candidate_spot_count` which we initialize to zero
//!    - `target_index` which we either set to a random value between 0..<the maximum number of
//!      possible addresses in the address space if ASLR is requested OR to zero otherwise.
//! 2. We then iterate over all the gaps between virtual address regions from lowest to highest looking.
//!    We count the number of addresses in each gap that satisfy the requested `Layout`s size and
//!    alignment and add that to the `candidate_spot_count`. IF the number of spots in the gap is
//!    greater than our chosen target index, we pick the spot at the target index and finish.
//!    ELSE we *decrement* the target index by the number of spots and continue to the next gap.
//! 3. After we have processed all the gaps, we have EITHER found a suitable spot OR our original
//!    guess for `target_index` was too big, in which case we need to retry.
//! 4. When retrying we iterate over all gaps between virtual address regions *again*, but this time
//!    we know the *actual* number of possible spots in the address space since we just counted them
//!    during the first attempt. We initialize `target_index` to `0..candidate_spot_count`
//!    which is guaranteed to return us a spot.
//!    IF `candidate_spot_count` is ZERO after the first attempt, there is no point in
//!    retrying since we cannot fulfill the requested layout.
//!
//! Note that in practice, we use a binary tree to keep track of regions, and we use binary search
//! to optimize the search for a suitable gap instead of linear iteration.

use core::alloc::Layout;
use core::marker::PhantomData;
use core::ops::Range;

use kmem_core::{AddressRangeExt, Arch, VirtualAddress};
use rand::Rng;
use rand::distr::Uniform;
use rand_chacha::ChaCha20Rng;

pub struct Randomizer<A: Arch> {
    rng: Option<ChaCha20Rng>,
    _arch: PhantomData<A>,
}

impl<A: Arch> Randomizer<A> {
    pub const fn new(rng: Option<ChaCha20Rng>) -> Self {
        Self {
            rng,
            _arch: PhantomData,
        }
    }

    /// Find a spot in the given `gaps` that satisfies the given `layout` requirements.
    ///
    /// If a spot suitable for holding data described by `layout` is found, the base address of the
    /// address range is returned in `Some`. The returned address is already correct aligned to
    /// `layout.align()`.
    ///
    /// Returns `None` if no suitable spot was found. This *does not* mean there are no more gaps in
    /// the address space just that the *combination* of `layout.size()` and `layout.align()` cannot
    /// be satisfied *at the moment*. Calls to this method will a different size, alignment, or at a
    /// different time might still succeed.
    #[expect(clippy::missing_panics_doc, reason = "internal assert")]
    pub fn find_spot_in(
        &mut self,
        layout: Layout,
        gaps: impl Iterator<Item = Range<VirtualAddress>> + Clone,
    ) -> Option<VirtualAddress> {
        let layout = layout.align_to(A::GRANULE_SIZE).unwrap();

        // First attempt: guess a random target index from all possible virtual addresses
        let max_candidate_spots = const {
            // The bits of entropy that we can pick a virtual address from. The theoretical maximum
            // is just the number of usable bits in a virtual address. E.g. if a given architecture
            // supports 48 bits virtual addresses, then our theoretical maximum entropy is 47 bits.
            // But, because the virtual memory subsystem doesn't deal in byte-granularity allocations
            // but in pages we need to subtract the number of bits that are just used to encode
            // the offset within a page (which is the log2 of the translation granule size).
            let entropy = A::VIRTUAL_ADDRESS_BITS as u32 - A::GRANULE_SIZE.ilog2();

            (1 << entropy) - 1
        };

        let distr = Uniform::new(0, max_candidate_spots)
            .expect("no candidate spots in max range, this is a bug!");
        let target_index: usize = self
            .rng
            .as_mut()
            .map(|rng| rng.sample(distr))
            .unwrap_or_default();

        // First attempt: visit the binary search tree to find a gap
        choose_spot(layout, target_index, gaps.clone())
            // Second attempt: pick a new target_index that's actually fulfillable
            // based on the candidate spots we counted during the previous attempt
            .map_err(|candidate_spots| {
                // if we counted no suitable candidate spots during the first attempt, we cannot fulfill
                // the request.
                if candidate_spots == 0 {
                    return None;
                }

                let distr = Uniform::new(0, candidate_spots).unwrap();

                let target_index: usize = self
                    .rng
                    .as_mut()
                    .map(|rng| rng.sample(distr))
                    .unwrap_or_default();

                let chosen_spot = choose_spot(layout, target_index, gaps)
                    .expect("There must be a chosen spot after the first attempt. This is a bug!");

                Some(chosen_spot)
            })
            .ok()
    }
}

fn choose_spot(
    layout: Layout,
    mut target_index: usize,
    gaps: impl Iterator<Item = Range<VirtualAddress>>,
) -> Result<VirtualAddress, usize> {
    let mut candidate_spots = 0;

    for gap in gaps {
        let aligned_gap = gap.align_in(layout.align());

        let spot_count = spots_in_range(layout, &aligned_gap);

        candidate_spots += spot_count;

        if target_index < spot_count {
            return Ok(aligned_gap
                .start
                .add(target_index << layout.align().ilog2()));
        } else {
            target_index -= spot_count;
        }
    }
    Err(candidate_spots)
}

/// Returns the number of spots in the given range that satisfy the layout we require
fn spots_in_range(layout: Layout, range: &Range<VirtualAddress>) -> usize {
    debug_assert!(
        range.start.is_aligned_to(layout.align()) && range.end.is_aligned_to(layout.align())
    );

    // ranges passed in here can become empty for a number of reasons (aligning might produce ranges
    // where end > start, or the range might be empty to begin with) in either case an empty
    // range means no spots are available
    if range.is_empty() {
        return 0;
    }

    let range_size = range.len();
    if range_size >= layout.size() {
        ((range_size - layout.size()) >> layout.align().ilog2()) + 1
    } else {
        0
    }
}
