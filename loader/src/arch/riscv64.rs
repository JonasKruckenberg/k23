use crate::boot_info::BootInfo;
use crate::STACK_FILL;
use core::arch::asm;
use core::ops::{Range, RangeInclusive};
use core::ptr::{addr_of, addr_of_mut, NonNull};
use core::{hint, usize};
use vmm::{
    BumpAllocator, EntryFlags, Flush, FrameAllocator, Mapper, Mode, PhysicalAddress, VirtualAddress,
};
use crate::stack::Stack;

pub const PAGE_SIZE: usize = 4096;

#[link_section = ".bss.uninit"]
pub static BOOT_STACK: Stack = Stack::ZERO;

type VMM = vmm::Riscv64Sv39;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi")
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
        "la     sp, {stack}",       // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}",  // load the stack size
        "add    sp, sp, t0",        // add the stack size to the stack pointer
        "mv     a2, sp",

        // fill our stack area with a canary pattern
        "li          t1, {stack_fill}",
        "la          t0, {stack}",
        "100:", // fillstack
        "sw          t1, 0(t0)",
        "addi        t0, t0, 4",
        "bltu        t0, sp, 100b",

        "jal zero, {start_rust}",   // jump into Rust
        stack = sym BOOT_STACK,
        stack_size = const (Stack::GUARD_PAGES + Stack::SIZE_PAGES) * PAGE_SIZE,
        stack_fill = const Stack::FILL_PATTERN,
        start_rust = sym start,
        options(noreturn)
    )
}

#[no_mangle]
unsafe extern "C" fn start(hartid: usize, opaque: *mut u8, stack_base: PhysicalAddress) -> ! {
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

    crate::logger::init();

    let boot_stack_region = BOOT_STACK.region();
    debug_assert!(
        boot_stack_region.contains(&stack_base),
        "region {boot_stack_region:?} stack_base {stack_base:?}"
    );
    log::trace!("boot stack region {boot_stack_region:?} stack base {stack_base:?}");

    let dtb_ptr = NonNull::new(opaque).unwrap();
    let boot_info = BootInfo::from_dtb(dtb_ptr);

    // let kernel = include_bytes!(env!("K23_KERNEL_ARTIFACT"));
    //
    // log::debug!(
    //     "Kernel image {:?} {} bytes",
    //     kernel.as_ptr()..kernel.as_ptr().add(kernel.len()),
    //     kernel.len()
    // );

    init_paging(&boot_info);

    crate::main(hartid)
}

fn init_paging(boot_info: &BootInfo) {
    extern "C" {
        static __text_start: u8;
        static __text_end: u8;
        static __rodata_start: u8;
        static __rodata_end: u8;
        static __stack_start: u8;
        static __data_end: u8;
    }

    let loader_executable_region = unsafe {
        let start = PhysicalAddress::new(addr_of!(__text_start) as usize);
        let end = PhysicalAddress::new(addr_of!(__text_end) as usize);
        start..end
    };

    let loader_read_only_region = unsafe {
        let start = PhysicalAddress::new(addr_of!(__rodata_start) as usize);
        let end = PhysicalAddress::new(addr_of!(__rodata_end) as usize);
        start..end
    };

    let loader_read_write_region = unsafe {
        let start = PhysicalAddress::new(addr_of!(__stack_start) as usize).add(8 * PAGE_SIZE);
        let end = PhysicalAddress::new(addr_of!(__data_end) as usize);
        log::debug!("read-write {start:?}..{end:?}");
        start..end
    };

    pub const KIB: usize = 1024;
    pub const MIB: usize = 1024 * KIB;
    pub const GIB: usize = 1024 * MIB;

    let phys_to_virt_identity =
        |addr: PhysicalAddress| unsafe { VirtualAddress::new(addr.as_raw()) };

    // step 1: init allocator
    let mut alloc: BumpAllocator<VMM> = unsafe {
        BumpAllocator::new(
            &boot_info.memories,
            loader_read_write_region
                .end
                .sub_addr(boot_info.memories[0].start),
            phys_to_virt_identity,
        )
    };

    // let f = alloc.allocate_frames(10).unwrap();
    // assert!(f > kernel_read_write_region.end);

    // step 2: init mapper
    let mut mapper = Mapper::new(0, &mut alloc, phys_to_virt_identity).unwrap();
    let mut flush = Flush::empty(0);

    // step 4: map own text section
    mapper
        .identity_map_range_with_flush(
            loader_executable_region,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut flush,
        )
        .unwrap();

    // step 5: map own read-only section
    mapper
        .identity_map_range_with_flush(loader_read_only_region, EntryFlags::READ, &mut flush)
        .unwrap();

    // step 6: map own read-write section
    mapper
        .identity_map_range_with_flush(
            loader_read_write_region,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    let frame_usage = mapper.allocator().frame_usage();
    log::debug!(
        "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * VMM::PAGE_SIZE) / KIB,
        (frame_usage.total * VMM::PAGE_SIZE) / MIB,
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );

    mapper.activate();

    log::debug!("success");

    // let m = Mapper::from_active(0, &mut alloc, phys_to_virt_identity);
    // m.root_table().debug_print_table().unwrap()
}
