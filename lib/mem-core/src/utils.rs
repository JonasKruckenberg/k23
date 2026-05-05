// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::range::{Range, RangeInclusive, RangeInclusiveIter};

use crate::VirtualAddress;
use crate::arch::{Arch, PageTableLevel};

/// Splits `range` into the per-entry sub-ranges of one table at `level`.
///
/// `range` must lie within a single table at `level`: a `PageTableEntries`
/// indexes one table, and each caller consumes it against one `Table`. A
/// cross-table range cannot be represented and panics in debug builds.
pub fn page_table_entries_for<A: Arch>(
    range: Range<VirtualAddress>,
    level: &PageTableLevel,
) -> PageTableEntries<A> {
    debug_assert!(
        {
            // `range` stays within one table at this level iff its first and last
            // byte differ only below the table's span. Masking drops the sign-
            // extension bits, so the whole-space root table always passes.
            let va_mask = (1usize << A::VIRTUAL_ADDRESS_BITS) - 1;
            let table_span = level.entries() as usize * level.page_size();
            ((range.start.get() ^ range.end.sub(1).get()) & va_mask) < table_span
        },
        "range {range:?} crosses a page-table boundary at this level (page size {})",
        level.page_size()
    );

    PageTableEntries {
        iter: RangeInclusive::from(
            level.pte_index_of(range.start)..=level.pte_index_of(range.end.sub(1)),
        )
        .into_iter(),
        page_start: range.start,
        max: range.end,
        page_size: level.page_size(),
        _arch: PhantomData,
    }
}

#[derive(Debug)]
pub struct PageTableEntries<A> {
    iter: RangeInclusiveIter<u16>,
    page_start: VirtualAddress,
    max: VirtualAddress,
    page_size: usize,
    _arch: PhantomData<A>,
}

impl<A: Arch> Iterator for PageTableEntries<A> {
    type Item = (u16, Range<VirtualAddress>);

    fn next(&mut self) -> Option<Self::Item> {
        let entry_index = self.iter.next()?;

        let slot_end = self
            .page_start
            .align_down(self.page_size)
            .saturating_add(self.page_size);
        let page_range = Range::from(self.page_start..slot_end.min(self.max));

        if page_range.is_empty() {
            return None;
        }

        self.page_start = page_range.end.canonicalize::<A>();

        Some((entry_index, page_range))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::for_every_arch;

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

    for_every_arch!(A => {
        proptest! {
            /// Regression test for [`PageTableEntries`] (review Blocker: `PageTableEntries::next`).
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
        fn rejects_a_range_crossing_a_table_boundary() {
            let level = &A::LEVELS[A::LEVELS.len() - 1];
            let table_span = level.entries() as usize * level.page_size();

            // Straddle the boundary between the first two tables.
            let range = Range::from(VirtualAddress::new(table_span - A::GRANULE_SIZE)
                ..VirtualAddress::new(table_span + A::GRANULE_SIZE));

            let _ = page_table_entries_for::<A>(range, level);
        }
    });
}
