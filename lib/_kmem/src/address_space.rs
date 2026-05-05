// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod region;

use alloc::boxed::Box;
use core::alloc::Layout;
use core::ops::{ControlFlow, Range};
use core::ptr::NonNull;

use anyhow::{Context, format_err};
use rand::Rng;
use rand::distr::Uniform;
use rand_chacha::ChaCha20Rng;
use wavltree::WAVLTree;

use crate::address_space::region::AddressSpaceRegion;
use crate::{AccessRules, AddressRangeExt, VirtualAddress};

/// # Safety
///
/// The correct and safe functioning of the entire crate depends on the correct implementation of this
/// trait. In particular implementors must ensure:
///
/// - the declared PAGE_SIZE matches the actual smallest page size of the target.
pub unsafe trait RawAddressSpace {
    /// The smallest addressable chunk of memory of this address space. All address argument provided
    /// to methods of this type (both virtual and physical) must be aligned to this.
    const PAGE_SIZE: usize;
    const VIRT_ADDR_BITS: u32;

    #[expect(
        clippy::cast_possible_truncation,
        reason = "cannot use try_from in const expr"
    )]
    const PAGE_SIZE_LOG_2: u8 = (Self::PAGE_SIZE - 1).count_ones() as u8;
    const CANONICAL_ADDRESS_MASK: usize = !((1 << (Self::VIRT_ADDR_BITS)) - 1);
}

pub struct AddressSpace<R> {
    #[expect(unused, reason = "used by later changes")]
    raw: R,
    regions: WAVLTree<AddressSpaceRegion>,
    max_range: Range<VirtualAddress>,
    rng: Option<ChaCha20Rng>,
}

// ===== impl AddressSpace =====

