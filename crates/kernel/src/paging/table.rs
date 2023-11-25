use super::entry::{Entry, PageFlags};
use super::PhysicalAddress;
use core::mem;

pub struct Table {
    /// the physical address of this table
    /// [Entry; 512]
    phys: PhysicalAddress,
}

impl Table {
    pub fn from_address(phys: PhysicalAddress) -> Self {
        Self { phys }
    }

    pub fn address(&self) -> PhysicalAddress {
        self.phys
    }

    pub fn get_entry(&self, vpn: usize) -> &Entry {
        let ptr = self.phys.add(vpn * mem::size_of::<Entry>()).0 as *const Entry;
        unsafe { &*(ptr) }
    }

    pub fn get_entry_mut(&mut self, vpn: usize) -> &mut Entry {
        let ptr = self.phys.add(vpn * mem::size_of::<Entry>()).0 as *mut Entry;
        unsafe { &mut *(ptr) }
    }

    pub fn print_table(&self, level: usize) {
        let padding = match level {
            0 => 8,
            1 => 4,
            _ => 0,
        };

        for i in 0..512 {
            let entry = self.get_entry(i);

            if entry
                .flags()
                .intersects(PageFlags::READ | PageFlags::EXECUTE)
            {
                log::debug!("{:^padding$}{level}:{i} is a leaf", "");
            } else if entry.flags().contains(PageFlags::VALID) {
                log::debug!("{:^padding$}{level}:{i} is a table node", "",);
                Table::from_address(entry.address()).print_table(level - 1);
            }
        }
    }
}
