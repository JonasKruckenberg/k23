use crate::alloc::{BumpAllocator, FrameAllocator, FrameUsage};
use crate::{
    phys_to_virt, zero_frames, AddressRangeExt, Error, Mode, PhysicalAddress, VirtualAddress,
};
use core::fmt;
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::ops::Range;

struct TableEntry<M> {
    /// The region of physical memory this table entry manages
    region: Range<PhysicalAddress>,
    /// The first free page
    skip: usize,
    /// The number of used pages
    used: usize,
    _m: PhantomData<M>,
}

impl<M> fmt::Debug for TableEntry<M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableEntry")
            .field("region", &self.region)
            .field("skip", &self.skip)
            .field("used", &self.used)
            .finish()
    }
}

impl<M: Mode> TableEntry<M> {
    /// The number of pages this entry represents
    pub fn pages(&self) -> usize {
        let region_size = self.region.end.0 - self.region.start.0;
        region_size >> M::PAGE_SHIFT
    }

    pub fn is_page_used(&self, page: usize, physmem_off: VirtualAddress) -> bool {
        let phys = self.region.start.add(page / 8);
        let bits = unsafe {
            let virt = phys_to_virt(physmem_off, phys);
            *(virt.0 as *const u8)
        };
        bits & (1 << (page % 8)) != 0
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn mark_page_as_used(&mut self, page: usize, physmem_off: VirtualAddress) {
        let phys = self.region.start.add(page / 8);
        let virt = phys_to_virt(physmem_off, phys);

        unsafe {
            let bits = core::ptr::read_volatile(virt.0 as *const u8);
            core::ptr::write_volatile(virt.0 as *mut u8, bits | 1 << (page as u8 % 8));
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn mark_page_as_free(&mut self, page: usize, physmem_off: VirtualAddress) {
        let phys = self.region.start.add(page / 8);
        unsafe {
            let virt = phys_to_virt(physmem_off, phys);
            let bits = core::ptr::read_volatile(virt.0 as *const u8);
            core::ptr::write_volatile(virt.0 as *mut u8, bits & !(1 << (page as u8 % 8)));
        }
    }

    /// The number of pages required to store the usage map for this entry
    pub fn usage_map_pages(&self) -> usize {
        // we can fit 8 bits into one byte
        let bytes = self.pages() / 8;

        // align-up to next page
        (bytes + (M::PAGE_SIZE - 1)) >> M::PAGE_SHIFT
    }
}

pub struct BitMapAllocator<A> {
    /// The base address of the allocation table
    table_virt: VirtualAddress,
    physmem_off: VirtualAddress,
    _m: PhantomData<A>,
}

impl<M: Mode> BitMapAllocator<M> {
    const NUM_ENTRIES: usize = M::PAGE_SIZE / size_of::<TableEntry<M>>();

    fn entries_mut(&mut self) -> &mut [TableEntry<M>] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.table_virt.as_raw() as *mut TableEntry<M>,
                Self::NUM_ENTRIES,
            )
        }
    }

    fn entries(&self) -> &[TableEntry<M>] {
        unsafe {
            core::slice::from_raw_parts(
                self.table_virt.as_raw() as *const TableEntry<M>,
                Self::NUM_ENTRIES,
            )
        }
    }

    /// Create a new bitmap allocator
    ///
    /// # Errors
    ///
    /// Returns an error if the backing table could not be allocated.
    pub fn new(mut bump_allocator: BumpAllocator<M>) -> crate::Result<Self> {
        let physmem_off = bump_allocator.physmem_off;

        // allocate a frame to hold the table
        let table_phys = bump_allocator.allocate_frame_zeroed()?;
        let table_virt = phys_to_virt(physmem_off, table_phys);

        let mut this = Self {
            table_virt,
            physmem_off,
            _m: PhantomData,
        };

        log::debug!(
            "allocator table region {:?}",
            table_virt..table_virt.add(M::PAGE_SIZE)
        );

        log::trace!("filling table with memory regions...");
        let mut offset = bump_allocator.offset();
        for mut region in bump_allocator.regions().iter().cloned() {
            let region_size = region.size();

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
            let usage_map_pages = entry.usage_map_pages();

            for page in 0..usage_map_pages {
                entry.mark_page_as_used(page, physmem_off);
            }

            if usage_map_pages > 0 {
                let addr = phys_to_virt(physmem_off, entry.region.start);
                zero_frames::<M>(addr.as_raw() as *mut u64, usage_map_pages);
            }

            entry.skip = usage_map_pages;
            entry.used = usage_map_pages;
        }

        Ok(this)
    }

    pub fn debug_print_table(&self) {
        for entry in self.entries() {
            log::debug!("{entry:?}");
        }
    }
}

impl<M: Mode> FrameAllocator<M> for BitMapAllocator<M> {
    fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        let physical_memory_offset = self.physmem_off;

        for entry in self.entries_mut() {
            for page in entry.skip..entry.pages() {
                if !entry.is_page_used(page, physical_memory_offset) {
                    entry.mark_page_as_used(page, physical_memory_offset);
                    entry.skip = page;
                    entry.used += 1;

                    return Ok(entry.region.start.add(page * M::PAGE_SIZE));
                }
            }
        }

        Err(Error::OutOfMemory)
    }

    fn allocate_frames(&mut self, num_frames: usize) -> crate::Result<PhysicalAddress> {
        let physmem_off = self.physmem_off;

        for entry in self.entries_mut() {
            // find a consecutive run of free pages
            let mut free_page = entry.skip;
            let mut free_len = 0;

            for page in entry.skip..entry.pages() {
                if entry.is_page_used(page, physmem_off) {
                    free_page = page + 1;
                    free_len = 0;
                } else {
                    free_len += 1;

                    if free_len == num_frames {
                        for page in free_page..free_page + free_len {
                            entry.mark_page_as_used(page, physmem_off);
                        }

                        if entry.skip == free_page {
                            entry.skip = free_page + free_len;
                        }
                        entry.used += num_frames;

                        return Ok(entry.region.start.add(free_page << M::PAGE_SHIFT));
                    }
                }
            }
        }

        Err(Error::OutOfMemory)
    }

    fn deallocate_frames(&mut self, base: PhysicalAddress, num_frames: usize) -> crate::Result<()> {
        let physmem_off = self.physmem_off;

        for entry in self.entries_mut() {
            if base >= entry.region.start && base.add(num_frames * M::PAGE_SIZE) <= entry.region.end
            {
                let start_page = (base.0 - entry.region.start.0) >> M::PAGE_SHIFT;

                for page in start_page..start_page + num_frames {
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
                            entry.region.start.add(page << M::PAGE_SHIFT),
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
            total += region_size >> M::PAGE_SHIFT;
            used += entry.used;
        }

        FrameUsage { used, total }
    }

    fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        self.physmem_off.add(phys.0)
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
