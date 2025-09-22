// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod batch;
mod raw;
mod region;

use alloc::boxed::Box;
use core::alloc::Layout;
use core::ops::{Bound, ControlFlow, Range};
use core::ptr::NonNull;

use anyhow::{Context, format_err};
use batch::Batch;
use rand::Rng;
use rand::distr::Uniform;
use rand_chacha::ChaCha20Rng;
pub use raw::{Flush, RawAddressSpace};
use region::AddressSpaceRegion;
use wavltree2::{CursorMut, WAVLTree};

use crate::addresses::AddressRangeExt;
use crate::utils::assert_unsafe_precondition_;
use crate::{AccessRules, VirtualAddress};

pub struct AddressSpace<R: RawAddressSpace> {
    raw: R,
    regions: WAVLTree<AddressSpaceRegion>,
    batched_raw: Batch,
    max_range: Range<VirtualAddress>,
    rng: Option<ChaCha20Rng>,
}

impl<R: RawAddressSpace> AddressSpace<R> {
    pub const fn new(raw: R, rng: Option<ChaCha20Rng>) -> Self {
        Self {
            raw,
            regions: WAVLTree::new(),
            batched_raw: Batch::new(),
            max_range: VirtualAddress::MIN..VirtualAddress::MAX,
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

        let layout = layout.align_to(R::PAGE_SIZE.get()).unwrap();

        let spot = self
            .find_spot_for(layout)
            .context(format_err!("cannot find free spot for layout {layout:?}"))?;

        // TODO "relaxed" frame provider
        let region = AddressSpaceRegion::new(spot, layout, access_rules);

        let region = self.regions.insert(Box::pin(region));

        // TODO OPTIONAL eagerly commit a few pages

        self.batched_raw.flush_changes(&mut self.raw)?;

        Ok(region.as_non_null())
    }

    /// Behaves like [`map`][AddressSpace::map], but also *guarantees* the virtual memory region
    /// is zero-initialized.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or mapping otherwise fails.
    pub fn map_zeroed(
        &mut self,
        layout: Layout,
        access_rules: AccessRules,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::map_zeroed]");

        let layout = layout.align_to(R::PAGE_SIZE.get()).unwrap();

        let spot = self
            .find_spot_for(layout)
            .context(format_err!("cannot find free spot for layout {layout:?}"))?;

        // TODO "zeroed" frame provider
        let region = AddressSpaceRegion::new(spot, layout, access_rules);

        let region = self.regions.insert(Box::pin(region));

        // TODO OPTIONAL eagerly commit a few pages

        self.batched_raw.flush_changes(&mut self.raw)?;

        Ok(region.as_non_null())
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
    pub unsafe fn unmap(&mut self, ptr: NonNull<u8>, layout: Layout) {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::unmap]");

        // Safety: responsibility of caller
        let mut cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, layout) };

        // Safety: responsibility of caller
        let mut region = unsafe { cursor.remove().unwrap_unchecked() };

        region.decommit(.., &mut self.batched_raw).unwrap();

        self.batched_raw.flush_changes(&mut self.raw).unwrap();
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
    pub unsafe fn grow(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::grow]");

        let new_layout = new_layout.align_to(R::PAGE_SIZE.get()).unwrap();

        // Safety: responsibility of caller
        let mut cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, old_layout) };

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
    pub unsafe fn grow_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::grow_in_place]");

        let new_layout = new_layout.align_to(R::PAGE_SIZE.get()).unwrap();

        // Safety: responsibility of caller
        let mut cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, old_layout) };

        let next_range = cursor.peek_next().map(|region| region.range().clone());

        // Safety: responsibility of caller
        let mut region = unsafe { cursor.get_mut().unwrap_unchecked() };

        // TODO check against next region if we can map

        region.grow_in_place(new_layout, &mut self.batched_raw)?;

        self.batched_raw.flush_changes(&mut self.raw)?;

        Ok(region.as_non_null())
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
    pub unsafe fn shrink(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::shrink]");

        let new_layout = new_layout.align_to(R::PAGE_SIZE.get()).unwrap();

        // Safety: responsibility of caller
        let _cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, old_layout) };

        todo!()
    }

    /// Behaves like [`shrink`][AddressSpace::shrink], but *guarantees* that the region will be
    /// shrunk in-place.
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
    pub unsafe fn shrink_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::shrink_in_place]");

        let new_layout = new_layout.align_to(R::PAGE_SIZE.get()).unwrap();

        // Safety: responsibility of caller
        let mut cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, old_layout) };

        // Safety: responsibility of caller
        let mut region = unsafe { cursor.get_mut().unwrap_unchecked() };

        region.shrink(new_layout, &mut self.batched_raw)?;

        self.batched_raw.flush_changes(&mut self.raw)?;

        Ok(region.as_non_null())
    }

    /// Updates the access rules for the virtual memory region referenced by `ptr`.
    ///
    /// If this returns `Ok`, access to this region must obey the new `rules` or cause a hardware fault.
    ///
    /// If this method returns `Err`, the access rules of the memory region are unaltered.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    pub unsafe fn update_access_rules(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
        access_rules: AccessRules,
    ) -> crate::Result<()> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::update_access_rules]");

        // Safety: responsibility of caller
        let mut cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, layout) };

        // Safety: responsibility of caller
        let mut region = unsafe { cursor.get_mut().unwrap_unchecked() };

        region.update_access_rules(access_rules, &mut self.batched_raw)?;

        self.batched_raw.flush_changes(&mut self.raw)?;

        Ok(())
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
    pub unsafe fn clear(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        #[cfg(debug_assertions)]
        self.assert_valid("[AddressSpace::clear]");

        // Safety: responsibility of caller
        let mut cursor = unsafe { get_region_for_ptr(&mut self.regions, ptr, layout) };

        // Safety: responsibility of caller
        let mut region = unsafe { cursor.get_mut().unwrap_unchecked() };

        region.clear(&mut self.batched_raw)?;

        self.batched_raw.flush_changes(&mut self.raw)?;

        Ok(region.as_non_null())
    }

    pub fn assert_valid(&self, msg: &str) {
        let mut regions = self.regions.iter();

        let Some(first_region) = regions.next() else {
            assert!(
                self.regions.is_empty(),
                "{msg}region iterator is empty but tree is not."
            );

            return;
        };

        first_region.assert_valid(msg);

        let mut seen_range = first_region.range().clone();

        while let Some(region) = regions.next() {
            assert!(
                !region.range().is_overlapping(&seen_range),
                "{msg}region cannot overlap previous region; region={region:?}"
            );
            assert!(
                region.range().start >= self.max_range.start
                    && region.range().end <= self.max_range.end,
                "{msg}region cannot lie outside of max address space range; region={region:?}"
            );

            seen_range = seen_range.start..region.range().end;

            region.assert_valid(msg);

            // TODO assert validity of of VMO against phys addresses
            // let (_phys, access_rules) = self
            //     .batched_raw
            //     .raw_address_space()
            //     .lookup(region.range().start)
            //     .unwrap_or_else(|| {
            //         panic!("{msg}region base address is not mapped in raw address space region={region:?}")
            //     });
            //
            // assert_eq!(
            //     access_rules,
            //     region.access_rules(),
            //     "{msg}region's access rules do not match access rules in raw address space; region={region:?}, expected={:?}, actual={access_rules:?}",
            //     region.access_rules(),
            // );
        }
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

        let layout = layout.pad_to_align();

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

        // assert!(chosen.is_canonical());

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
        if v.visit(root.subtree_range().end..self.max_range.end)
            .is_break()
        {
            return;
        }
    }
}

