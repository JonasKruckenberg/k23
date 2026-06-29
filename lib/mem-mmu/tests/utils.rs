// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use mem_core::VirtualAddress;
use mem_core::arch::{Arch, PageTableLevel};
use mem_mmu::page_table_entries_for;
use mem_testkit::for_arch;
use proptest::prelude::*;

/// A page table level paired with a granule-aligned, non-empty virtual range
/// contained within a single table at that level and within the canonical
/// lower half of `A` (so `canonicalize` in `PageTableEntries::next` is the
/// identity).
fn level_and_range<A: Arch>()
-> impl Strategy<Value = (&'static PageTableLevel, Range<VirtualAddress>)> {
    // Every range a caller passes is aligned to the smallest page size.
    let granule = A::GRANULE_SIZE;
    let canonical_half = 1usize << (A::VIRTUAL_ADDRESS_BITS - 1);

    (0..A::LEVELS.len()).prop_flat_map(move |level_idx| {
        let level = &A::LEVELS[level_idx];
        // Granule-aligned offsets that fit inside one table at this level,
        // clamped to the canonical lower half.
        let slots =
            (level.entries() as usize * level.page_size()).min(canonical_half) / granule;

        // Pick two distinct offsets; the smaller is the start, the larger the end.
        (0..slots, 0..slots).prop_filter_map("range must be non-empty", move |(a, b)| {
            (a != b).then(|| {
                let start = VirtualAddress::new(a.min(b) * granule);
                let end = VirtualAddress::new(a.max(b) * granule);
                (level, Range::from(start..end))
            })
        })
    })
}

for_arch!(A in [
    Riscv64Sv39,
    #[cfg(not(miri))]
    Riscv64Sv48,
    #[cfg(not(miri))]
    Riscv64Sv57,
] {
    proptest! {
        /// Regression test for `PageTableEntries` (review Blocker: `PageTableEntries::next`).
        ///
        /// For every page table level, `page_table_entries_for` must split a range into
        /// per-entry sub-ranges that tile the input exactly, with each sub-range
        /// confined to one naturally-aligned entry slot. The buggy `next` ends a
        /// sub-range at `page_start + page_size` from an unaligned `page_start`,
        /// overshooting the slot and dropping later entries.
        #[test]
        fn entries_tile_the_range_within_aligned_slots(
            (level, range) in level_and_range::<A>(),
        ) {
            let page_size = level.page_size();

            let entries: Vec<(u16, Range<VirtualAddress>)> =
                page_table_entries_for::<A>(range, level).collect();

            // A non-empty range yields at least one entry.
            prop_assert!(!entries.is_empty());

            // The sub-ranges tile the input range exactly, in ascending order.
            prop_assert_eq!(entries.first().unwrap().1.start, range.start);
            prop_assert_eq!(entries.last().unwrap().1.end, range.end);
            for pair in entries.windows(2) {
                prop_assert_eq!(pair[0].1.end, pair[1].1.start);
            }

            // Each sub-range stays within a single naturally-aligned slot and carries
            // the matching PTE index.
            for (index, sub) in &entries {
                prop_assert!(sub.start < sub.end);
                let slot_start = sub.start.align_down(page_size);
                prop_assert!(sub.end.get() <= slot_start.get() + page_size);
                prop_assert_eq!(*index, level.pte_index_of(sub.start));
            }
        }
    }

    /// A cross-table range is invalid input: `PageTableEntries` indexes one
    /// table, so it must panic rather than silently truncate.
    #[test]
    #[should_panic = "crosses a page-table boundary"]
    #[cfg_attr(not(debug_assertions), ignore)]
    fn rejects_a_range_crossing_a_table_boundary() {
        let level = &A::LEVELS[A::LEVELS.len() - 1];
        let table_span = level.entries() as usize * level.page_size();

        // Straddle the boundary between the first two tables.
        let range = Range::from(
            VirtualAddress::new(table_span - A::GRANULE_SIZE)
                ..VirtualAddress::new(table_span + A::GRANULE_SIZE),
        );

        let _ = page_table_entries_for::<A>(range, level);
    }
});