impl<R: RawAddressSpace> AddressSpace<R> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "cannot use try_from in const expr"
    )]
    const __RAW_ASPACE_ASSERT: () = {
        assert!(R::PAGE_SIZE.ilog2() as u8 == R::PAGE_SIZE_LOG_2,);
        assert!(R::CANONICAL_ADDRESS_MASK == !((1 << (R::VIRT_ADDR_BITS)) - 1),);
    };

    pub const fn new(raw: R, max_range: Range<VirtualAddress>, rng: Option<ChaCha20Rng>) -> Self {
        Self {
            raw,
            regions: WAVLTree::new(),
            max_range,
            rng,
        }
    }

    /// Attempts to reserve a region of virtual memory.
    ///
    /// On success, returns a [`NonNull<[u8]>`][NonNull] meeting the size and alignment guarantees
    /// of `layout`. Access to this region must obey the provided `rules` or cause a hardware fault.
    ///
    /// The returned region may have a larger size than specified by `layout.size()`, and may or may
    /// not have its contents initialized.
    ///
    /// The returned region of virtual memory remains mapped as long as it is [*currently mapped*]
    /// and the address space type itself has not been dropped.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or mapping otherwise fails.
    pub fn map(
        &mut self,
        layout: Layout,
        access_rules: AccessRules,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::map]");

        let spot = self
            .find_spot_for(layout)
            .with_context(|| format_err!("cannot find free spot for layout {layout:?}"))?;

        let region = AddressSpaceRegion::new(
            spot,
            access_rules,
            #[cfg(debug_assertions)]
            layout,
        );
        let region = self.regions.insert(Box::pin(region));

        // TODO OPTIONAL eagerly commit a few pages

        Ok(region.as_non_null())
    }

    /// Attempts to extend the virtual memory reservation.
    ///
    /// Returns a new [`NonNull<[u8]>`][NonNull] containing a pointer and the actual size of the
    /// mapped region. The pointer is suitable for holding data described by `new_layout`. To accomplish
    /// this, the address space may extend the mapping referenced by `ptr` to fit the new layout.
    ///
    /// TODO describe how extending a file-backed, of DMA-backed mapping works
    ///
    /// The [`AccessRules`] of the new virtual memory region are *the same* at the old ones.
    ///
    /// If this returns `Ok`, then ownership of the memory region referenced by `ptr` has been
    /// transferred to this address space. Any access to the old `ptr` is [*Undefined Behavior*],
    /// even if the mapping was grown in-place. The newly returned pointer is the only valid pointer
    /// for accessing this region now.
    ///
    /// If this method returns `Err`, then ownership of the memory region has not been transferred to
    /// this address space, and the contents of the region are unaltered.
    ///
    /// [*Undefined Behavior*]
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be greater than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or growing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn grow(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Behaves like [`grow`][AddressSpace::grow], only grows the region if it can be grown in-place.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be greater than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or growing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn grow_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Attempts to shrink the virtual memory reservation.
    ///
    /// Returns a new [`NonNull<[u8]>`][NonNull] containing a pointer and the actual size of the
    /// mapped region. The pointer is suitable for holding data described by `new_layout`. To accomplish
    /// this, the address space may shrink the mapping referenced by `ptr` to fit the new layout.
    ///
    /// TODO describe how shrinking a file-backed, of DMA-backed mapping works
    ///
    /// The [`AccessRules`] of the new virtual memory region are *the same* at the old ones.
    ///
    /// If this returns `Ok`, then ownership of the memory region referenced by `ptr` has been
    /// transferred to this address space. Any access to the old `ptr` is [*Undefined Behavior*],
    /// even if the mapping was shrunk in-place. The newly returned pointer is the only valid pointer
    /// for accessing this region now.
    ///
    /// If this method returns `Err`, then ownership of the memory region has not been transferred to
    /// this address space, and the contents of the region are unaltered.
    ///
    /// [*Undefined Behavior*]
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be smaller than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or shrinking otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn shrink(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Behaves like [`shrink`][AddressSpace::shrink], but *guarantees* that the region will be
    /// shrunk in-place. Both `old_layout` and `new_layout` need to be at least page aligned.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be smaller than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or growing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn shrink_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Unmaps the virtual memory region referenced by `ptr`.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn unmap(&mut self, ptr: NonNull<u8>, layout: Layout) {
        todo!()
    }

    /// Updates the access rules for the virtual memory region referenced by `ptr`.
    ///
    /// After this returns, access to this region must obey the new `rules` or cause a hardware fault.
    // If this returns `Ok`, access to this region must obey the new `rules` or cause a hardware fault.
    // If this method returns `Err`, the access rules of the memory region are unaltered.
    ///
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn update_access_rules(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
        access_rules: AccessRules,
    ) {
        todo!()
    }

    /// Attempts to fill the virtual memory region referenced by `ptr` with zeroes.
    ///
    /// Returns a new [`NonNull<[u8]>`][NonNull] containing a pointer and the actual size of the
    /// mapped region. The pointer is suitable for holding data described by `new_layout` and is
    /// *guaranteed* to be zero-initialized. To accomplish this, the address space may remap the
    /// virtual memory region.
    ///
    /// TODO describe how clearing a file-backed, of DMA-backed mapping works
    ///
    /// The [`AccessRules`] of the new virtual memory region are *the same* at the old ones.
    ///
    /// If this returns `Ok`, then ownership of the memory region referenced by `ptr` has been
    /// transferred to this address space. Any access to the old `ptr` is [*Undefined Behavior*],
    /// even if the mapping was cleared in-place. The newly returned pointer is the only valid pointer
    /// for accessing this region now.
    ///
    /// If this method returns `Err`, then ownership of the memory region has not been transferred to
    /// this address space, and the contents of the region are unaltered.
    ///
    /// [*Undefined Behavior*]
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, clearing a virtual memory region is not supported by the backing storage, or
    /// clearing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn clear(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    pub fn assert_valid(&self, ctx: &str) {
        self.regions.assert_valid(ctx);
    }

    /// Find a spot in the address space that satisfies the given `layout` requirements.
    ///
    /// If a spot suitable for holding data described by `layout` is found, the base address of the
    /// address range is returned in `Some`. The returned address is already correct aligned to
    /// `layout.align()`.
    ///
    /// Returns `None` if no suitable spot was found. This *does not* mean there are no more gaps in
    /// the address space just that the *combination* of `layout.size()` and `layout.align()` cannot
    /// be satisfied *at the moment*. Calls to this method will a different size, alignment, or at a
    /// different time might still succeed.
    fn find_spot_for(&mut self, layout: Layout) -> Option<VirtualAddress> {
        // The algorithm we use here - loosely based on Zircon's (Fuchsia's) implementation - is
        // guaranteed to find a spot (if any even exist) with max 2 attempts. Additionally, it works
        // elegantly *with* AND *without* ASLR, picking a random spot or the lowest free spot respectively.
        // Here is how it works:
        // 1. We set up two counters: (see the GapVisitor)
        //    - `candidate_spot_count` which we initialize to zero
        //    - `target_index` which we either set to a random value between 0..<the maximum number of
        //       possible addresses in the address space> if ASLR is requested OR to zero otherwise.
        // 2. We then iterate over all `AddressSpaceRegion`s from lowest to highest looking at the
        //    gaps between regions. We count the number of addresses in each gap that satisfy the
        //    requested `Layout`s size and alignment and add that to the `candidate_spot_count`.
        //    IF the number of spots in the gap is greater than our chosen target index, we pick the
        //    spot at the target index and finish. ELSE we *decrement* the target index by the number
        //    of spots and continue to the next gap.
        // 3. After we have processed all the gaps, we have EITHER found a suitable spot OR our original
        //    guess for `target_index` was too big, in which case we need to retry.
        // 4. When retrying we iterate over all `AddressSpaceRegion`s *again*, but this time we know
        //    the *actual* number of possible spots in the address space since we just counted them
        //    during the first attempt. We initialize `target_index` to `0..candidate_spot_count`
        //    which is guaranteed to return us a spot.
        //    IF `candidate_spot_count` is ZERO after the first attempt, there is no point in
        //    retrying since we cannot fulfill the requested layout.
        //
        // Note that in practice, we use a binary tree to keep track of regions, and we use binary search
        // to optimize the search for a suitable gap instead of linear iteration.

        // First attempt: guess a random target index
        let max_candidate_spots = self.max_range.size();

        let target_index: usize = self
            .rng
            .as_mut()
            .map(|prng| prng.sample(Uniform::new(0, max_candidate_spots).unwrap()))
            .unwrap_or_default();

        // First attempt: visit the binary search tree to find a gap
        let mut v = GapVisitor::new(layout, target_index);
        self.visit_gaps(&mut v);

        // if we found a spot already we're done
        if let Some(chosen) = v.chosen {
            return Some(chosen);
        }

        // otherwise, Second attempt: we need to retry with the correct candidate spot count
        // but if we counted no suitable candidate spots during the first attempt, we cannot fulfill
        // the request.
        if v.candidate_spots == 0 {
            return None;
        }

        // Second attempt: pick a new target_index that's actually fulfillable
        let target_index: usize = self
            .rng
            .as_mut()
            .map(|prng| prng.sample(Uniform::new(0, v.candidate_spots).unwrap()))
            .unwrap_or_default();

        // Second attempt: visit the binary search tree to find a gap
        let mut v = GapVisitor::new(layout, target_index);
        self.visit_gaps(&mut v);

        let chosen = v
            .chosen
            .expect("There must be a chosen spot after the first attempt. This is a bug!");

        Some(chosen)
    }

    /// Visit all gaps (address ranges not covered by an [`AddressSpaceRegion`]) in this address space
    /// from lowest to highest addresses.
    fn visit_gaps(&self, v: &mut GapVisitor) {
        let Some(root) = self.regions.root().get() else {
            // if the tree is empty, we treat the entire max_range as the gap
            // note that we do not care about the returned ControlFlow, as there is nothing else we
            // could try to find a spot anyway
            let _ = v.visit(self.max_range.clone());

            return;
        };

        // see if there is a suitable gap between BEFORE the first address space region
        if v.visit(self.max_range.start..root.subtree_range().start)
            .is_break()
        {
            return;
        }

        // now comes the main part of the search. we start at the WAVLTree root node and do a
        // binary search for a suitable gap. We use special metadata on each `AddressSpaceRegion`
        // to speed up this search. See `AddressSpaceRegion`  for details on how this works.

        let mut maybe_current = self.regions.root().get();
        let mut already_visited = VirtualAddress::MIN;

        while let Some(current) = maybe_current {
            // If there is no suitable gap in this entire
            if current.suitable_gap_in_subtree(v.layout()) {
                // First, look at the left subtree
                if let Some(left) = current.left_child() {
                    if left.suitable_gap_in_subtree(v.layout())
                        && left.subtree_range().end > already_visited
                    {
                        maybe_current = Some(left);
                        continue;
                    }

                    if v.visit(left.subtree_range().end..current.range().start)
                        .is_break()
                    {
                        return;
                    }
                }

                if let Some(right) = current.right_child() {
                    if v.visit(current.range().end..right.subtree_range().start)
                        .is_break()
                    {
                        return;
                    }

                    if right.suitable_gap_in_subtree(v.layout())
                        && right.subtree_range().end > already_visited
                    {
                        maybe_current = Some(right);
                        continue;
                    }
                }
            }

            already_visited = current.subtree_range().end;
            maybe_current = current.parent();
        }

        // see if there is a suitable gap between AFTER the last address space region
        // NB: regardless of the function return we reached the end of this function anyway, there
        // is nothing else we can do. It's therefore fine to ignore the result.
        let _ = v.visit(root.subtree_range().end..self.max_range.end);
    }
}

