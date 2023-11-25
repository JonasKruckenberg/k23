use super::entry::PageFlags;
use super::flush::Flush;
use super::frame_alloc::FrameAllocator;
use super::table::Table;
use super::{PhysicalAddress, VirtualAddress, MAX_LEVEL};

pub struct Mapper {
    root_table: PhysicalAddress,
    allocator: FrameAllocator,
}

impl Mapper {
    pub fn new(mut allocator: FrameAllocator) -> Self {
        let root_table = allocator.allocate_frame().unwrap();

        Self {
            root_table,
            allocator,
        }
    }

    pub fn allocator(&self) -> &FrameAllocator {
        &self.allocator
    }

    pub fn root_table(&self) -> Table {
        Table::from_address(self.root_table)
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: PageFlags,
        level: usize,
    ) -> Option<Flush> {
        // Make sure that Read, Write, or Execute have been provided
        // otherwise, we'll leak memory and always create a page fault.
        assert!(flags.intersects(PageFlags::READ | PageFlags::WRITE | PageFlags::EXECUTE));

        let mut table = self.root_table();

        for i in (level..=MAX_LEVEL).rev() {
            let entry = table.get_entry_mut(virt.vpn(i));

            if i == level {
                entry.set_flags(flags | PageFlags::VALID);
                entry.set_address(phys);
                return Some(Flush::new(virt));
            } else {
                if !entry.flags().contains(PageFlags::VALID) {
                    let frame = self.allocator.allocate_frame().unwrap();
                    entry.set_flags(PageFlags::VALID);
                    entry.set_address(frame);
                }

                table = Table::from_address(entry.address());
            }
        }

        None
    }

    pub fn virt_to_phys(&self, virt: VirtualAddress) -> Result<PhysicalAddress, VirtualAddress> {
        let mut table = self.root_table();

        for i in (0..=MAX_LEVEL).rev() {
            let entry = table.get_entry(virt.vpn(i));

            if entry
                .flags()
                .intersects(PageFlags::EXECUTE | PageFlags::READ)
            {
                // PTE is a leaf

                let addr = entry.address();
                let off_mask = (1 << 12) - 1;
                let pgoff = virt.0 & off_mask;

                return Ok(PhysicalAddress(addr.0 & !off_mask | pgoff));
            } else {
                // PTE is pointer to next level page table
                assert!(
                    entry.flags().contains(PageFlags::VALID),
                    "invalid page table entry {entry:?} for virt {:#x?}",
                    virt.0
                );
                table = Table::from_address(entry.address());
            }
        }

        Err(virt)
    }
}
