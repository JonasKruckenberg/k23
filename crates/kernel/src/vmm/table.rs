use crate::vmm::entry::Entry;
use crate::vmm::{Mode, PhysicalAddress, Riscv64Sv39, VirtualAddress};
use bitflags::Flags;
use core::marker::PhantomData;

pub struct Table<M> {
    /// The level of this table entry.
    ///
    /// Will be between `0` and `M::PAGE_TABLE_LEVELS - 1`, `M::PAGE_TABLE_LEVELS - 1` designates the root table
    /// and `0` means a leaf entry.
    pub(super) level: usize,
    pub(super) addr: PhysicalAddress,
    pub(super) phys_offset: usize,
    pub(super) _m: PhantomData<M>,
}

impl<M: Mode> Table<M> {
    /// Loads the table pointed by the given `entry`
    pub unsafe fn new(addr: PhysicalAddress, level: usize, phys_offset: usize) -> Table<M> {
        Self {
            level,
            addr,
            phys_offset,
            _m: PhantomData,
        }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn addr(&self) -> PhysicalAddress {
        self.addr
    }

    pub fn entry_mut(&mut self, index: usize) -> &mut Entry<M> {
        debug_assert!(index < M::PAGE_TABLE_ENTRIES, "index was {}", index);
        let addr = self.addr.add(self.phys_offset);
        let ptr = addr.0 as *mut Entry<M>;
        unsafe { &mut *ptr }
    }

    pub fn entry(&self, index: usize) -> &Entry<M> {
        debug_assert!(index < M::PAGE_TABLE_ENTRIES);
        let addr = self.addr.add(self.phys_offset);
        let ptr = addr.0 as *const Entry<M>;
        unsafe { &*ptr }
    }

    pub fn index_of_virt(&self, virt: VirtualAddress) -> usize {
        // A virtual address is made up of a `n`-bit page offset and `LEVELS - 1` number of `m`-bit page numbers.
        //
        // We therefore need to right-shift first by `n` bits to account for the page offset and
        // then `level * m` bits to get the correct page number and last mask out all but the `m` bits
        // of the page number we're interested in

        (virt.as_raw() >> (self.level * M::PAGE_ENTRY_SHIFT + M::PAGE_SHIFT)) & M::PAGE_ENTRY_MASK
    }

    #[cfg(debug_assertions)]
    pub fn virt_from_index(&self, index: usize) -> VirtualAddress {
        // TODO make sure addr is valid
        unsafe {
            VirtualAddress::new(
                (index & M::PAGE_ENTRY_SHIFT) << (self.level * M::PAGE_ENTRY_SHIFT + M::PAGE_SHIFT),
            )
        }
    }
}

impl Table<Riscv64Sv39> {
    #[cfg(debug_assertions)]
    pub fn debug_print_table(&self) -> crate::Result<()> {
        self.debug_print_table_inner(unsafe { VirtualAddress::new(0) })
    }

    #[cfg(debug_assertions)]
    fn debug_print_table_inner(&self, acc: VirtualAddress) -> crate::Result<()> {
        let padding = match self.level {
            0 => 8,
            1 => 4,
            _ => 0,
        };

        for i in 0..Riscv64Sv39::PAGE_TABLE_ENTRIES {
            let entry = self.entry(i);
            let virt = VirtualAddress(acc.as_raw() | self.virt_from_index(i).as_raw());

            if entry.get_flags().intersects(
                <Riscv64Sv39 as Mode>::EntryFlags::READ
                    | <Riscv64Sv39 as Mode>::EntryFlags::EXECUTE,
            ) {
                log::debug!(
                    "{:^padding$}{}:{i} is a leaf {} => {}",
                    "",
                    self.level,
                    virt,
                    entry.get_address()
                );
            } else if entry
                .get_flags()
                .intersects(<Riscv64Sv39 as Mode>::EntryFlags::VALID)
            {
                log::debug!("{:^padding$}{}:{i} is a table node", "", self.level);
                unsafe {
                    Self::new(entry.get_address(), self.level - 1, self.phys_offset)
                        .debug_print_table_inner(virt)?;
                }
            }
        }

        Ok(())
    }
}