/// # Safety
///
/// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
/// * `layout` must [*fit*] that region of memory.
///
/// [*currently mapped*]: #currently-mapped-memory
/// [*fit*]: #memory-fitting
unsafe fn get_region_for_ptr(
    regions: &mut WAVLTree<AddressSpaceRegion>,
    ptr: NonNull<u8>,
    layout: Layout,
) -> CursorMut<'_, AddressSpaceRegion> {
    let addr = VirtualAddress::from_non_null(ptr);

    let cursor = regions.lower_bound_mut(Bound::Included(&addr));

    assert_unsafe_precondition_!(
        "TODO",
        (cursor: &CursorMut<AddressSpaceRegion> = &cursor) => cursor.get().is_some()
    );

    // Safety: The caller guarantees the pointer is currently mapped which means we must have
    // a corresponding address space region for it
    let region = unsafe { cursor.get().unwrap_unchecked() };

    assert_unsafe_precondition_!(
        "TODO",
        (region: &AddressSpaceRegion = region, addr: VirtualAddress = addr) => {
            let range = region.range();

            range.start.get() <= addr.get() && addr.get() < range.end.get()
        }
    );

    assert_unsafe_precondition_!(
        "`layout` does not fit memory region",
        (layout: Layout = layout, region: &AddressSpaceRegion = &region) => region.layout_fits_region(layout)
    );

    cursor
}

