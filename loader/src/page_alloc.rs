// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::fmt::Formatter;
use core::ops::Range;

use arrayvec::ArrayVec;
use kmem_core::{AddressRangeExt, VirtualAddress};
use rand_chacha::ChaCha20Rng;

#[derive(Debug, Copy, Clone)]
pub struct AllocError;

impl core::fmt::Display for AllocError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.write_str("virtual memory allocation failed")
    }
}

impl core::error::Error for AllocError {}

#[derive(Debug)]
pub struct PageAllocator {
    regions: ArrayVec<Range<VirtualAddress>, 256>, // should be enough for anyone!
    rng: Option<ChaCha20Rng>,
}

impl PageAllocator {
    pub fn new(rng: Option<ChaCha20Rng>) -> Self {
        Self {
            regions: ArrayVec::new(),
            rng,
        }
    }

    pub fn allocate(&mut self, layout: Layout) -> Result<Range<VirtualAddress>, AllocError> {
        assert!(layout.align().is_power_of_two());

        let gaps = Gaps {
            prev_region_end: self.max_range.start,
            max_range_end: self.max_range.end,
            regions: self.regions.iter(),
        };

        let spot =
            kmem_aslr::find_spot_for(layout, gaps, self.max_range.clone(), self.rng.as_mut())
                .ok_or(AllocError)?;

        let region = Range::from_start_len(spot, layout.size());

        self.regions.push(region.clone());

        Ok(region)
    }

    pub unsafe fn reserve(&mut self, region: Range<VirtualAddress>) {
        log::trace!("marking {region:?} as used",);
        self.regions.push(region);
    }
}

#[derive(Debug, Clone)]
pub struct Gaps<'vec> {
    prev_region_end: Option<VirtualAddress>,
    max_range_end: VirtualAddress,
    regions: core::slice::Iter<'vec, Range<VirtualAddress>>,
}

impl Iterator for Gaps<'_> {
    type Item = Range<VirtualAddress>;
    fn next(&mut self) -> Option<Self::Item> {
        let prev_region_end = self.prev_region_end.take()?;

        if let Some(region) = self.regions.next() {
            let gap = prev_region_end..region.start;

            self.prev_region_end = Some(region.end);

            Some(gap)
        } else {
            let gap = prev_region_end..self.max_range_end;

            Some(gap)
        }
    }
}