struct GapVisitor {
    layout: Layout,
    target_index: usize,
    candidate_spots: usize,
    chosen: Option<VirtualAddress>,
}

impl GapVisitor {
    fn new(layout: Layout, target_index: usize) -> Self {
        Self {
            layout,
            target_index,
            candidate_spots: 0,
            chosen: None,
        }
    }

    /// Returns the [`Layout`] that we are allocating for.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Visitor callback for a gap in the tree. This method will return `ControlFlow::Continue`
    /// to indicate the caller should continue traversing the tree and `ControlFlow::Break` to stop.
    pub fn visit(&mut self, gap: Range<VirtualAddress>) -> ControlFlow<()> {
        // if we have already chosen a spot, signal the caller to stop
        if self.chosen.is_some() {
            return ControlFlow::Break(());
        }

        let aligned_gap = gap.checked_align_in(self.layout.align()).unwrap();

        let spot_count = self.spots_in_range(&aligned_gap);

        self.candidate_spots += spot_count;

        if self.target_index < spot_count {
            self.chosen = Some(
                aligned_gap
                    .start
                    .checked_add(self.target_index << self.layout.align().ilog2())
                    .unwrap(),
            );

            ControlFlow::Break(())
        } else {
            self.target_index -= spot_count;

            ControlFlow::Continue(())
        }
    }

