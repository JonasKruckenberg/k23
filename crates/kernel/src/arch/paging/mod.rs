use crate::arch;
use crate::arch::paging::entry::PageFlags;
use crate::arch::paging::mapper::Mapper;
use crate::board_info::BoardInfo;
use crate::paging::frame_alloc::FrameAllocator;
use crate::paging::PhysicalAddress;
use core::ops::Range;
use core::ptr::addr_of;
use riscv::register::satp;
use riscv::register::satp::Mode;

mod entry;
mod flush;
mod mapper;
mod table;

pub const MAX_LEVEL: usize = 2; // Sv39

extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __stack_start: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
}

/// Initialize virtual memory management
///
/// This will set up the page table, identity map the kernel and stack, and enable paging.
///
/// TODO lift this out of the arch module
pub fn init(board_info: &BoardInfo) -> crate::Result<()> {
    let stack_start = unsafe { addr_of!(__stack_start) as usize };
    let text_start = unsafe { addr_of!(__text_start) as usize };
    let text_end = unsafe { addr_of!(__text_end) as usize };
    let rodata_start = unsafe { addr_of!(__rodata_start) as usize };
    let rodata_end = unsafe { addr_of!(__rodata_end) as usize };

    let stack_region = unsafe {
        let start = PhysicalAddress::new(stack_start);

        start..start.add(arch::STACK_SIZE_PAGES * arch::PAGE_SIZE * board_info.cpus)
    };
    let kernel_region =
        unsafe { PhysicalAddress::new(text_start)..PhysicalAddress::new(stack_start) };
    let text_region = unsafe { PhysicalAddress::new(text_start)..PhysicalAddress::new(text_end) };
    let rodata_region =
        unsafe { PhysicalAddress::new(rodata_start)..PhysicalAddress::new(rodata_end) };

    // Step 1: collect memory regions
    // these are all the addresses we can use for allocation
    // which should get this info from the DTB ideally
    let regions = [stack_region.end..board_info.memory.end];

    // Step 2: initialize allocator
    let allocator = unsafe { FrameAllocator::new(&regions) };

    // Step 4: create mapper
    let mut mapper = Mapper::new(allocator)?;

    // helper function to identity map a region
    let mut identity_map_range = |region: Range<PhysicalAddress>| -> crate::Result<()> {
        let len = region.end.as_raw() - region.start.as_raw();

        for i in 0..len / arch::PAGE_SIZE {
            let phys = region.start.add(i * arch::PAGE_SIZE);

            let flags = if text_region.contains(&phys) {
                PageFlags::READ | PageFlags::EXECUTE
            } else if rodata_region.contains(&phys) {
                PageFlags::READ
            } else {
                PageFlags::READ | PageFlags::WRITE
            };

            let flush = mapper.map_identity(phys, flags)?;

            unsafe {
                flush.ignore(); // no flushing during init
            }
        }

        Ok(())
    };

    // Step 4: map kernel
    log::debug!("mapping kernel region: {:?}", kernel_region);
    identity_map_range(kernel_region.clone())?;

    // Step 5: map stack
    log::debug!("mapping stack region: {:?}", stack_region);
    identity_map_range(stack_region.clone())?;

    // Step 6: map MMIO regions (UART)
    log::debug!("mapping mmio region: {:?}", board_info.serial.mmio_regs);
    identity_map_range(align_range(board_info.serial.mmio_regs.clone()))?;

    mapper.root_table().print_table();

    // Step 7: enable paging
    log::debug!("enabling paging... {:?}", mapper.root_table().address());
    mapper.activate()?;
    log::debug!("paging enabled");

    Ok(())
}

fn align_range(range: Range<PhysicalAddress>) -> Range<PhysicalAddress> {
    let start = range.start.as_raw() & !(arch::PAGE_SIZE - 1);
    let end = (range.end.as_raw() + arch::PAGE_SIZE - 1) & !(arch::PAGE_SIZE - 1);

    unsafe { PhysicalAddress::new(start)..PhysicalAddress::new(end) }
}
