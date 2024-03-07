use crate::arch;
use crate::boot_info::{BootInfo, BOOT_INFO};
use core::arch::asm;
use core::marker::PhantomData;
use core::mem::align_of_val;
use core::ops::Range;
use core::ptr::{addr_of, addr_of_mut, NonNull};
use uart_16550::SerialPort;
use vmm::{
    AddressRangeExt, BumpAllocator, Error, Flush, FrameAllocator, FrameUsage, Mapper, Mode,
    PhysicalAddress, VirtualAddress,
};

pub const STACK_SIZE_PAGES: usize = 16;
pub const PAGE_SIZE: usize = 4096;

pub type QEMUExit = qemu_exit::RISCV64;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!(
        ".option push",
        ".option norelax",
        "    la		gp, __global_pointer$",
        ".option pop",
        "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}", // load the stack size
        "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
        "mul    t0, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t0", // add the offset from sp to get the harts stack pointer

        // "addi sp, sp, -{trap_frame_size}",
        // "csrrw x0, sscratch, sp", // sscratch points to the trap frame

        "jal zero, {start_rust}", // jump into Rust

        stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
        // trap_frame_size = const mem::size_of::<TrapFrame>(),
        start_rust = sym start,
        options(noreturn)
    )
}

unsafe extern "C" fn start(hartid: usize, opaque: *mut u8) -> ! {
    // use `call_once` to do all global one-time initialization
    let info = BOOT_INFO.call_once(|| {
        extern "C" {
            static mut __bss_start: u64;
            static mut __bss_end: u64;
        }

        // Zero BSS section
        let mut ptr = addr_of_mut!(__bss_start);
        let end = addr_of_mut!(__bss_end);
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }

        let dtb_ptr = NonNull::new(opaque).unwrap();

        let info = BootInfo::from_dtb(dtb_ptr);

        crate::logger::init(&info);

        {
            let mut port = SerialPort::new(
                info.serial.reg.start.as_raw(),
                info.serial.clock_frequency,
                38400,
            );

            let mut v = dtb_parser::debug::DebugVisitor::new(&mut port);

            dtb_parser::DevTree::from_raw(dtb_ptr)
                .unwrap()
                .visit(&mut v)
                .unwrap();
        }

        init_paging(&info);

        // for i in 0..info.cpus {
        //     if i != hartid {
        //         sbicall::hsm::start_hart(i, _start as usize, opaque as usize).unwrap();
        //     }
        // }

        info
    });

    crate::main(hartid, info)
}

