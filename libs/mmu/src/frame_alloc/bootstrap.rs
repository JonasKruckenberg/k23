// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::PAGE_SIZE;
use crate::frame_alloc::FrameAllocator;
use crate::{arch, AddressRangeExt, PhysicalAddress, VirtualAddress};
use core::alloc::Layout;
use core::range::Range;
use core::{cmp, iter, ptr, slice};

#[derive(Debug, Default)]
pub struct FrameUsage {
    pub used: usize,
    pub total: usize,
}

pub struct BootstrapAllocator<'a> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
    phys_offset: VirtualAddress,
}

impl<'a> BootstrapAllocator<'a> {
    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new(regions: &'a [Range<PhysicalAddress>]) -> Self {
        Self {
            regions,
            offset: 0,
            phys_offset: VirtualAddress::default(),
        }
    }

    pub fn set_phys_offset(&mut self, phys_offset: VirtualAddress) {
        self.phys_offset = phys_offset;
    }

    #[must_use]
    pub fn free_regions(&self) -> FreeRegions<'_> {
        FreeRegions {
            offset: self.offset,
            inner: self.regions.iter().rev().cloned(),
        }
    }

    #[must_use]
    pub fn used_regions(&self) -> UsedRegions<'_> {
        UsedRegions {
            offset: self.offset,
            inner: self.regions.iter().rev().cloned(),
        }
    }

    pub fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        for region in self.regions {
            let region_size = region.size();
            total += region_size >> arch::PAGE_SHIFT;
        }
        let used = self.offset >> arch::PAGE_SHIFT;
        FrameUsage { used, total }
    }
}

impl FrameAllocator for BootstrapAllocator<'_> {
    fn allocate_one(&mut self) -> Option<PhysicalAddress> {
        self.allocate_contiguous(unsafe { Layout::from_size_align_unchecked(PAGE_SIZE, PAGE_SIZE) })
    }

    fn allocate_one_zeroed(&mut self) -> Option<PhysicalAddress> {
        self.allocate_contiguous_zeroed(unsafe {
            Layout::from_size_align_unchecked(PAGE_SIZE, PAGE_SIZE)
        })
    }

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let requested_size = layout.pad_to_align().size();
        assert_eq!(
            layout.align(),
            arch::PAGE_SIZE,
            "BootstrapAllocator only supports page-aligned allocations"
        );
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            // only consider regions that we haven't already exhausted
            if offset < region.size() {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region.size() - offset < requested_size {
                    log::warn!("Skipped memory region {region:?} since it was fulfill request for {requested_size} bytes. Wasted {} bytes in the process...", region.size() - offset);

                    self.offset += region.size() - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.checked_sub(offset + requested_size).unwrap();
                self.offset += requested_size;

                return Some(frame);
            }

            offset -= region.size();
        }

        None
    }

    fn deallocate_contiguous(&mut self, _addr: PhysicalAddress, _layout: Layout) {
        unimplemented!("Bootstrap allocator can't free");
    }

    fn allocate_contiguous_zeroed(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let requested_size = layout.pad_to_align().size();
        let addr = self.allocate_contiguous(layout)?;
        unsafe {
            ptr::write_bytes::<u8>(
                self.phys_offset
                    .checked_add(addr.get())
                    .unwrap()
                    .as_mut_ptr(),
                0,
                requested_size,
            )
        }
        Some(addr)
    }

    fn allocate_partial(&mut self, layout: Layout) -> Option<(PhysicalAddress, usize)> {
        let requested_size = layout.pad_to_align().size();
        assert_eq!(
            layout.align(),
            arch::PAGE_SIZE,
            "BootstrapAllocator only supports page-aligned allocations"
        );
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            // only consider regions that we haven't already exhausted
            if offset < region.size() {
                let alloc_size = cmp::min(requested_size, region.size() - offset);

                let frame = region.end.checked_sub(offset + alloc_size).unwrap();
                self.offset += alloc_size;

                return Some((frame, alloc_size));
            }

            offset -= region.size();
        }

        None
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
            if self.offset >= region.size() {
                self.offset -= region.size();
                continue;
            } else if self.offset > 0 {
                region.end = region.end.checked_sub(self.offset).unwrap();
                self.offset = 0;
            }

            return Some(region);
        }
    }
}

pub struct UsedRegions<'a> {
    offset: usize,
    inner: iter::Cloned<iter::Rev<slice::Iter<'a, Range<PhysicalAddress>>>>,
}

impl Iterator for UsedRegions<'_> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.inner.next()?;

        if self.offset >= region.size() {
            Some(region)
        } else if self.offset > 0 {
            region.start = region.end.checked_sub(self.offset).unwrap();
            self.offset = 0;

            Some(region)
        } else {
            None
        }
    }
}
