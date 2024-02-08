use crate::{AddressRange, Arch, Error, PhysicalAddress, VirtualAddress};
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::ops::Range;
use core::{fmt, mem};

#[derive(Debug)]
pub struct FrameUsage {
    pub used: usize,
    pub total: usize,
}

pub struct BumpAllocator<A> {
    regions: &'static [Range<PhysicalAddress>],
    offset: usize,
    _m: PhantomData<A>,
}

impl<A: Arch> BumpAllocator<A> {
    /// # Safety
    ///
    /// The regions list is assumed to be sorted and not overlapping
    pub unsafe fn new(regions: &'static [Range<PhysicalAddress>], offset: usize) -> Self {
        Self {
            regions,
            offset,
            _m: PhantomData,
        }
    }

    pub fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        let mut offset = self.offset;

        for region in self.regions.iter() {
            if offset < region.size_in_bytes() {
                let page_phys = region.start.add(offset);
                // TODO zero out page
                self.offset += A::PAGE_SIZE;
                return Ok(page_phys);
            }
            offset -= region.size_in_bytes();
        }

        Err(Error::OutOfMemory)
    }

    pub fn memory_usage(&self) -> FrameUsage {
        let mut total = 0;
        for region in self.regions.iter() {
            total += region.size_in_bytes() >> A::PAGE_SHIFT;
        }
        let used = self.offset >> A::PAGE_SHIFT;
        FrameUsage { used, total }
    }
}

/*
   =====================================
            Buddy allocator
   =====================================
*/

struct TableEntry<A> {
    /// The region of physical memory this table entry manages
    region: Range<PhysicalAddress>,
    /// The first free page
    skip: usize,
    /// The number of used pages
    used: usize,
    _m: PhantomData<A>,
}

impl<A: Arch> fmt::Debug for TableEntry<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let usage_map = unsafe {
            let virt = A::phys_to_virt(self.region.start);
            core::slice::from_raw_parts(virt.as_raw() as *const u8, self.pages() / 8)
        };

        f.debug_struct("TableEntry")
            .field("region", &self.region)
            .field("skip", &self.skip)
            .field("used", &self.used)
            .field("__pages", &self.pages())
            .field("__usage_map_pages", &self.usage_map_pages())
            .field("__usage_map", &usage_map)
            .finish()
    }
}

impl<A: Arch> TableEntry<A> {
    /// The number of pages this entry represents
    pub fn pages(&self) -> usize {
        self.region.size_in_bytes() >> A::PAGE_SHIFT
    }

    pub fn is_page_used(&self, page: usize) -> bool {
        let phys = self.region.start.add(page / 8);
        let bits = unsafe {
            let virt = A::phys_to_virt(phys);
            *(virt.as_raw() as *const u8)
        };
        bits & (1 << (page % 8)) != 0
    }

    pub fn mark_page_as_used(&mut self, page: usize) {
        let phys = self.region.start.add(page / 8);
        unsafe {
            let virt = A::phys_to_virt(phys);
            let bits = core::ptr::read_volatile(virt.as_raw() as *const u8);
            core::ptr::write_volatile(virt.as_raw() as *mut u8, bits | 1 << (page as u8 % 8));
        }
    }
    pub fn mark_page_as_free(&mut self, page: usize) {
        let phys = self.region.start.add(page / 8);
        unsafe {
            let virt = A::phys_to_virt(phys);
            let bits = core::ptr::read_volatile(virt.as_raw() as *const u8);
            core::ptr::write_volatile(virt.as_raw() as *mut u8, bits & !(1 << (page as u8 % 8)));
        }
    }

    /// The number of pages required to store the usage map for this entry
    pub fn usage_map_pages(&self) -> usize {
        // we can fit 8 bits into one byte
        let bytes = self.pages() / 8;

        // align-up to next page
        (bytes + (A::PAGE_SIZE - 1)) >> A::PAGE_SHIFT
    }
}

pub struct FrameAllocator<A> {
    /// The base address of the buddy allocation table
    table_virt: VirtualAddress,
    _m: PhantomData<A>,
}

impl<A: Arch> FrameAllocator<A> {
    const NUM_ENTRIES: usize = A::PAGE_SIZE / mem::size_of::<TableEntry<A>>();

