// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ops::Range;

use arrayvec::ArrayVec;
use lock_api::Mutex;

use crate::frame_alloc::{AllocError, FrameAllocator};
use crate::{AddressRangeExt, PhysicalAddress};

pub const DEFAULT_MAX_REGIONS: usize = 16;

pub struct BootstrapAllocator<R: lock_api::RawMutex, const MAX_REGIONS: usize = DEFAULT_MAX_REGIONS>(
    Mutex<R, BootstrapAllocatorInner<MAX_REGIONS>>,
);

struct BootstrapAllocatorInner<const MAX_REGIONS: usize> {
    regions: ArrayVec<Range<PhysicalAddress>, MAX_REGIONS>,
    // offset from the top of memory regions
    offset: usize,
    page_size: usize,
}

impl<R: lock_api::RawMutex, const MAX_REGIONS: usize> BootstrapAllocator<R, MAX_REGIONS> {
    pub fn new(
        mut regions: ArrayVec<Range<PhysicalAddress>, MAX_REGIONS>,
        page_size: usize,
    ) -> Self {
        if regions.len() > 1 {
            regions.sort_unstable_by_key(|region| region.start);

            let mut iter = regions.as_mut_slice().windows(2);

            while let Some([cur, next]) = iter.next() {
                assert!(
                    !cur.overlaps(next),
                    "regions {cur:#x?} and {next:#x?} overlap"
                );
            }
        }

        regions
            .iter_mut()
            .for_each(|region| *region = region.clone().align_in(page_size));

        Self(Mutex::new(BootstrapAllocatorInner {
            regions,
            offset: 0,
            page_size,
        }))
    }

    pub fn regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        self.0.lock().regions.clone().into_iter()
    }

    pub fn free_regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        let inner = self.0.lock();

        FreeRegions {
            offset: inner.offset,
            inner: inner.regions.clone().into_iter(),
        }
    }

    pub fn used_regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        let inner = self.0.lock();

        UsedRegions {
            offset: inner.offset,
            inner: inner.regions.clone().into_iter(),
        }
    }

    pub fn usage(&self) -> usize {
        self.0.lock().offset
    }
}

// Safety: bootstrap allocator manages raw physical memory regions, they remain valid theoretically
// forever we merely hand out "land claims" to it.
unsafe impl<R: lock_api::RawMutex, const MAX_REGIONS: usize> FrameAllocator
    for BootstrapAllocator<R, MAX_REGIONS>
{
    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        self.0.lock().allocate(layout)
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {
        unimplemented!()
    }
}

impl<const MAX_REGIONS: usize> BootstrapAllocatorInner<MAX_REGIONS> {
    fn allocate(&mut self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let requested_size = layout.pad_to_align().size();
        assert_eq!(
            layout.align(),
            self.page_size,
            "BootstrapAllocator only supports page-aligned allocations"
        );
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.len();

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region_size - offset < requested_size {
                    log::warn!(
                        "Skipped memory region {region:?} since it was too small to fulfill request for {requested_size} bytes. Wasted {} bytes in the process...",
                        region_size - offset
                    );

                    self.offset += region_size - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.sub(offset + requested_size);
                self.offset += requested_size;
                return Ok(frame);
            }

            offset -= region_size;
        }

        Err(AllocError)
    }
}

struct FreeRegions<const MAX_REGIONS: usize> {
    offset: usize,
    inner: arrayvec::IntoIter<Range<PhysicalAddress>, { MAX_REGIONS }>,
}

impl<const MAX_REGIONS: usize> Iterator for FreeRegions<MAX_REGIONS> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut region = self.inner.next()?;
            // keep advancing past already fully used memory regions
            let region_size = region.len();

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

struct UsedRegions<const MAX_REGIONS: usize> {
    offset: usize,
    inner: arrayvec::IntoIter<Range<PhysicalAddress>, { MAX_REGIONS }>,
}

impl<const MAX_REGIONS: usize> Iterator for UsedRegions<MAX_REGIONS> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.inner.next()?;

        if self.offset >= region.len() {
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

#[cfg(test)]
mod tests {
    use riscv::satp;

    use super::*;
    use crate::Arch;
    use crate::arch::emulate::{EmulateArch, MachineBuilder};
    use crate::arch::riscv64::{RISCV64_SV39, Riscv64};
    use crate::frame_alloc::FrameAllocator;
    use crate::test_utils::setup_aspace_and_alloc;

    #[test]
    fn basically_works() {
        let machine = MachineBuilder::new()
            .with_memory_mode(const { &RISCV64_SV39 })
            .with_memory_regions([0x3000])
            .with_cpus(1)
            .finish();
        let arch: EmulateArch<Riscv64, parking_lot::RawMutex> = EmulateArch::new(machine);

        let page_size = arch.memory_mode().page_size();

        let alloc: BootstrapAllocator<parking_lot::RawMutex> =
            BootstrapAllocator::new(arch.machine().memory_regions(), page_size);

        alloc
            .allocate_contiguous(Layout::from_size_align(4096, page_size).unwrap())
            .unwrap();

        alloc
            .allocate_contiguous(Layout::from_size_align(2 * 4096, page_size).unwrap())
            .unwrap();

        alloc
            .allocate_contiguous(Layout::from_size_align(2 * 4096, page_size).unwrap())
            .unwrap_err();
    }

    #[test_log::test]
    fn zeroed() {
        let (aspace, frame_alloc) =
            setup_aspace_and_alloc(Riscv64::new(1, satp::Mode::Sv39), [0x4000]);

        log::trace!("{:?}", aspace.arch().machine());

        let _frame = frame_alloc
            .allocate_contiguous_zeroed(aspace.arch().memory_mode().page_layout(), aspace.arch())
            .unwrap();
    }

    // TODO add more tests
    //  - tests for zeroed methods
    //  - tests for multiple regions
    //  - tests for weirdly sized or invalid regions
}
