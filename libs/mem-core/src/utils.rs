use core::marker::PhantomData;
use core::range::{IterRangeInclusive, Range, RangeInclusive};

use crate::VirtualAddress;
use crate::arch::{Arch, PageTableLevel};

// TODO: tests
//  - ensure this only returns in-bound indices
pub fn page_table_entries_for<A: Arch>(
    range: Range<VirtualAddress>,
    level: &PageTableLevel,
) -> PageTableEntries<A> {
    PageTableEntries {
        iter: RangeInclusive {
            start: level.pte_index_of(range.start),
            end: level.pte_index_of(range.end.sub(1)),
        }
        .iter(),
        page_start: range.start,
        max: range.end,
        page_size: level.page_size(),
        _arch: PhantomData,
    }
}

#[derive(Debug)]
pub struct PageTableEntries<A> {
    iter: IterRangeInclusive<u16>,
    page_start: VirtualAddress,
    max: VirtualAddress,
    page_size: usize,
    _arch: PhantomData<A>,
}

impl<A: Arch> Iterator for PageTableEntries<A> {
    type Item = (u16, Range<VirtualAddress>);

    fn next(&mut self) -> Option<Self::Item> {
        let entry_index = self.iter.next()?;

        let page_range = Range {
            start: self.page_start,
            end: self.page_start.saturating_add(self.page_size).min(self.max),
        };

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
