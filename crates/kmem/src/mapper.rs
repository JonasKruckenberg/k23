use crate::arch::Arch;
use crate::error::ensure;
use crate::flush::Flush;
use crate::frame_alloc::FrameAllocator;
use crate::table::{PageFlags, Table};
use crate::Error;
use crate::{PhysicalAddress, VirtualAddress};
use core::ops::Range;

pub struct Mapper<'a, A> {
    address_space: usize,
    allocator: &'a mut FrameAllocator<A>,
    root_table: PhysicalAddress,
}

impl<'a, A: Arch> Mapper<'a, A> {
    pub fn new(address_space: usize, allocator: &'a mut FrameAllocator<A>) -> crate::Result<Self> {
        let root_table = allocator.allocate_frame()?;

        let mut this = Self {
            address_space,
            allocator,
            root_table,
        };

        let flush = this.map_identity(root_table, PageFlags::READ | PageFlags::WRITE)?;
        unsafe { flush.ignore() }

        Ok(this)
    }

    pub fn address_space(&self) -> usize {
        self.address_space
    }

    pub fn root_table(&self) -> Table<A> {
        unsafe {
            Table::new(
                PhysicalAddress::new(self.root_table.as_raw()),
                A::PAGE_LEVELS - 1,
            )
        }
    }

    pub fn allocator(&self) -> &FrameAllocator<A> {
        &self.allocator
    }

    pub fn allocator_mut(&mut self) -> &mut FrameAllocator<A> {
        &mut self.allocator
    }

    pub fn identity_map_range(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: PageFlags<A>,
    ) -> crate::Result<Flush<A>> {
        let mut flush = Flush::empty(self.address_space);
        self.identity_map_range_with_flush(phys_range, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn identity_map_range_with_flush(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: PageFlags<A>,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        let virt_start = unsafe { VirtualAddress::new(phys_range.start.as_raw()) };
        let virt_end = unsafe { VirtualAddress::new(phys_range.end.as_raw()) };

        self.map_range_with_flush(virt_start..virt_end, phys_range, flags, flush)
    }

    pub fn map_range(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: PageFlags<A>,
    ) -> crate::Result<Flush<A>> {
        let mut flush = Flush::empty(self.address_space);
        self.map_range_with_flush(virt_range, phys_range, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn map_range_with_flush(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: PageFlags<A>,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        let len = virt_range.end.as_raw() - virt_range.start.as_raw();
        // make sure both ranges are the same size
        debug_assert_eq!(len, phys_range.end.as_raw() - phys_range.start.as_raw());

        for i in 0..len / A::PAGE_SIZE {
            let virt = virt_range.start.add(i * A::PAGE_SIZE);
            let phys = phys_range.start.add(i * A::PAGE_SIZE);
            self.map_with_flush(virt, phys, flags, flush)?;
        }

        Ok(())
    }

    pub fn map_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: PageFlags<A>,
    ) -> crate::Result<Flush<A>> {
        let mut flush = Flush::empty(self.address_space);
        self.map_identity_with_flush(phys, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn map_identity_with_flush(
        &mut self,
        phys: PhysicalAddress,
        flags: PageFlags<A>,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        let virt = unsafe { VirtualAddress::new(phys.as_raw()) };
        self.map_with_flush(virt, phys, flags, flush)
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: PageFlags<A>,
    ) -> crate::Result<Flush<A>> {
        let mut flush = Flush::empty(self.address_space);
        self.map_with_flush(virt, phys, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn map_with_flush(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: PageFlags<A>,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        ensure!(
            phys.as_raw() % A::PAGE_SIZE == 0,
            Error::PhysicalAddressAlignment(phys)
        );
        ensure!(
            (virt.as_raw() % A::PAGE_SIZE) == 0,
            Error::VirtualAddressAlignment(virt)
        );

        // Make sure that Read, Write, or Execute have been provided
        // otherwise, we'll leak memory and always create a page fault.
        ensure!(
            flags.intersects(PageFlags::READ | PageFlags::WRITE | PageFlags::EXECUTE),
            Error::InvalidPageFlags
        );

        let mut table = self.root_table();

        for i in (0..A::PAGE_LEVELS).rev() {
            let entry = table.entry_mut(table.index_of_virt(virt))?;

            if i == 0 {
                entry.set_flags(flags | PageFlags::VALID);
                entry.set_address(phys);
                flush.extend_range(virt..virt.add(A::PAGE_SIZE), self.address_space)?;
                return Ok(());
            } else {
                if !entry.is_valid() {
                    let frame = self.allocator.allocate_frame()?;
                    entry.set_flags(PageFlags::VALID);
                    entry.set_address(frame);

                    // TODO don't map identity
                    let flush = self.map_identity(frame, PageFlags::READ | PageFlags::WRITE)?;
                    unsafe { flush.ignore() }
                }

                table = Table::new(entry.address(), i - 1);
            }
        }

        todo!()
    }

    pub fn unmap(&mut self, virt: VirtualAddress) -> crate::Result<Flush<A>> {
        ensure!(
            virt.as_raw() % A::PAGE_SIZE == 0,
            Error::VirtualAddressAlignment(virt)
        );

        let addr = self.unmap_inner(virt, &mut self.root_table())?;

        self.allocator.deallocate_frame(addr);

        Ok(Flush::new(self.address_space, virt..virt.add(A::PAGE_SIZE)))
    }

    fn unmap_inner(
        &mut self,
        virt: VirtualAddress,
        table: &mut Table<A>,
    ) -> crate::Result<PhysicalAddress> {
        let level = table.level();
        let entry = table.entry_mut(table.index_of_virt(virt))?;

        if level == 0 {
            let address = entry.address();
            entry.clear();
            Ok(address)
        } else {
            let mut subtable = Table::new(entry.address(), level - 1);
            let res = self.unmap_inner(virt, &mut subtable)?;

            let is_still_populated = (0..512)
                .map(|j| subtable.entry(j).expect("must be within bounds"))
                .any(|e| e.is_valid());

            if !is_still_populated {
                self.allocator.deallocate_frame(subtable.address());
                entry.clear();
            }

            Ok(res)
        }
    }

    pub fn virt_to_phys(&self, virt: VirtualAddress) -> crate::Result<PhysicalAddress> {
        let mut table = self.root_table();

        for i in (0..A::PAGE_LEVELS).rev() {
            let entry = table.entry(table.index_of_virt(virt))?;

            if entry
                .flags()
                .intersects(PageFlags::EXECUTE | PageFlags::READ)
            {
                let addr = entry.address();
                let pgoff = virt.as_raw() & A::ADDR_OFFSET_MASK;

                unsafe {
                    return Ok(PhysicalAddress::new(
                        addr.as_raw() & !A::ADDR_OFFSET_MASK | pgoff,
                    ));
                }
            } else {
                // PTE is pointer to next level page table
                ensure!(
                    entry.flags().intersects(PageFlags::VALID),
                    Error::InvalidPageFlags
                );
                table = Table::new(entry.address(), i - 1);
            }
        }

        todo!()
    }
}
