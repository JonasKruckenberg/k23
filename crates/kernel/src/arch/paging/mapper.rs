use super::entry::PageFlags;
use super::flush::Flush;
use super::table::Table;
use crate::arch::paging::MAX_LEVEL;
use crate::arch::PAGE_SIZE;
use crate::paging::frame_alloc::FrameAllocator;
use crate::paging::{PhysicalAddress, VirtualAddress};
use crate::Error;
use riscv::register::satp;
use riscv::register::satp::Mode;

pub struct Mapper {
    root_table: PhysicalAddress,
    allocator: FrameAllocator,
}

impl Mapper {
    pub fn new(mut allocator: FrameAllocator) -> crate::Result<Self> {
        let root_table = allocator.allocate_frame()?;

        Ok(Self {
            root_table,
            allocator,
        })
    }

    pub fn from_active(allocator: FrameAllocator) -> Self {
        let root_table = unsafe { PhysicalAddress::new(satp::read().ppn() << 12) };

        Self {
            root_table,
            allocator,
        }
    }

    pub fn activate(&self) -> crate::Result<()> {
        unsafe {
            // we have to access these addresses as the table is not mapped
            // so after activating the page table, we can't access it anymore
            // TODO: this is a bit of a hack, we should probably map the table
            let start = self.root_table().lowest_mapped_address().unwrap();
            let end = self.root_table().highest_mapped_address().unwrap();

            let ppn = self.root_table().address().as_raw() >> 12;
            satp::set(Mode::Sv39, 0, ppn);

            // the most brutal approach to this, probably not necessary
            // this will take a hammer to the page tables and synchronize *everything*
            sbicall::rfence::sfence_vma(0, -1isize as usize, start.as_raw(), end.as_raw())?;
        }

        Ok(())
    }

    pub fn allocator(&self) -> &FrameAllocator {
        &self.allocator
    }

    pub fn root_table(&self) -> Table {
        Table::from_address(self.root_table, MAX_LEVEL)
    }

    pub fn map_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: PageFlags,
    ) -> crate::Result<Flush> {
        let virt = unsafe { VirtualAddress::new(phys.as_raw()) };
        self.map(virt, phys, flags, 0)
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: PageFlags,
        level: usize,
    ) -> crate::Result<Flush> {
        assert_eq!(
            phys.as_raw() % PAGE_SIZE,
            0,
            "can only map to aligned physical addresses {:#x?}",
            phys.as_raw()
        );

        // Make sure that Read, Write, or Execute have been provided
        // otherwise, we'll leak memory and always create a page fault.
        assert!(flags.intersects(PageFlags::READ | PageFlags::WRITE | PageFlags::EXECUTE));

        let mut table = self.root_table();

        for i in (level..=MAX_LEVEL).rev() {
            let entry = &mut table[virt];

            if i == level {
                entry.set_flags(flags | PageFlags::VALID);
                entry.set_address(phys);
                return Ok(Flush::new(virt));
            } else {
                if !entry.flags().contains(PageFlags::VALID) {
                    let frame = self.allocator.allocate_frame()?;
                    entry.set_flags(PageFlags::VALID);
                    entry.set_address(frame);
                }

                table = Table::from_address(entry.address(), i - 1);
            }
        }

        Err(Error::VirtualAddressTooLarge(virt))
    }

    pub fn virt_to_phys(&self, virt: VirtualAddress) -> crate::Result<PhysicalAddress> {
        let mut table = self.root_table();

        for i in (0..=MAX_LEVEL).rev() {
            let entry = &table[virt];

            if entry
                .flags()
                .intersects(PageFlags::EXECUTE | PageFlags::READ)
            {
                let addr = entry.address();
                let off_mask = (1 << 12) - 1;
                let pgoff = virt.as_raw() & off_mask;

                unsafe {
                    return Ok(PhysicalAddress::new(addr.as_raw() & !off_mask | pgoff));
                }
            } else {
                // PTE is pointer to next level page table
                assert!(
                    entry.flags().contains(PageFlags::VALID),
                    "invalid page table entry {entry:?} for virt {:#x?}",
                    virt.as_raw()
                );
                table = Table::from_address(entry.address(), i - 1);
            }
        }

        Err(Error::VirtualAddressNotMapped(virt))
    }
}