pub(crate) struct GapVisitor {
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

    pub fn layout(&self) -> Layout {
        self.layout
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
            ((range_size - self.layout.size()) >> self.layout.align().ilog2()) + 1
        } else {
            0
        }
    }

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
}

#[cfg(test)]
mod tests {
    // tests for `find_spot_for`

    mod aslr_enabled {
        use alloc::vec;
        use alloc::vec::Vec;
        use core::alloc::Layout;
        use core::cmp;
        use core::ops::Range;

        use proptest::prelude::*;
        use proptest::sample::{select, size_range};
        use rand::SeedableRng;
        use rand_chacha::ChaCha20Rng;

        use crate::test_utils::TestAddressSpace;
        use crate::{AccessRules, AddressSpace, VirtualAddress};

        const POWERS_OF_TWO: [usize; 17] = [
            1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
        ];

        prop_compose! {
            fn nonoverlapping_ranges(len: usize)
                (offsets in prop::collection::vec(any::<usize>(), 0..len*2))
                -> Vec<Range<usize>> {
                let mut result = Vec::with_capacity(offsets.len() / 2);

                let mut base: usize = 0;
                for chunk in offsets.chunks(2) {
                    let [start_offset, end_offset, ..] = chunk else {
                        panic!()
                    };

                    let Some(start) = base.checked_add(*start_offset) else {
                        return result;
                    };
                     let Some(end) = start.checked_add(*end_offset) else {
                        return result;
                    };

                    result.push(start..end);

                    base = end;
                }

                result
           }
        }

        // Finding a spot in an empty address space should always succeed.
        proptest! {
            #[test]
            fn find_spot_empty_aspace(layout_size in 1..usize::MAX/2, layout_align in select(&POWERS_OF_TWO), rng_seed: u64) {
                let mut aspace = AddressSpace::new(TestAddressSpace::<4096>::new(), Some(ChaCha20Rng::seed_from_u64(rng_seed)));

                let layout = Layout::from_size_align(layout_size, layout_align).unwrap();
                let spot = aspace.find_spot_for(layout);

                prop_assert!(spot.is_some());
                let spot = spot.unwrap();
                prop_assert!(spot.is_aligned_to(layout_align));
            }

            #[test]
            fn maps(size_and_aligns in proptest::collection::vec((1..usize::MAX/2, select(&POWERS_OF_TWO)), 10), rng_seed: u64) {
                let mut aspace = AddressSpace::new(TestAddressSpace::<4096>::new(), Some(ChaCha20Rng::seed_from_u64(rng_seed)));

                let mut used_size: usize = 0;

                for (size, align) in size_and_aligns {
                    let layout = Layout::from_size_align(size, align).unwrap();

                    let res = aspace.map(layout, AccessRules::new());

                    if let (new_used_size, false) = used_size.overflowing_add(layout.pad_to_align().size()) {
                        prop_assert!(res.is_ok());

                        used_size = new_used_size;

                        let ptr = res.unwrap();

                        let addr = VirtualAddress::from_non_null(ptr);
                        prop_assert!(addr.is_aligned_to(layout.align()));
                        prop_assert_eq!(ptr.len(), size);
                    } else {
                        prop_assert!(res.is_err());
                    }
                }
            }
        }
    }

    mod aslr_disabled {}
}
