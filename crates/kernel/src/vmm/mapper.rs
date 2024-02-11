use crate::vmm::flush::Flush;
use crate::vmm::table::Table;
use crate::vmm::{FrameAllocator, Mode, PhysicalAddress, VirtualAddress};
use bitflags::Flags;
use core::marker::PhantomData;
use core::ops::Range;

pub struct Mapper<'a, M> {
    asid: usize,
    root_table: PhysicalAddress,
    allocator: &'a mut dyn FrameAllocator<M>,
    /// Optional offset to apply to table addresses before reading them
    phys_offset: usize,
}

impl<'a, M: Mode> Mapper<'a, M> {
    pub fn new(asid: usize, allocator: &'a mut dyn FrameAllocator<M>) -> crate::Result<Self> {
        let root_table = allocator.allocate_frame()?;

        Ok(Self {
            asid,
            root_table,
            allocator,
            phys_offset: 0,
        })
    }

    pub fn from_active(asid: usize, allocator: &'a mut dyn FrameAllocator<M>) -> Self {
        let root_table = M::get_active_table(asid);
        debug_assert!(root_table.0 != 0);

        Self {
            asid,
            root_table,
            allocator,
            phys_offset: M::PHYS_OFFSET,
        }
    }

    pub fn activate(self) {
        M::activate_table(self.asid, self.root_table)
    }

    pub fn allocator(&self) -> &dyn FrameAllocator<M> {
        self.allocator
    }

    pub fn allocator_mut(&mut self) -> &mut dyn FrameAllocator<M> {
        self.allocator
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

        self.map_range_with_flush(virt_start..virt_end, phys_range, flags, flush)
    }

    pub fn map_range(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_range_with_flush(virt_range, phys_range, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn map_range_with_flush(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let len = virt_range.end.0 - virt_range.start.0;
        // make sure both ranges are the same size
        debug_assert_eq!(len, phys_range.end.0 - phys_range.start.0);

        for i in 0..len / M::PAGE_SIZE {
            let virt = virt_range.start.add(i * M::PAGE_SIZE);
            let phys = phys_range.start.add(i * M::PAGE_SIZE);
            self.map_with_flush(virt, phys, flags, flush)?;
        }

        Ok(())
    }

    pub fn map_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_identity_with_flush(phys, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn map_identity_with_flush(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt = unsafe { VirtualAddress::new(phys.0) };
        self.map_with_flush(virt, phys, flags, flush)
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
    ) -> crate::Result<Flush<M>> {
        let mut flush = Flush::empty(self.asid);
        self.map_with_flush(virt, phys, flags, &mut flush)?;
        Ok(flush)
    }

    pub fn map_with_flush(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        debug_assert!(phys.0 % M::PAGE_SIZE == 0,);
        debug_assert!((virt.0 % M::PAGE_SIZE) == 0,);

        let mut table = self.root_table();

        for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
            let entry = table.entry_mut(table.index_of_virt(virt));

            if lvl == 0 {
                // we reached the leaf entry
                entry.set_address_and_flags(phys, flags.union(M::ENTRY_FLAG_DEFAULT_LEAF));
                flush.extend_range(virt..virt.add(M::PAGE_SIZE), self.asid)?;
                return Ok(());
            } else {
                if entry.is_vacant() {
                    // allocate a new physical frame to hold the entries children
                    let frame_phys = self.allocator.allocate_frame()?;
                    entry.set_address_and_flags(frame_phys, M::ENTRY_FLAG_DEFAULT_TABLE);
                }

                table =
                    unsafe { Table::new(entry.get_address(), table.level - 1, table.phys_offset) };
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
        let phys_offset = table.phys_offset;
        let entry = table.entry_mut(table.index_of_virt(virt));

        if level == 0 {
            let address = entry.get_address();
            entry.clear();
            Ok(address)
        } else {
            let mut subtable = unsafe { Table::new(entry.get_address(), level - 1, phys_offset) };

            let res = self.unmap_inner(virt, &mut subtable)?;

            let is_still_populated = (0..512).map(|j| subtable.entry(j)).any(|e| !e.is_vacant());

            if !is_still_populated {
                self.allocator.deallocate_frame(subtable.addr())?;
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

                table =
                    unsafe { Table::new(entry.get_address(), table.level - 1, table.phys_offset) };
            }
        }

        None
    }

    pub fn root_table(&self) -> Table<M> {
        Table {
            level: M::PAGE_TABLE_LEVELS - 1,
            addr: self.root_table,
            phys_offset: self.phys_offset,
            _m: PhantomData,
        }
    }
}
