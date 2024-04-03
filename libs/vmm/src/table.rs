use crate::entry::Entry;
use crate::{Mode, PhysicalAddress, VirtualAddress};
use core::marker::PhantomData;
use core::mem;

pub struct Table<M> {
    /// The level of this table entry.
    ///
    /// Will be between `0` and `M::PAGE_TABLE_LEVELS - 1`, `M::PAGE_TABLE_LEVELS - 1` designates the root table
    /// and `0` means a leaf entry.
    level: usize,
    addr: VirtualAddress,
    _m: PhantomData<M>,
}

impl<M: Mode> Table<M> {
    /// Loads the table pointed by the given `entry`
    pub unsafe fn new(addr: VirtualAddress, level: usize) -> Table<M> {
        Self {
            level,
            addr,
            _m: PhantomData,
        }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn addr(&self) -> VirtualAddress {
        self.addr
    }

    pub fn entry_mut(&mut self, index: usize) -> &mut Entry<M> {
        debug_assert!(index < M::PAGE_TABLE_ENTRIES, "index was {}", index);
        let ptr = self.addr.add(index * mem::size_of::<Entry<M>>()).as_raw() as *mut Entry<M>;
        // log::trace!("{ptr:?} self.addr {:?} index: {index}", self.addr);
        unsafe { &mut *ptr }
    }

    pub fn entry(&self, index: usize) -> &Entry<M> {
        debug_assert!(index < M::PAGE_TABLE_ENTRIES, "index was {}", index);
        let ptr = self.addr.add(index * mem::size_of::<Entry<M>>()).as_raw() as *mut Entry<M>;
        log::trace!("{ptr:?} self.addr {:?} index: {index}", self.addr);
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

    pub fn virt_from_index(&self, index: usize) -> VirtualAddress {
        let raw = ((index & M::PAGE_ENTRY_MASK)
            << (self.level * M::PAGE_ENTRY_SHIFT + M::PAGE_SHIFT)) as isize;

        let shift = mem::size_of::<usize>() as u32 * 8 - 38;
        VirtualAddress(raw.wrapping_shl(shift).wrapping_shr(shift) as usize)
    }
}

impl<M: Mode> Table<M> {
    pub fn debug_print_table(&self) -> crate::Result<()> {
        self.debug_print_table_inner(VirtualAddress(0))
    }

    fn debug_print_table_inner(&self, acc: VirtualAddress) -> crate::Result<()> {
        let padding = match self.level {
            0 => 8,
            1 => 4,
            _ => 0,
        };

        for i in 0..M::PAGE_TABLE_ENTRIES {
            let entry = self.entry(i);
            let virt = VirtualAddress(acc.as_raw() | self.virt_from_index(i).as_raw());

            if M::entry_is_leaf(entry) {
                log::debug!(
                    "{:^padding$}{}:{i:<3} is a leaf {} => {} {:?}",
                    "",
                    self.level,
                    virt,
                    entry.get_address(),
                    entry.get_flags()
                );
            } else if !entry.is_vacant() {
                log::debug!("{:^padding$}{}:{i} is a table node", "", self.level);
                let entry_phys = entry.get_address();
                let entry_virt = M::phys_to_virt(entry_phys);
                unsafe {
                    Self::new(entry_virt, self.level - 1).debug_print_table_inner(virt)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::table::Table;
    use crate::{EmulateArch, VirtualAddress};

    #[test]
    fn index_of_virt() {
        let virt = EmulateArch::virt_from_parts(511, 510, 509, 38);
        assert_eq!(EmulateArch::virt_into_parts(virt), (511, 510, 509, 38));

        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 2) };
        assert_eq!(table.index_of_virt(virt), 511);

        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 1) };
        assert_eq!(table.index_of_virt(virt), 510);

        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 0) };
        assert_eq!(table.index_of_virt(virt), 509);
    }

    #[test]
    fn virt_from_index() {
        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 2) };
        assert_eq!(
            table.virt_from_index(511),
            EmulateArch::virt_from_parts(511, 0, 0, 0)
        );

        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 1) };
        assert_eq!(
            table.virt_from_index(7),
            EmulateArch::virt_from_parts(0, 7, 0, 0)
        );

        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 0) };
        assert_eq!(
            table.virt_from_index(89),
            EmulateArch::virt_from_parts(0, 0, 89, 0)
        );

        let table: Table<EmulateArch> = unsafe { Table::new(VirtualAddress(0), 2) };
        assert_eq!(
            table.virt_from_index(0),
            EmulateArch::virt_from_parts(0, 0, 0, 0)
        );
    }
}
