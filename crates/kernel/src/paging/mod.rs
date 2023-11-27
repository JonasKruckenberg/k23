use crate::board_info::BoardInfo;
use crate::paging::entry::PageFlags;
use crate::paging::frame_alloc::FrameAllocator;
use crate::paging::mapper::Mapper;
use crate::start::STACK_SIZE_PAGES;
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;
use core::ptr::addr_of;
use riscv::register::satp;
use riscv::register::satp::Mode;
// use spin::Once;

mod entry;
mod flush;
mod frame_alloc;
mod mapper;
mod table;

pub const PAGE_SIZE: usize = 4096;
pub const MAX_LEVEL: usize = 2; // Sv39

/// A physical address.
#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub unsafe fn new(addr: usize) -> Self {
        Self(addr)
    }
    pub fn add(&self, offset: usize) -> Self {
        Self(self.0 + offset)
    }
    pub fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

/// A virtual address.
#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub fn vpn(&self, level: usize) -> usize {
        (self.0 >> (level * 9 + 12)) & 0x1ff
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

// static MAPPER: Once<Mapper> = Once::new();

extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __stack_start: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
}

pub fn init(board_info: &BoardInfo) {
    let stack_start = unsafe { addr_of!(__stack_start) as usize };
    let text_start = unsafe { addr_of!(__text_start) as usize };
    let text_end = unsafe { addr_of!(__text_end) as usize };
    let rodata_start = unsafe { addr_of!(__rodata_start) as usize };
    let rodata_end = unsafe { addr_of!(__rodata_end) as usize };

    let stack_region = unsafe {
        let start = PhysicalAddress::new(stack_start);

        start..start.add(STACK_SIZE_PAGES * PAGE_SIZE * board_info.cpus)
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
    let mut mapper = Mapper::new(allocator);

    let mut identity_map = |region: Range<PhysicalAddress>| {
        let len = region.end.0 - region.start.0;

        for i in 0..len / PAGE_SIZE {
            let phys = region.start.add(i * PAGE_SIZE);
            let virt = VirtualAddress(phys.0);

            let flags = if text_region.contains(&phys) {
                PageFlags::READ | PageFlags::EXECUTE
            } else if rodata_region.contains(&phys) {
                PageFlags::READ
            } else {
                PageFlags::READ | PageFlags::WRITE
            };

            let flush = mapper.map(virt, phys, flags, 0).unwrap();
            unsafe {
                flush.ignore(); // no flushing during init
            }
        }
    };

    // Step 4: map kernel
    log::debug!("mapping kernel region: {:?}", kernel_region);
    identity_map(kernel_region.clone());
    // Step 5: map stack
    log::debug!("mapping stack region: {:?}", stack_region);
    identity_map(stack_region.clone());
    // Step 6: map MMIO regions (UART)
    log::debug!("mapping mmio region: {:?}", board_info.serial.mmio_regs);
    identity_map(align_range(board_info.serial.mmio_regs.clone()));

    mapper.root_table().print_table(2);

    let addr = PhysicalAddress(0x10000005);
    let virt = VirtualAddress(addr.0);
    let phys = mapper.virt_to_phys(virt).unwrap();
    assert_eq!(phys, addr);

    let addr = stack_region.start.add(400);
    let virt = VirtualAddress(addr.0);
    let phys = mapper.virt_to_phys(virt).unwrap();
    assert_eq!(phys, addr);

    // Step 7: enable paging
    log::debug!("enabling paging... {:?}", mapper.root_table().address());
    unsafe {
        let ppn = mapper.root_table().address().as_raw() >> 12;
        satp::set(Mode::Sv39, 0, ppn);
        // the most brutal approach to this, probably not necessary
        // this will take a hammer to the page tables and synchronize *everything*
        sbicall::rfence::sfence_vma(0, -1isize as usize, 0, board_info.memory.end.as_raw())
            .unwrap();
    }
    log::debug!("paging enabled");

    // Step 7: set global mapper
    // MAPPER.call_once(|| mapper);
}

fn align_range(range: Range<PhysicalAddress>) -> Range<PhysicalAddress> {
    let start = range.start.0 & !(PAGE_SIZE - 1);
    let end = (range.end.0 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    PhysicalAddress(start)..PhysicalAddress(end)
}
