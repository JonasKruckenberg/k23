use crate::frame_alloc::{FrameAllocator, FrameUsage};
use crate::{arch, PhysicalAddress, VirtualAddress};
use core::alloc::Layout;
use core::ops::Range;
use core::{cmp, iter, ptr, slice};

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
}

impl FrameAllocator for BootstrapAllocator<'_> {
    fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let requested_size = layout.pad_to_align().size();
        assert_eq!(
            layout.align(),
            arch::PAGE_SIZE,
            "BootstrapAllocator only supports page-aligned allocations"
        );
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.end.sub_addr(region.start);

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region_size - offset < requested_size {
                    log::warn!("Skipped memory region {region:?} since it was fulfill request for {requested_size} bytes. Wasted {} bytes in the process...", region_size - offset);

                    self.offset += region_size - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.sub(offset + requested_size);
                self.offset += requested_size;

                return Some(frame);
            }

            offset -= region_size;
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
            ptr::write_bytes(
                self.phys_offset.add(addr.as_raw()).as_raw() as *mut u8,
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
            let region_size = region.end.sub_addr(region.start);

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                let alloc_size = cmp::min(requested_size, region_size - offset);

                let frame = region.end.sub(offset + alloc_size);
                self.offset += alloc_size;

                return Some((frame, alloc_size));
            }

            offset -= region_size;
        }

        None
    }

    fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        for region in self.regions {
            let region_size = region.end.0 - region.start.0;
            total += region_size >> arch::PAGE_SHIFT;
        }
        let used = self.offset >> arch::PAGE_SHIFT;
        FrameUsage { used, total }
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
            let region_size = region.end.as_raw() - region.start.as_raw();
            // keep advancing past already fully used memory regions
            if self.offset >= region_size {
                self.offset -= region_size;
                continue;
            } else if self.offset > 0 {
                region.end = region.end.sub(self.offset);
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
        let region_size = region.end.as_raw() - region.start.as_raw();

        if self.offset >= region_size {
            Some(region)
        } else if self.offset > 0 {
            region.start = region.end.sub(self.offset);
            self.offset = 0;

            Some(region)
        } else {
            None
        }
    }
}
