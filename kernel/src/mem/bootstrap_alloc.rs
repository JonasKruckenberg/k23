// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ops::Range;
use core::{iter, ptr, slice};

use kmem::{AddressRangeExt, PhysicalAddress};

use crate::arch;

pub struct BootstrapAllocator<'a> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
}

impl<'a> BootstrapAllocator<'a> {
    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new(regions: &'a [Range<PhysicalAddress>]) -> Self {
        Self { regions, offset: 0 }
    }

    #[must_use]
    pub fn free_regions(&self) -> FreeRegions<'_> {
        FreeRegions {
            offset: self.offset,
            inner: self.regions.iter().rev().cloned(),
        }
    }

    pub fn allocate_one(&mut self) -> Option<PhysicalAddress> {
        // Safety: layout is always valid
        self.allocate_contiguous(unsafe {
            Layout::from_size_align_unchecked(arch::PAGE_SIZE, arch::PAGE_SIZE)
        })
    }

    pub fn allocate_one_zeroed(&mut self) -> Option<PhysicalAddress> {
        // Safety: layout is always valid
        self.allocate_contiguous_zeroed(unsafe {
            Layout::from_size_align_unchecked(arch::PAGE_SIZE, arch::PAGE_SIZE)
        })
    }

    pub fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let requested_size = layout.pad_to_align().size();
        assert_eq!(
            layout.align(),
            arch::PAGE_SIZE,
            "BootstrapAllocator only supports page-aligned allocations"
        );
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            // only consider regions that we haven't already exhausted
            if offset < region.len() {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region.len() - offset < requested_size {
                    tracing::warn!(
                        "Skipped memory region {region:?} since it was too small to fulfill request for {requested_size} bytes. Wasted {} bytes in the process...",
                        region.len() - offset
                    );

                    self.offset += region.len() - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.checked_sub(offset + requested_size).unwrap();
                self.offset += requested_size;

                return Some(frame);
            }

            offset -= region.len();
        }

        None
    }

    pub fn deallocate_contiguous(&mut self, _addr: PhysicalAddress, _layout: Layout) {
        unimplemented!("Bootstrap allocator can't free");
    }

    pub fn allocate_contiguous_zeroed(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let requested_size = layout.pad_to_align().size();
        let addr = self.allocate_contiguous(layout)?;

        // Safety: we just allocated the frame
        unsafe {
            ptr::write_bytes::<u8>(
                arch::KERNEL_ASPACE_RANGE
                    .start()
                    .checked_add(addr.get())
                    .unwrap()
                    .as_mut_ptr(),
                0,
                requested_size,
            );
        }
        Some(addr)
    }
}

pub struct FreeRegions<'a> {
    offset: usize,
    inner: iter::Cloned<iter::Rev<slice::Iter<'a, Range<PhysicalAddress>>>>,
}

impl Iterator for FreeRegions<'_> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut region = self.inner.next()?;
            // keep advancing past already fully used memory regions
            if self.offset >= region.len() {
                self.offset -= region.len();
                continue;
            } else if self.offset > 0 {
                region.end = region.end.checked_sub(self.offset).unwrap();
                self.offset = 0;
            }

            return Some(region);
        }
    }
}
