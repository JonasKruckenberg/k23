use crate::arch;
use crate::frame_alloc::zero_pages;
use crate::{
    frame_alloc::{BumpAllocator, FrameAllocator, FrameUsage},
    Error, PhysicalAddress, VirtualAddress,
};
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{fmt, ptr};

pub struct BitMapAllocator {
    /// The base address of the allocation table
    table_virt: VirtualAddress,
    phys_offset: VirtualAddress,
}

impl BitMapAllocator {
    const NUM_ENTRIES: usize = arch::PAGE_SIZE / size_of::<TableEntry>();

    fn entries_mut(&mut self) -> &mut [TableEntry] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.table_virt.as_raw() as *mut TableEntry,
                Self::NUM_ENTRIES,
            )
        }
    }

    fn entries(&self) -> &[TableEntry] {
        unsafe {
            core::slice::from_raw_parts(
                self.table_virt.as_raw() as *const TableEntry,
                Self::NUM_ENTRIES,
            )
        }
    }

    /// Create a new bitmap allocator
    ///
    /// # Errors
    ///
    /// Returns an error if the backing table could not be allocated.
    pub fn new(
        mut bump_allocator: BumpAllocator,
        phys_offset: VirtualAddress,
    ) -> crate::Result<Self> {
        // allocate a frame to hold the table
        let table_phys = bump_allocator.allocate_one_zeroed(phys_offset)?;
        let table_virt = VirtualAddress::from_phys(table_phys, phys_offset);

        let mut this = Self {
            table_virt,
            phys_offset,
        };

        log::debug!(
            "allocator table region {:?}",
            table_virt..table_virt.add(arch::PAGE_SIZE)
        );

        log::trace!("filling table with memory regions...");
        let mut offset = bump_allocator.offset();
        for mut region in bump_allocator.regions().iter().cloned() {
            let region_size = region.end.as_raw() - region.start.as_raw();

            // keep advancing past already fully used memory regions
            if offset >= region_size {
                offset -= region_size;
                continue;
            } else if offset > 0 {
                region.end = region.end.sub(offset);
                offset = 0;
            }

            for entry in this.entries_mut() {
                if entry.region.end.0 == entry.region.start.0 {
                    // Create new entry
                    entry.region = region.clone();
                    break;
                } else if region.end == entry.region.start {
                    // Combine entry at start
                    entry.region.start = region.start;
                    break;
                } else if region.start == entry.region.end {
                    entry.region.end = region.end;
                    break;
                }
            }
        }

        log::trace!("mark entries...");
        for entry in this.entries_mut() {
            if let Some(usage_map_pages) = NonZeroUsize::new(entry.usage_map_pages()) {
                for page in 0..usage_map_pages.get() {
                    entry.mark_page_as_used(page, phys_offset);
                }

                let addr = VirtualAddress::from_phys(entry.region.start, phys_offset);
                unsafe {
                    zero_pages(addr.as_raw() as *mut u64, usage_map_pages);
                }

                entry.skip = usage_map_pages.get();
                entry.used = usage_map_pages.get();
            }
        }

        Ok(this)
    }
}

impl FrameAllocator for BitMapAllocator {
    fn allocate_contiguous(
        &mut self,
        frames: NonZeroUsize,
    ) -> crate::Result<(PhysicalAddress, NonZeroUsize)> {
        let phys_offset = self.phys_offset;
        for entry in self.entries_mut() {
            let mut free_page = entry.skip;
            let mut free_len = 0;

            for page in entry.skip..entry.pages() {
                let is_used = entry.is_page_used(page, phys_offset);
                if let Some(free_len) = NonZeroUsize::new(free_len)
                    && is_used
                {
                    for page in free_page..free_page + free_len.get() {
                        entry.mark_page_as_used(page, phys_offset);
                    }

                    if entry.skip == free_page {
                        entry.skip = free_page + free_len.get();
                    }
                    entry.used += frames.get();

                    return Ok((
                        entry.region.start.add(free_page << arch::PAGE_SHIFT),
                        free_len,
                    ));
                } else if is_used {
                    free_page = page + 1;
                    free_len = 0;
                } else {
                    free_len += 1;
                }
            }
        }

        Err(Error::OutOfMemory)
    }

    fn deallocate(&mut self, base: PhysicalAddress, frames: NonZeroUsize) -> crate::Result<()> {
        let frames = frames.get();
        let physmem_off = self.phys_offset;

        for entry in self.entries_mut() {
            if base >= entry.region.start && base.add(frames * arch::PAGE_SIZE) <= entry.region.end
            {
                let start_page = (base.0 - entry.region.start.0) >> arch::PAGE_SHIFT;

                for page in start_page..start_page + frames {
                    if entry.is_page_used(page, physmem_off) {
                        // Update skip if necessary
                        if page < entry.skip {
                            entry.skip = page;
                        }

                        // Update used page count
                        entry.used -= 1;

                        entry.mark_page_as_free(page, physmem_off);
                    } else {
                        return Err(Error::DoubleFree(
                            entry.region.start.add(page << arch::PAGE_SHIFT),
                        ));
                    }
                }

                return Ok(());
            }
        }

        Err(Error::DoubleFree(base))
    }

    fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        let mut used = 0;
        for entry in self.entries() {
            let region_size = entry.region.end.0 - entry.region.start.0;
            total += region_size >> arch::PAGE_SHIFT;
            used += entry.used;
        }

        FrameUsage { used, total }
    }
}

struct TableEntry {
    /// The region of physical memory this table entry manages
    region: Range<PhysicalAddress>,
    /// The first free page
    skip: usize,
    /// The number of used pages
    used: usize,
}

