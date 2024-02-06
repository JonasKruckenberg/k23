use crate::arch::riscv64::paging::entry::PageFlags;
use crate::arch::riscv64::paging::flush::Flush;
use crate::arch::riscv64::paging::table::Table;
use crate::board_info::BoardInfo;
use crate::paging::{PhysicalAddress, VirtualAddress};
use crate::{arch, Error};
use core::ops::Range;
use core::ptr::addr_of;
use frame::FrameAllocator;
use riscv::register::satp;
use riscv::register::satp::Mode;

mod entry;
mod flush;
mod frame;
mod table;

const MAX_ADDR: VirtualAddress = unsafe { VirtualAddress::from_raw_parts(511, 511, 511, 0) };
const HEAP_SIZE_PAGES: usize = (64 * MIB) / arch::PAGE_SIZE;
const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
const GIB: usize = 1024 * MIB;
const MAX_LEVEL: usize = 2; // Sv39

struct Mapper {
    root_table: PhysicalAddress,
    allocator: FrameAllocator,
}

impl Mapper {
    pub fn new(mut allocator: FrameAllocator) -> crate::Result<Self> {
        let mut this = Self {
            root_table: allocator.allocate_frame()?,
            allocator,
        };

        let flush = this.map_identity(this.root_table, PageFlags::READ | PageFlags::WRITE)?;
        unsafe { flush.ignore() }

        Ok(this)
    }

    fn root_table(&self) -> Table {
        unsafe { Table::from_address(PhysicalAddress::new(self.root_table.as_raw()), MAX_LEVEL) }
    }