    fn entries_mut(&self) -> &mut [TableEntry<A>] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.table_virt.as_raw() as *mut TableEntry<A>,
                Self::NUM_ENTRIES,
            )
        }
    }

    fn entries(&self) -> &[TableEntry<A>] {
        unsafe {
            core::slice::from_raw_parts(
                self.table_virt.as_raw() as *const TableEntry<A>,
                Self::NUM_ENTRIES,
            )
        }
    }

    pub fn new(mut bump_allocator: BumpAllocator<A>) -> crate::Result<Self> {
        // allocate a frame to hold the table
        let table_phys = bump_allocator.allocate_frame()?;
        let table_virt = unsafe { A::phys_to_virt(table_phys) };

        let mut this = Self {
            table_virt,
            _m: PhantomData,
        };

        // fill the table with the memory regions
        log::debug!("bump allocator offset {}", bump_allocator.offset);
        let mut offset = bump_allocator.offset;
        for mut region in bump_allocator.regions.iter().cloned() {
            // keep advancing past already fully used memory regions
            if offset >= region.size_in_bytes() {
                offset -= region.size_in_bytes();
                continue;
            } else if offset > 0 {
                region.start = region.start.add(offset);
                offset = 0;
            }

            for entry in this.entries_mut() {
                if entry.region.size_in_bytes() == 0 {
                    // Create new entry
                    entry.region = region.clone();
                    break;
                } else if region.end == entry.region.start {
                    // Combine entry at start
                    entry.region.start = region.start.clone();
                    break;
                } else if region.start == entry.region.end {
                    entry.region.end = region.end.clone();
                    break;
                }
            }
        }

        for entry in this.entries_mut() {
            let usage_map_pages = entry.usage_map_pages();

            for page in 0..usage_map_pages {
                entry.mark_page_as_used(page);
            }

            entry.skip = usage_map_pages;
            entry.used = usage_map_pages;
        }

        Ok(this)
    }

    pub fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        for entry in self.entries_mut() {
            for page in entry.skip..entry.pages() {
                if !entry.is_page_used(page) {
                    entry.mark_page_as_used(page);
                    entry.skip = page;
                    entry.used += 1;

                    return Ok(entry.region.start.add(page * A::PAGE_SIZE));
                }
            }
        }

        Err(Error::OutOfMemory)
    }

    pub fn allocate_frames(&mut self, requested: usize) -> crate::Result<PhysicalAddress> {
        for entry in self.entries_mut() {
            // find a consecutive run of free pages
            let mut free_page = entry.skip;
            let mut free_len = 0;

            for page in entry.skip..entry.pages() {
                if entry.is_page_used(page) {
                    free_page = page + 1;
                    free_len = 0;
                } else {
                    free_len += 1;

                    if free_len == requested {
                        for page in free_page..free_page + free_len {
                            entry.mark_page_as_used(page);
                        }

                        if entry.skip == free_page {
                            entry.skip = free_page + free_len;
                        }
                        entry.used += requested;

                        return Ok(entry.region.start.add(free_page << A::PAGE_SHIFT));
                    }
                }
            }
        }

        Err(Error::OutOfMemory)
    }

    pub fn deallocate_frame(&mut self, phys: PhysicalAddress) -> crate::Result<()> {
        self.deallocate_frames(phys, 1)
    }

    pub fn deallocate_frames(&mut self, phys: PhysicalAddress, count: usize) -> crate::Result<()> {
        for entry in self.entries_mut() {
            if phys >= entry.region.start && phys.add(count * A::PAGE_SIZE) <= entry.region.end {
                let start_page = (phys.as_raw() - entry.region.start.as_raw()) >> A::PAGE_SHIFT;

                for page in start_page..start_page + count {
                    if entry.is_page_used(page) {
                        // Update skip if necessary
                        if page < entry.skip {
                            entry.skip = page;
                        }

                        // Update used page count
                        entry.used -= 1;

                        entry.mark_page_as_free(page);
                    } else {
                        return Err(Error::DoubleFree(
                            entry.region.start.add(page << A::PAGE_SHIFT),
                        ));
                    }
                }

                return Ok(());
            }
        }

        Err(Error::DoubleFree(phys))
    }

    pub fn debug_print(&self) {
        for entry in self.entries() {
            log::debug!("{entry:?}");
        }
    }

    pub fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        let mut used = 0;
        for entry in self.entries() {
            total += entry.region.size_in_bytes() >> A::PAGE_SHIFT;
            used += entry.used;
        }

        FrameUsage { used, total }
    }
}

// #[cfg(test)]
// mod test {
//     #[test]
//     fn test() {
// let frame = frame_alloc.allocate_frames(50)?;
// log::debug!("allocated 50 frames");
// assert_eq!(frame.as_raw(), 0x8059f000);
//
// frame_alloc.deallocate_frame(frame)?;
// log::debug!("deallocated 1 frame");
// frame_alloc.debug_print();
//
// frame_alloc.deallocate_frames(frame.add(MemoryMode::PAGE_SIZE), 49)?;
// log::debug!("deallocated the other 49 frames");
// frame_alloc.debug_print();

// assert_eq!(next.as_raw(), 0x805a6000);
//
// let next = frame_alloc.allocate_frame()?;
// log::debug!("alloc2 after free");
// assert_eq!(next.as_raw(), 0x805ab000);
//
//     }
// }