impl fmt::Debug for TableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableEntry")
            .field("region", &self.region)
            .field("skip", &self.skip)
            .field("used", &self.used)
            .finish()
    }
}

impl TableEntry {
    /// The number of pages this entry represents
    pub fn pages(&self) -> usize {
        let region_size = self.region.end.0 - self.region.start.0;
        region_size >> arch::PAGE_SHIFT
    }

    pub fn is_page_used(&self, page: usize, phys_offset: VirtualAddress) -> bool {
        let phys = self.region.start.add(page / 8);
        let bits = unsafe {
            let virt = VirtualAddress::from_phys(phys, phys_offset);
            *(virt.0 as *const u8)
        };
        bits & (1 << (page % 8)) != 0
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn mark_page_as_used(&mut self, page: usize, phys_offset: VirtualAddress) {
        let phys = self.region.start.add(page / 8);
        let virt = VirtualAddress::from_phys(phys, phys_offset);

        unsafe {
            let bits = ptr::read_volatile(virt.0 as *const u8);
            ptr::write_volatile(virt.0 as *mut u8, bits | 1 << (page as u8 % 8));
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn mark_page_as_free(&mut self, page: usize, phys_offset: VirtualAddress) {
        let phys = self.region.start.add(page / 8);
        unsafe {
            let virt = VirtualAddress::from_phys(phys, phys_offset);
            let bits = ptr::read_volatile(virt.0 as *const u8);
            ptr::write_volatile(virt.0 as *mut u8, bits & !(1 << (page as u8 % 8)));
        }
    }

    /// The number of pages required to store the usage map for this entry
    pub fn usage_map_pages(&self) -> usize {
        // we can fit 8 bits into one byte
        let bytes = self.pages() / 8;

        // align-up to next page
        (bytes + (arch::PAGE_SIZE - 1)) >> arch::PAGE_SHIFT
    }
}

// #[cfg(test)]
// mod test {
//     use crate::{
//         BitMapAllocator, BumpAllocator, EmulateMode, Error, FrameAllocator, Mode, PhysicalAddress,
//         VirtualAddress,
//     };
//
//     #[test]
//     fn single_region_single_frame() -> Result<(), Error> {
//         let bump_alloc: BumpAllocator<EmulateMode> = unsafe {
//             BumpAllocator::new(
//                 &[PhysicalAddress(0)..PhysicalAddress(4 * EmulateMode::PAGE_SIZE)],
//                 VirtualAddress::default(),
//             )
//         };
//         let mut alloc = BitMapAllocator::new(bump_alloc)?;
//
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x3000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x2000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x1000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
//         assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
//
//         Ok(())
//     }
//
//     #[test]
//     fn single_region_multi_frame() -> Result<(), Error> {
//         let bump_alloc: BumpAllocator<EmulateMode> = unsafe {
//             BumpAllocator::new(
//                 &[PhysicalAddress(0)..PhysicalAddress(4 * EmulateMode::PAGE_SIZE)],
//                 VirtualAddress::default(),
//             )
//         };
//         let mut alloc = BitMapAllocator::new(bump_alloc)?;
//
//         assert_eq!(alloc.allocate_frames(3)?, PhysicalAddress(0x1000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
//         assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
//
//         Ok(())
//     }
//
//     #[test]
//     fn multi_region_single_frame() -> Result<(), Error> {
//         let bump_alloc: BumpAllocator<EmulateMode> = unsafe {
//             BumpAllocator::new(
//                 &[
//                     PhysicalAddress(0)..PhysicalAddress(4 * EmulateMode::PAGE_SIZE),
//                     PhysicalAddress(7 * EmulateMode::PAGE_SIZE)
//                         ..PhysicalAddress(9 * EmulateMode::PAGE_SIZE),
//                 ],
//                 VirtualAddress::default(),
//             )
//         };
//         let mut alloc = BitMapAllocator::new(bump_alloc)?;
//
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x8000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x7000));
//
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x3000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x2000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x1000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
//         assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
//
//         Ok(())
//     }
//
//     #[test]
//     fn multi_region_multi_frame() -> Result<(), Error> {
//         let bump_alloc: BumpAllocator<EmulateMode> = unsafe {
//             BumpAllocator::new(
//                 &[
//                     PhysicalAddress(0)..PhysicalAddress(4 * EmulateMode::PAGE_SIZE),
//                     PhysicalAddress(7 * EmulateMode::PAGE_SIZE)
//                         ..PhysicalAddress(9 * EmulateMode::PAGE_SIZE),
//                 ],
//                 VirtualAddress::default(),
//             )
//         };
//         let mut alloc = BitMapAllocator::new(bump_alloc)?;
//
//         assert_eq!(alloc.allocate_frames(2)?, PhysicalAddress(0x7000));
//
//         assert_eq!(alloc.allocate_frames(2)?, PhysicalAddress(0x2000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x1000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
//         assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
//
//         Ok(())
//     }
//
//     #[test]
//     fn multi_region_multi_frame2() -> Result<(), Error> {
//         let bump_alloc: BumpAllocator<EmulateMode> = unsafe {
//             BumpAllocator::new(
//                 &[
//                     PhysicalAddress(0)..PhysicalAddress(4 * EmulateMode::PAGE_SIZE),
//                     PhysicalAddress(7 * EmulateMode::PAGE_SIZE)
//                         ..PhysicalAddress(9 * EmulateMode::PAGE_SIZE),
//                 ],
//                 VirtualAddress::default(),
//             )
//         };
//         let mut alloc = BitMapAllocator::new(bump_alloc)?;
//
//         assert_eq!(alloc.allocate_frames(3)?, PhysicalAddress(0x1000));
//         assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
//         assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
//
//         Ok(())
//     }
// }
