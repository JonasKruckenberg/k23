use crate::flush::Flush;
use crate::table::Table;
use crate::{AddressRangeExt, FrameAllocator, Mode, PhysicalAddress, VirtualAddress};
use bitflags::Flags;
use core::marker::PhantomData;
use core::ops::Range;
use core::ptr;

pub struct Mapper<'a, M> {
    asid: usize,
    root_table: VirtualAddress,
    allocator: &'a mut dyn FrameAllocator,
    _m: PhantomData<M>,
}

impl<'a, M: Mode> Mapper<'a, M> {
    pub fn new(asid: usize, allocator: &'a mut dyn FrameAllocator) -> crate::Result<Self> {
        let root_table = allocator.allocate_frame()?;
        let root_table_virt = M::phys_to_virt(root_table);
        
        Ok(Self {
            asid,
            root_table: root_table_virt,
            allocator,
            _m: PhantomData,
        })
    }

    pub fn from_active(asid: usize, allocator: &'a mut dyn FrameAllocator) -> Self {
        let root_table = M::get_active_table(asid);
        let root_table_virt = M::phys_to_virt(root_table);
        debug_assert!(root_table.0 != 0);

        Self {
            asid,
            root_table: root_table_virt,
            allocator,
            _m: PhantomData,
        }
    }
    
    pub fn shallow_clone_active(asid: usize, allocator: &'a mut dyn FrameAllocator) -> crate::Result<Self> {
        let root_table_orig = M::get_active_table(asid);
        let root_table_orig_virt = M::phys_to_virt(root_table_orig);

        let root_table = allocator.allocate_frame()?;
        let root_table_virt = M::phys_to_virt(root_table);
        
        unsafe {
            ptr::copy_nonoverlapping(root_table_orig_virt.as_raw() as *const u8, root_table_virt.as_raw() as *mut u8, M::PAGE_SIZE);
        }

        Ok(Self {
            asid,
            root_table: root_table_virt,
            allocator,
            _m: PhantomData,
        })
    }

    pub fn activate(&self) {
        M::activate_table(self.asid, self.root_table);
    }

    pub fn allocator(&self) -> &dyn FrameAllocator {
        self.allocator
    }

    pub fn allocator_mut(&mut self) -> &mut dyn FrameAllocator {
        self.allocator
    }

    pub fn root_table(&self) -> Table<M> {
        unsafe { Table::new(self.root_table, M::PAGE_TABLE_LEVELS - 1) }
    }

