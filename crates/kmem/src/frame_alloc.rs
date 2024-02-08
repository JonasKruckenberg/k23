use crate::{AddressRange, Arch, Error, PhysicalAddress, VirtualAddress};
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::ops::Range;
use core::{fmt, mem};

struct MemoryUsage {
    used: usize,
    total: usize,
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
    pub unsafe fn new(regions: &'static [Range<PhysicalAddress>]) -> Self {
        Self {
            regions,
            offset: 0,
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

    pub fn memory_usage(&self) -> MemoryUsage {
        let mut total = 0;
        for region in self.regions.iter() {
            total += region.size_in_bytes() >> A::PAGE_SHIFT;
        }
        let used = self.offset >> A::PAGE_SHIFT;
        MemoryUsage { used, total }
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
            core::slice::from_raw_parts(
                virt.as_raw() as *const u8,
                self.usage_map_pages() * A::PAGE_SIZE,
            )
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

    /// Set the usage number for a specific page
    pub fn set_usage_for_page(&mut self, page: usize, usage: u8) {
        let phys = self.region.start.add(page * mem::size_of::<u8>());
        unsafe {
            let virt = A::phys_to_virt(phys);
            core::ptr::write(virt.as_raw() as *mut u8, usage);

            let v = core::ptr::read(virt.as_raw() as *const u8);
            assert!(matches!(v, 0 | 1));
        };
    }

    pub fn get_usage_for_page(&self, page: usize) -> u8 {
        let phys = self.region.start.add(page * mem::size_of::<u8>());
        unsafe {
            let virt = A::phys_to_virt(phys);
            core::ptr::read(virt.as_raw() as *const u8)
        }
    }

    /// The number of pages required to store the usage map for this entry
    pub fn usage_map_pages(&self) -> usize {
        let bytes = self.pages() * mem::size_of::<u8>();
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

            for i in 0..Self::NUM_ENTRIES {
                let entry = this.entry_mut(i);

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

        for i in 0..Self::NUM_ENTRIES {
            let entry = this.entry_mut(i);

            let usage_map_pages = entry.usage_map_pages();

            for page in 0..usage_map_pages {
                entry.set_usage_for_page(page, 1);
            }

            entry.skip = usage_map_pages;
            entry.used = usage_map_pages;
        }

        Ok(this)
    }

    fn entry(&self, index: usize) -> &TableEntry<A> {
        let virt = self.table_virt.add(index * mem::size_of::<TableEntry<A>>());
        unsafe { &*(virt.as_raw() as *const TableEntry<A>) }
    }

    fn entry_mut(&mut self, index: usize) -> &mut TableEntry<A> {
        let virt = self.table_virt.add(index * mem::size_of::<TableEntry<A>>());
        unsafe { &mut *(virt.as_raw() as *mut TableEntry<A>) }
    }

    pub fn debug_print(&self) {
        for i in 0..Self::NUM_ENTRIES {
            let entry = self.entry(i);
            log::debug!("{entry:?}");
        }
    }

    pub fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        for i in 0..Self::NUM_ENTRIES {
            let entry = self.entry_mut(i);

            for page in entry.skip..entry.pages() {
                let usage = entry.get_usage_for_page(page);
                if usage == 0 {
                    entry.set_usage_for_page(page, 1);
                    entry.skip = page;
                    entry.used += 1;

                    return Ok(entry.region.start.add(page * A::PAGE_SIZE));
                }
            }
        }

        Err(Error::OutOfMemory)
    }

    pub fn allocate_frames(&mut self, requested: usize) -> crate::Result<PhysicalAddress> {
        for i in 0..Self::NUM_ENTRIES {
            let entry = self.entry_mut(i);

            // find a consecutive run of free pages
            let mut free_page = entry.skip;
            let mut free_len = 0;

            for page in entry.skip..entry.pages() {
                let usage = entry.get_usage_for_page(page);

                if usage > 0 {
                    free_page = page + 1;
                    free_len = 0;
                } else {
                    free_len += 1;

                    if free_len == requested {
                        for page in free_page..free_page + free_len {
                            let usage = entry.get_usage_for_page(page);
                            entry.set_usage_for_page(page, usage + 1);
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
        for i in 0..Self::NUM_ENTRIES {
            let entry = self.entry_mut(i);

            if entry.region.contains(&phys) {
                let page = (phys.as_raw() - entry.region.start.as_raw()) >> A::PAGE_SHIFT;

                let mut usage = entry.get_usage_for_page(page);

                if usage > 0 {
                    usage -= 1;
                } else {
                    return Err(Error::DoubleFree(
                        entry.region.start.add(page << A::PAGE_SHIFT),
                    ));
                }

                // if page was freed
                if usage == 0 {
                    // Update skip if necessary
                    if page < entry.skip {
                        entry.skip = page;
                    }

                    // Update used page count
                    entry.used -= 1;
                }

                entry.set_usage_for_page(page, usage);

                return Ok(());
            }
        }

        todo!()
    }

    pub fn deallocate_frames(&mut self, phys: PhysicalAddress, count: usize) -> crate::Result<()> {
        for i in 0..Self::NUM_ENTRIES {
            let entry = self.entry_mut(i);

            if phys >= entry.region.start && phys.add(count * A::PAGE_SIZE) <= entry.region.end {
                let start_page = (phys.as_raw() - entry.region.start.as_raw()) >> A::PAGE_SHIFT;

                for page in start_page..start_page + count {
                    let mut usage = entry.get_usage_for_page(page);

                    if usage > 0 {
                        usage -= 1;
                    } else {
                        return Err(Error::DoubleFree(
                            entry.region.start.add(page << A::PAGE_SHIFT),
                        ));
                    }

                    // if page was freed
                    if usage == 0 {
                        // Update skip if necessary
                        if page < entry.skip {
                            entry.skip = page;
                        }

                        // Update used page count
                        entry.used -= 1;
                    }

                    entry.set_usage_for_page(page, usage);
                }

                return Ok(());
            }
        }

        todo!()
    }
}
