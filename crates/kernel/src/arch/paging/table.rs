use super::entry::{Entry, PageFlags};
use super::PhysicalAddress;
use crate::paging::VirtualAddress;
use core::{mem, ops};

pub struct Table {
    /// the physical address of this table
    /// [Entry; 512]
    phys: PhysicalAddress,
    level: usize,
}

impl Table {
    pub fn from_address(phys: PhysicalAddress, level: usize) -> Self {
        Self { phys, level }
    }

    pub fn address(&self) -> PhysicalAddress {
        self.phys
    }

    pub fn lowest_mapped_address(&self) -> Option<VirtualAddress> {
        self.lowest_mapped_address_inner(unsafe { VirtualAddress::new(0) })
    }

    pub fn lowest_mapped_address_inner(&self, acc: VirtualAddress) -> Option<VirtualAddress> {
        for i in 0..512 {
            let entry = &self[i];
            let virt =
                unsafe { VirtualAddress::new(acc.as_raw() | (i & 0x1ff) << (self.level * 9 + 12)) };

            if entry
                .flags()
                .intersects(PageFlags::READ | PageFlags::EXECUTE)
            {
                return Some(virt);
            } else if entry.flags().contains(PageFlags::VALID) {
                return Table::from_address(entry.address(), self.level - 1)
                    .lowest_mapped_address_inner(virt);
            }
        }

        None
    }

    pub fn highest_mapped_address(&self) -> Option<VirtualAddress> {
        self.highest_mapped_address_inner(unsafe { VirtualAddress::new(0) })
    }

    fn highest_mapped_address_inner(&self, acc: VirtualAddress) -> Option<VirtualAddress> {
        for i in (0..512).rev() {
            let entry = &self[i];
            let virt =
                unsafe { VirtualAddress::new(acc.as_raw() | (i & 0x1ff) << (self.level * 9 + 12)) };

            if entry
                .flags()
                .intersects(PageFlags::READ | PageFlags::EXECUTE)
            {
                return Some(virt);
            } else if entry.flags().contains(PageFlags::VALID) {
                return Table::from_address(entry.address(), self.level - 1)
                    .highest_mapped_address_inner(virt);
            }
        }

        None
    }

    fn index_of_virt(&self, virt: VirtualAddress) -> usize {
        (virt.as_raw() >> (self.level * 9 + 12)) & 0x1ff
    }

    pub fn print_table(&self) {
        let padding = match self.level {
            0 => 8,
            1 => 4,
            _ => 0,
        };

        for i in 0..512 {
            let entry = &self[i];

            if entry
                .flags()
                .intersects(PageFlags::READ | PageFlags::EXECUTE)
            {
                log::debug!("{:^padding$}{}:{i} is a leaf", "", self.level);
            } else if entry.flags().contains(PageFlags::VALID) {
                log::debug!("{:^padding$}{}:{i} is a table node", "", self.level);
                Table::from_address(entry.address(), self.level - 1).print_table();
            }
        }
    }
}

impl ops::Index<usize> for Table {
    type Output = Entry;

    fn index(&self, index: usize) -> &Self::Output {
        let ptr = self.phys.add(index * mem::size_of::<Entry>()).as_raw() as *const Entry;
        unsafe { &*(ptr) }
    }
}

impl ops::IndexMut<usize> for Table {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        let ptr = self.phys.add(index * mem::size_of::<Entry>()).as_raw() as *mut Entry;
        unsafe { &mut *(ptr) }
    }
}

impl ops::Index<VirtualAddress> for Table {
    type Output = Entry;

    fn index(&self, index: VirtualAddress) -> &Self::Output {
        self.index(self.index_of_virt(index))
    }
}

impl ops::IndexMut<VirtualAddress> for Table {
    fn index_mut(&mut self, index: VirtualAddress) -> &mut Self::Output {
        self.index_mut(self.index_of_virt(index))
    }
}