    /// Returns the number of spots in the given range that satisfy the layout we require
    fn spots_in_range(&self, range: &Range<VirtualAddress>) -> usize {
        debug_assert!(
            range.start.is_aligned_to(self.layout.align())
                && range.end.is_aligned_to(self.layout.align())
        );

        // ranges passed in here can become empty for a number of reasons (aligning might produce ranges
        // where end > start, or the range might be empty to begin with) in either case an empty
        // range means no spots are available
        if range.is_empty() {
            return 0;
        }

        let range_size = range.size();
        if range_size >= self.layout.size() {
            range_size / self.layout.size()
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use rand::SeedableRng;

    use super::*;
    use crate::test_utils::TestAddressSpace;

    const PAGE_SIZE: usize = 4096;
    /// Hardcoded seed for the ASLR CPRNG to remove the reliance on high-quality entropy for tests
    /// (because that can be an issue in CI runners).
    /// THIS MUST ONLY EVER BE USED FOR TESTS AND NEVER NEVER NEVER FOR PRODUCTION CODE
    const ASLR_SEED: [u8; 32] = [
        232, 66, 52, 206, 40, 195, 141, 166, 130, 237, 114, 177, 190, 54, 88, 88, 30, 196, 41, 165,
        54, 85, 157, 181, 124, 91, 106, 9, 179, 48, 75, 245,
    ];

    #[test]
    fn find_spot_for_no_aslr() {
        // ===== find a spot *after* the regions =====
        let mut aspace = AddressSpace::new(
            TestAddressSpace::<PAGE_SIZE, 38>::new(),
            VirtualAddress::MIN..VirtualAddress::MAX,
            None,
        );

        aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(0),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap(),
        )));

        aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(4 * PAGE_SIZE),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap(),
        )));

        // we know the following: NO gap to the left, NO gap between the regions and A BIG gap after
        // we therefore expect even the smallest layout to be placed AFTER 9 * 4096
        let spot = aspace
            .find_spot_for(Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap())
            .unwrap();

        assert_eq!(spot, VirtualAddress::new(8 * PAGE_SIZE));

        // ===== find a spot *between* the regions =====

        let mut aspace = AddressSpace::new(
            TestAddressSpace::<PAGE_SIZE, 38>::new(),
            VirtualAddress::MIN..VirtualAddress::MAX,
            None,
        );

        aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(0),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap(),
        )));

        aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(5 * PAGE_SIZE),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap(),
        )));

        // we know the following: NO gap to the left, ONE page gap between the regions and A BIG gap after
        let spot = aspace
            .find_spot_for(Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap())
            .unwrap();

        assert_eq!(spot, VirtualAddress::new(4 * PAGE_SIZE));

        // ===== find a spot *before* the regions =====

        let mut aspace = AddressSpace::new(
            TestAddressSpace::<PAGE_SIZE, 38>::new(),
            VirtualAddress::MIN..VirtualAddress::MAX,
            None,
        );

        aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(4 * PAGE_SIZE),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap(),
        )));

        aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(8 * PAGE_SIZE),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap(),
        )));

        // we know the following: NO gap to the left, ONE page gap between the regions and A BIG gap after
        let spot = aspace
            .find_spot_for(Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap())
            .unwrap();

        assert_eq!(spot, VirtualAddress::new(0));
    }

    #[test]
    fn find_spot_aslr_entropy() {
        let layout = Layout::from_size_align(4096, 4096).unwrap();

        let mut aspace = AddressSpace::new(
            TestAddressSpace::<PAGE_SIZE, 38>::new(),
            VirtualAddress::MIN..VirtualAddress::MAX,
            Some(ChaCha20Rng::from_seed(ASLR_SEED)),
        );

        // first we fill up the address space with 100 randomly placed regions just so
        // we are not just testing the entropy of the RNG
        for _ in 0..100 {
            let spot = aspace.find_spot_for(layout).unwrap();
            aspace.regions.insert(Box::pin(AddressSpaceRegion::new(
                spot,
                AccessRules::new().with(AccessRules::READ, true),
                layout,
            )));
        }

        let mut rng = ChaCha20Rng::from_os_rng();

        // then we sample the algorithm 500 times, adding the chosen spots to the test buffer
        let mut data = Vec::new();
        for _ in 0..5000 {
            let spot = aspace.find_spot_for(layout).unwrap();

            // because all spots are page-aligned we need to generate the lower 12 bits of randomness for
            // our statistical test to pass
            let spot = spot.get() | rng.sample(Uniform::new(0, 4096).unwrap());

            data.push(VirtualAddress::new(spot));
        }

        // finally we run the Frequency (Monobit) Test over the collected data so see if
        // we get proper distribution
        let (passed, pval) = frequency_test(&data);

        assert!(passed, "test returned P-value of {pval} (expected >= 0.01)");
    }

    /// Implements the Frequency (Monobit) Test
    ///
    /// This function calculates the proportion of zero-bits and one-bits in `spots` expecting
    /// it to be approximately the same as for a truly random sequence.
    ///
    /// from NIST "STATISTICAL TEST SUITE FOR RANDOM AND PSEUDORANDOM NUMBER GENERATORS FOR CRYPTOGRAPHIC APPLICATIONS"
    /// (<https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-22r1a.pdf>)
    pub fn frequency_test(spots: &[VirtualAddress]) -> (bool, f64) {
        const TEST_THRESHOLD: f64 = 0.01;

        let nbits = spots.len() * VirtualAddress::BITS as usize;
        let ones: usize = spots
            .into_iter()
            .map(|spot| spot.get().count_ones() as usize)
            .sum();

        let sn = 2 * ones as isize - nbits as isize;
        let sobs = sn.abs() as f64 / (nbits as f64).sqrt();
        let p = libm::erfc(sobs / 2.0_f64.sqrt());

        (p >= TEST_THRESHOLD, p)
    }
}