    pub fn map_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: PageFlags,
    ) -> crate::Result<Flush> {
        let virt = unsafe { VirtualAddress::new(phys.as_raw()) };
        self.map(virt, phys, flags)
    }

    pub fn identity_map_range(
        &mut self,
        phys_range: Range<PhysicalAddress>,
        flags: PageFlags,
    ) -> crate::Result<Flush> {
        let virt_start = unsafe { VirtualAddress::new(phys_range.start.as_raw()) };
        let virt_end = unsafe { VirtualAddress::new(phys_range.end.as_raw()) };

        self.map_range(virt_start..virt_end, phys_range, flags)
    }

    pub fn map_range(
        &mut self,
        virt_range: Range<VirtualAddress>,
        phys_range: Range<PhysicalAddress>,
        flags: PageFlags,
    ) -> crate::Result<Flush> {
        let len = virt_range.end.as_raw() - virt_range.start.as_raw();
        // make sure both ranges are the same size
        debug_assert_eq!(len, phys_range.end.as_raw() - phys_range.start.as_raw());

        for i in 0..len / arch::PAGE_SIZE {
            let virt = virt_range.start.add(i * arch::PAGE_SIZE);
            let phys = phys_range.start.add(i * arch::PAGE_SIZE);
            let flush = self.map(virt, phys, flags)?;
            unsafe {
                flush.ignore(); // we produce a flush for the joint range at the end
            }
        }

        Ok(Flush::new(virt_range))
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: PageFlags,
    ) -> crate::Result<Flush> {
        const LEVEL: usize = 0;

        assert_eq!(
            virt.as_raw() % arch::PAGE_SIZE,
            0,
            "can only map to aligned virtual addresses {:#x?}",
            phys.as_raw()
        );

        assert_eq!(
            phys.as_raw() % arch::PAGE_SIZE,
            0,
            "can only map to aligned physical addresses {:#x?}",
            phys.as_raw()
        );

        // Make sure that Read, Write, or Execute have been provided
        // otherwise, we'll leak memory and always create a page fault.
        assert!(flags.intersects(PageFlags::READ | PageFlags::WRITE | PageFlags::EXECUTE));

        let mut table = self.root_table();

        for i in (LEVEL..=MAX_LEVEL).rev() {
            let entry = &mut table[virt];

            if i == LEVEL {
                entry.set_flags(flags | PageFlags::VALID);
                entry.set_address(phys);
                return Ok(Flush::new(virt..virt));
            } else {
                if !entry.flags().contains(PageFlags::VALID) {
                    let frame = self.allocator.allocate_frame()?;
                    entry.set_flags(PageFlags::VALID);
                    entry.set_address(frame);

                    let flush = self.map_identity(frame, PageFlags::READ | PageFlags::WRITE)?;
                    unsafe { flush.ignore() }
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

extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __stack_start: u8;
    static __rodata_start: u8;
    static __eh_frame_end: u8;
}

pub fn init(board_info: &BoardInfo) -> crate::Result<()> {
    let stack_start = unsafe { addr_of!(__stack_start) as usize };
    let text_start = unsafe { addr_of!(__text_start) as usize };
    let text_end = unsafe { addr_of!(__text_end) as usize };
    let rodata_start = unsafe { addr_of!(__rodata_start) as usize };
    let eh_frame_end = unsafe { addr_of!(__eh_frame_end) as usize };

    let stack_region = unsafe {
        let start = PhysicalAddress::new(stack_start);

        start..start.add(arch::STACK_SIZE_PAGES * arch::PAGE_SIZE * board_info.cpus)
    };

    let kernel_executable_region =
        unsafe { PhysicalAddress::new(text_start)..PhysicalAddress::new(text_end) };

    let kernel_read_only_region =
        unsafe { PhysicalAddress::new(rodata_start)..PhysicalAddress::new(eh_frame_end) };

    let kernel_read_write_region =
        unsafe { PhysicalAddress::new(text_end)..PhysicalAddress::new(stack_start) };

    // Step 1: collect memory regions
    // these are all the addresses we can use for allocation
    // which should get this info from the DTB ideally
    let regions = [stack_region.end..board_info.memory.end];

    log::debug!("physical memory regions {regions:?}");

    // Step 2: initialize allocator
    let allocator = unsafe { FrameAllocator::new(&regions) };

    // Step 4: create mapper
    let mut mapper = Mapper::new(allocator)?;

    log::debug!(
        "Identity mapping kernel executable region {}..{}",
        kernel_executable_region.start,
        kernel_executable_region.end
    );
    let flush = mapper.identity_map_range(
        kernel_executable_region,
        PageFlags::READ | PageFlags::EXECUTE,
    )?;
    unsafe { flush.ignore() }

    log::debug!(
        "Identity mapping kernel read only region {}..{}",
        kernel_read_only_region.start,
        kernel_read_only_region.end
    );
    let flush = mapper.identity_map_range(kernel_read_only_region, PageFlags::READ)?;
    unsafe { flush.ignore() }

    log::debug!(
        "Identity mapping kernel read-write section {}..{}",
        kernel_read_write_region.start,
        kernel_read_write_region.end
    );
    let flush =
        mapper.identity_map_range(kernel_read_write_region, PageFlags::READ | PageFlags::WRITE)?;
    unsafe { flush.ignore() }

    log::debug!(
        "Identity mapping kernel stack region {}..{}",
        stack_region.start,
        stack_region.end
    );
    let flush =
        mapper.identity_map_range(stack_region.clone(), PageFlags::READ | PageFlags::WRITE)?;
    unsafe { flush.ignore() }

    let mmio_range = align_range(board_info.serial.mmio_regs.clone());
    log::debug!(
        "Identity mapping mmio region {}..{}",
        mmio_range.start,
        mmio_range.end
    );
    let flush = mapper.identity_map_range(mmio_range, PageFlags::READ | PageFlags::WRITE)?;
    unsafe { flush.ignore() }

    // let heap_base = unsafe { VirtualAddress::from_raw_parts(255, 511, 511, 0) };
    let heap_base = MAX_ADDR.sub(2 * GIB);

    log::debug!(
        "Mapping kernel heap {}..{}",
        heap_base,
        heap_base.add(64 * MIB)
    );

    for i in 0..HEAP_SIZE_PAGES {
        let virt = heap_base.add(i * arch::PAGE_SIZE);
        let frame = mapper.allocator.allocate_frame()?;

        let flush = mapper.map(virt, frame, PageFlags::READ | PageFlags::WRITE)?;
        unsafe { flush.ignore() }
    }

    unsafe {
        let ppn = mapper.root_table().address().as_raw() >> 12;
        satp::set(Mode::Sv39, 0, ppn);

        let start = mapper.root_table().lowest_mapped_address().unwrap();
        let end = mapper.root_table().highest_mapped_address().unwrap();

        log::debug!("flushing table for range {start}..{end}");

        // the most brutal approach to this, probably not necessary
        // this will take a hammer to the page tables and synchronize *everything*
        sbicall::rfence::sfence_vma(0, -1isize as usize, start.as_raw(), end.as_raw())?;
    }

    log::debug!("initializing kernel heap allocator");
    unsafe {
        crate::allocator::ALLOCATOR.init(heap_base, HEAP_SIZE_PAGES * arch::PAGE_SIZE);
    }

    Ok(())
}

fn align_range(range: Range<PhysicalAddress>) -> Range<PhysicalAddress> {
    let start = range.start.as_raw() & !(arch::PAGE_SIZE - 1);
    let end = (range.end.as_raw() + arch::PAGE_SIZE - 1) & !(arch::PAGE_SIZE - 1);

    unsafe { PhysicalAddress::new(start)..PhysicalAddress::new(end) }
}
