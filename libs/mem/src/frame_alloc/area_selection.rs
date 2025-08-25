// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::slice;
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::mem;
use core::mem::MaybeUninit;
use core::ops::Range;

use fallible_iterator::FallibleIterator;
use smallvec::SmallVec;

use crate::address_space::RawAddressSpace;
use crate::{AddressRangeExt, Frame, PhysicalAddress};

const MAX_WASTED_AREA_BYTES: usize = 0x8_4000; // 528 KiB

#[derive(Debug)]
pub struct AreaSelection {
    pub area: Range<PhysicalAddress>,
    pub bookkeeping: &'static mut [MaybeUninit<Frame>],
    pub wasted_bytes: usize,
}

#[derive(Debug)]
pub struct SelectionError {
    pub range: Range<PhysicalAddress>,
}

pub struct ArenaSelections<A: RawAddressSpace> {
    allocatable_regions: SmallVec<[Range<PhysicalAddress>; 4]>,
    wasted_bytes: usize,

    _aspace: PhantomData<A>,
}

pub fn select_areas<A: RawAddressSpace>(
    allocatable_regions: SmallVec<[Range<PhysicalAddress>; 4]>,
) -> ArenaSelections<A> {
    ArenaSelections {
        allocatable_regions,
        wasted_bytes: 0,

        _aspace: PhantomData,
    }
}

impl<A: RawAddressSpace> FallibleIterator for ArenaSelections<A> {
    type Item = AreaSelection;
    type Error = SelectionError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(mut area) = self.allocatable_regions.pop() else {
            return Ok(None);
        };

        while let Some(region) = self.allocatable_regions.pop() {
            debug_assert!(!area.is_overlapping(&region));

            let pages_in_hole = if area.end <= region.start {
                // the region is higher than the current area
                region.start.checked_sub_addr(area.end).unwrap() / A::PAGE_SIZE
            } else {
                debug_assert!(region.end <= area.start);
                // the region is lower than the current area
                area.start.checked_sub_addr(region.end).unwrap() / A::PAGE_SIZE
            };

            let waste_from_hole = size_of::<Frame>() * pages_in_hole;

            if self.wasted_bytes + waste_from_hole > MAX_WASTED_AREA_BYTES {
                self.allocatable_regions.push(region);
                break;
            } else {
                self.wasted_bytes += waste_from_hole;

                if area.end <= region.start {
                    area.end = region.end;
                } else {
                    area.start = region.start;
                }
            }
        }

        let mut aligned = area.checked_align_in(A::PAGE_SIZE).unwrap();
        // We can't use empty areas anyway
        if aligned.is_empty() {
            return Err(SelectionError { range: aligned });
        }

        let bookkeeping_size_frames = aligned.size() / A::PAGE_SIZE;

        let bookkeeping_start = aligned
            .end
            .checked_sub(bookkeeping_size_frames * size_of::<Frame>())
            .unwrap()
            .align_down(A::PAGE_SIZE);

        // The area has no space to hold its own bookkeeping
        if bookkeeping_start < aligned.start {
            return Err(SelectionError { range: aligned });
        }

        let bookkeeping = unsafe {
            slice::from_raw_parts_mut(
                bookkeeping_start.as_mut_ptr().cast(),
                bookkeeping_size_frames,
            )
        };
        aligned.end = bookkeeping_start;

        Ok(Some(AreaSelection {
            area: aligned,
            bookkeeping,
            wasted_bytes: mem::take(&mut self.wasted_bytes),
        }))
    }
}

// ===== impl SelectionError =====

impl core::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        todo!()
    }
}

impl core::error::Error for SelectionError {}