    pub fn identity_map_range(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.identity_map_range_with_flush(phys_range, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn identity_map_range_with_flush(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt_start = unsafe { VirtualAddress::new(phys_range.start.0) };
        let virt_end = unsafe { VirtualAddress::new(phys_range.end.0) };

        self.map_range_with_flush_inner(virt_start..virt_end, phys_range, flags, flush, false)
    }

    pub fn map_range(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_range_with_flush_inner(virt_range, phys_range, flags, &mut flush, false)?;
        Ok(flush)
    }

    pub fn map_range_with_flush(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        self.map_range_with_flush_inner(virt_range, phys_range, flags, flush, false)
    }

    pub fn map_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        level: usize,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_identity_with_flush(phys, flags, level, &mut flush)?;
        Ok(flush)
    }

    pub fn map_identity_with_flush(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        level: usize,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt = unsafe { VirtualAddress::new(phys.0) };
        self.map_with_flush(virt, phys, flags, level, flush, false)
    }

    pub fn identity_remap_range(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.identity_remap_range_with_flush(phys_range, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn identity_remap_range_with_flush(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt_start = unsafe { VirtualAddress::new(phys_range.start.0) };
        let virt_end = unsafe { VirtualAddress::new(phys_range.end.0) };

        self.map_range_with_flush_inner(virt_start..virt_end, phys_range, flags, flush, true)
    }

    pub fn remap_range(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_range_with_flush_inner(virt_range, phys_range, flags, &mut flush, true)?;
        Ok(flush)
    }

    pub fn remap_range_with_flush(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        self.map_range_with_flush_inner(virt_range, phys_range, flags, flush, true)
    }

    pub fn remap_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        level: usize,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.remap_identity_with_flush(phys, flags, level, &mut flush)?;
        Ok(flush)
    }

    pub fn remap_identity_with_flush(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        level: usize,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt = unsafe { VirtualAddress::new(phys.0) };
        self.map_with_flush(virt, phys, flags, level, flush, true)
    }

    fn map_range_with_flush_inner(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
        remap: bool,
    ) -> crate::Result<()> {
        let len = virt_range.end.0 - virt_range.start.0;
        // make sure both ranges are the same size
        debug_assert_eq!(
            len,
            phys_range.end.0 - phys_range.start.0,
            "cannot map virtual address range to physical address range of different size"
        );

        debug_assert!(
            virt_range.size() >= M::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            phys_range.size() >= M::PAGE_SIZE,
            "physical address range must span be at least one page"
        );

        let len_pages = len / M::PAGE_SIZE;
        let level = 0;

        for i in 0..len_pages {
            // log::trace!("mapping page {i} of {} at level {}", len_pages, level);
            let virt = virt_range.start.add(i * M::PAGE_SIZE);
            let phys = phys_range.start.add(i * M::PAGE_SIZE);
            self.map_with_flush(virt, phys, flags, level, flush, remap)?;
        }

        Ok(())
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        level: usize,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_with_flush(virt, phys, flags, level, &mut flush, false)?;
        Ok(flush)
    }

    pub fn map_with_flush(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        wanted_lvl: usize,
        flush: &mut Flush<M>,
        remap: bool,
    ) -> crate::Result<()> {
        debug_assert!(wanted_lvl < M::PAGE_TABLE_LEVELS);
        debug_assert!(
            phys.is_aligned(M::PAGE_SIZE),
            "physical address is not page aligned"
        );
        debug_assert!(
            virt.is_aligned(M::PAGE_SIZE),
            "virtual address is not page aligned"
        );

        let mut table = self.root_table();

        for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
            let entry = table.entry_mut(table.index_of_virt(virt));

            if lvl == wanted_lvl {
                // we reached the leaf entry
                assert_eq!(
                    entry.is_vacant(),
                    !remap,
                    "expected table entry to be vacant, to remap use  the _remap methods"
                );

                entry.set_address_and_flags(phys, flags.union(M::ENTRY_FLAG_DEFAULT_LEAF));
                flush.extend_range(self.asid, virt..virt.add(M::PAGE_SIZE))?;
                return Ok(());
            } else {
                if entry.is_vacant() {
                    // allocate a new physical frame to hold the entries children
                    let frame_phys = self.allocator.allocate_frame()?;
                    entry.set_address_and_flags(frame_phys, M::ENTRY_FLAG_DEFAULT_TABLE);
                }

                let table_phys = entry.get_address();
                let table_virt = M::phys_to_virt(table_phys);
                table = unsafe { Table::new(table_virt, table.level() - 1) };
            }
        }

        unreachable!("virtual address was too large to be mapped. This should not be possible");
    }

    pub fn unmap(&mut self, virt: VirtualAddress) -> crate::Result<Flush<M>> {
        debug_assert!(virt.0 % M::PAGE_SIZE == 0,);

        let addr = self.unmap_inner(virt, &mut self.root_table())?;

        self.allocator.deallocate_frame(addr)?;

        Ok(Flush::new(self.asid, virt..virt.add(M::PAGE_SIZE)))
    }

    fn unmap_inner(
        &mut self,
        virt: VirtualAddress,
        table: &mut Table<M>,
    ) -> crate::Result<PhysicalAddress> {
        let level = table.level();
        let entry = table.entry_mut(table.index_of_virt(virt));

        if level == 0 {
            let address = entry.get_address();
            entry.clear();
            Ok(address)
        } else {
            let table_phys = entry.get_address();
            let table_virt = M::phys_to_virt(table_phys);
            let mut subtable = unsafe { Table::new(table_virt, level - 1) };

            let res = self.unmap_inner(virt, &mut subtable)?;

            let is_still_populated = (0..512).map(|j| subtable.entry(j)).any(|e| !e.is_vacant());

            if !is_still_populated {
                let subtable_virt = subtable.addr();
                let subtable_phys = self.virt_to_phys(subtable_virt).unwrap();
                self.allocator.deallocate_frame(subtable_phys)?;
                entry.clear();
            }

            Ok(res)
        }
    }

    pub fn virt_to_phys(&self, virt: VirtualAddress) -> Option<PhysicalAddress> {
        let mut table = self.root_table();

        for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
            let entry = table.entry(table.index_of_virt(virt));

            if lvl == 0 {
                let mut phys = entry.get_address();
                // copy the offset bits from the virtual address
                phys.0 |= virt.0 & M::PAGE_OFFSET_MASK;
                return Some(phys);
            } else {
                assert!(!entry.is_vacant());

                let table_phys = entry.get_address();
                let table_virt = M::phys_to_virt(table_phys);
                table = unsafe { Table::new(table_virt, table.level() - 1) };
            }
        }

        None
    }
}