fn init_paging(boot_info: &BootInfo) {
    extern "C" {
        static __text_start: u8;
        static __text_end: u8;
        static __stack_start: u8;
        static __rodata_start: u8;
        static __rodata_end: u8;
    }

    let stack_start = unsafe { addr_of!(__stack_start) as usize };
    let text_start = unsafe { addr_of!(__text_start) as usize };
    let text_end = unsafe { addr_of!(__text_end) as usize };
    let rodata_start = unsafe { addr_of!(__rodata_start) as usize };
    let rodata_end = unsafe { addr_of!(__rodata_end) as usize };

    let stack_region = unsafe {
        let start = PhysicalAddress::new(stack_start);

        start..start.add(STACK_SIZE_PAGES * PAGE_SIZE * boot_info.cpus)
    };

    let kernel_executable_region =
        unsafe { PhysicalAddress::new(text_start)..PhysicalAddress::new(text_end) };

    let kernel_read_only_region =
        unsafe { PhysicalAddress::new(rodata_start)..PhysicalAddress::new(rodata_end) };

    let kernel_read_write_region = unsafe { PhysicalAddress::new(rodata_end)..stack_region.end };

    debug_assert!(stack_region.start < stack_region.end);
    debug_assert!(kernel_executable_region.start < kernel_executable_region.end);
    debug_assert!(kernel_read_only_region.start < kernel_read_only_region.end);
    debug_assert!(kernel_read_write_region.start < kernel_read_write_region.end);

    // Step 1: collect memory regions
    // these are all the addresses we can use for allocation
    // which should get this info from the DTB ideally
    let regions = [boot_info.memory.clone()];

    log::debug!("physical memory regions {regions:?}");

    // Step 2: initialize allocator
    let mut bump_alloc = unsafe {
        BumpAllocator::new(
            &regions,
            stack_region.end.sub_addr(boot_info.memory.start),
            |phys| VirtualAddress::new(phys.as_raw()),
        )
    };

    let frame_usage = bump_alloc.frame_usage();
    log::info!(
        "Used {}MiB of {}MiB total for kernel disk image & stack region. Available {}MiB",
        frame_usage.used * 4 / 1024,
        frame_usage.total * 4 / 1024,
        (frame_usage.total - frame_usage.used) * 4 / 1024,
    );

    // Step 4: create mapper
    let mut mapper: Mapper<vmm::Riscv64Sv39> = Mapper::new(0, &mut bump_alloc).unwrap();

    let mut flush = Flush::empty(0);

    // map all of physical memory at PHYS_OFFSET

    const PHYS_OFFSET: usize = 0xffff_ffff_0000_0000;

    let physmem_virt = unsafe {
        VirtualAddress::new(boot_info.memory.start.as_raw()).add(PHYS_OFFSET)
            ..VirtualAddress::new(boot_info.memory.end.as_raw()).add(PHYS_OFFSET)
    };
    log::trace!(
        "Mapping physical memory region {physmem_virt:?}=>{:?}...",
        boot_info.memory
    );
    mapper
        .map_range_with_flush(
            physmem_virt,
            boot_info.memory.clone(),
            vmm::EntryFlags::READ
                | vmm::EntryFlags::WRITE
                | vmm::EntryFlags::ACCESS
                | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    log::trace!("Identity mapping kernel executable region {kernel_executable_region:?}...");
    mapper
        .identity_map_range_with_flush(
            kernel_executable_region.clone(),
            vmm::EntryFlags::READ
                | vmm::EntryFlags::EXECUTE
                | vmm::EntryFlags::ACCESS
                | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    let kernel_executable_region_virt = unsafe {
        VirtualAddress::new(kernel_executable_region.start.as_raw()).add(PHYS_OFFSET)
            ..VirtualAddress::new(kernel_executable_region.end.as_raw()).add(PHYS_OFFSET)
    };
    log::trace!(
        "Remapping kernel executable region {kernel_executable_region_virt:?}=>{kernel_executable_region:?}...",
    );
    mapper
        .remap_range_with_flush(
            kernel_executable_region_virt,
            kernel_executable_region,
            vmm::EntryFlags::READ
                | vmm::EntryFlags::EXECUTE
                | vmm::EntryFlags::ACCESS
                | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    log::trace!("Identity mapping kernel read-only region {kernel_read_only_region:?}...");
    mapper
        .identity_map_range_with_flush(
            kernel_read_only_region.clone(),
            vmm::EntryFlags::READ | vmm::EntryFlags::ACCESS | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    let kernel_read_only_region_virt = unsafe {
        VirtualAddress::new(kernel_read_only_region.start.as_raw()).add(PHYS_OFFSET)
            ..VirtualAddress::new(kernel_read_only_region.end.as_raw()).add(PHYS_OFFSET)
    };
    log::trace!(
        "Remapping kernel read-only region {kernel_read_only_region_virt:?}=>{kernel_read_only_region:?}...",
    );
    mapper
        .remap_range_with_flush(
            kernel_read_only_region_virt,
            kernel_read_only_region,
            vmm::EntryFlags::READ | vmm::EntryFlags::ACCESS | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    log::trace!("Identity mapping kernel read-write region {kernel_read_write_region:?}...",);
    mapper
        .identity_map_range_with_flush(
            kernel_read_write_region,
            vmm::EntryFlags::READ
                | vmm::EntryFlags::WRITE
                | vmm::EntryFlags::ACCESS
                | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    // also map the UART MMIO device
    log::trace!(
        "Identity mapping UART MMIO region {:?}...",
        boot_info.serial.reg
    );
    mapper
        .identity_map_range_with_flush(
            boot_info.serial.reg.clone(),
            vmm::EntryFlags::READ
                | vmm::EntryFlags::WRITE
                | vmm::EntryFlags::ACCESS
                | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();

    let mut mmio_alloc = MmioAlloc::default();

    log::debug!("{mmio_alloc:?}");

    let uart_mmio_phys = boot_info.serial.reg.clone().align(PAGE_SIZE);
    let uart_mmio_virt = {
        let start = mmio_alloc.allocate_pages(uart_mmio_phys.size() / PAGE_SIZE);
        start..start.add(uart_mmio_phys.size())
    };

    log::trace!("Mapping UART MMIO region {uart_mmio_virt:?}=>{uart_mmio_phys:?}...",);

    mapper
        .map_range_with_flush(
            uart_mmio_virt,
            uart_mmio_phys,
            vmm::EntryFlags::READ
                | vmm::EntryFlags::WRITE
                | vmm::EntryFlags::ACCESS
                | vmm::EntryFlags::DIRTY,
            &mut flush,
        )
        .unwrap();
    
    let frame_usage = mapper.allocator().frame_usage();
    log::info!(
        "Physical memory after mapping: Used {}MiB of {}MiB, available {}MiB",
        frame_usage.used * 4 / 1024,
        frame_usage.total * 4 / 1024,
        (frame_usage.total - frame_usage.used) * 4 / 1024
    );
}

#[derive(Debug)]
pub struct MmioAlloc {
    region: Range<VirtualAddress>,
    offset: usize,
}

impl Default for MmioAlloc {
    fn default() -> Self {
        const KIB: usize = 1024;
        const MIB: usize = 1024 * KIB;
        const GIB: usize = 1024 * MIB;

        let mmio_start = unsafe { VirtualAddress::new(0xffff_ffc8_0000_0000) };

        Self {
            region: mmio_start..mmio_start.add(64 * GIB),
            offset: 0,
        }
    }
}

impl MmioAlloc {
    pub fn new(region: Range<VirtualAddress>) -> Self {
        Self { region, offset: 0 }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn region(&self) -> &Range<VirtualAddress> {
        &self.region
    }

    fn allocate_pages(&mut self, num_frames: usize) -> VirtualAddress {
        let mut offset = self.offset + num_frames * PAGE_SIZE;

        let region_size = self.region.end.sub_addr(self.region.start);

        if offset < region_size {
            let page_virt = self.region.start.add(offset);
            self.offset += num_frames * PAGE_SIZE;
            return page_virt;
        }
        offset -= region_size;

        panic!("oom")
    }
}
