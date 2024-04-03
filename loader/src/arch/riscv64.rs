use crate::boot_info::BootInfo;
use crate::kconfig;
use crate::stack::Stack;
use core::arch::asm;
use core::ptr::addr_of_mut;
use vmm::PhysicalAddress;

#[link_section = ".bss.uninit"]
pub static BOOT_STACK: Stack = Stack::ZERO;

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
        "la     sp, {stack}",       // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}",  // load the stack size
        "add    sp, sp, t0",        // add the stack size to the stack pointer
        "mv     a2, sp",

        // fill our stack area with a fixed pattern
        // so that we can identify unused stack memory in dumps & calculate stack usage
        "li          t1, {stack_fill}",
        "la          t0, {stack}",
        "100:",
        "sw          t1, 0(t0)",
        "addi        t0, t0, 4",
        "bltu        t0, sp, 100b",

        "jal zero, {start_rust}",   // jump into Rust
        stack = sym BOOT_STACK,
        stack_size = const (Stack::GUARD_PAGES + Stack::SIZE_PAGES) * kconfig::PAGE_SIZE,
        stack_fill = const Stack::FILL_PATTERN,
        start_rust = sym start,
        options(noreturn)
    )
}

#[no_mangle]
unsafe extern "C" fn start(hartid: usize, opaque: *const u8, stack_base: PhysicalAddress) -> ! {
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

    let boot_info = BootInfo::from_dtb(opaque);

    log::debug!("{boot_info:?}");

    init_heap(&boot_info);

    crate::main(hartid)
}

fn init_heap(boot_info: &BootInfo) {
    extern "C" {
        static mut __data_end: u8;
    }

    let heap_bottom = unsafe { addr_of_mut!(__data_end) };
    let heap_size = boot_info.memories[0].end.sub(heap_bottom as usize).as_raw();

    #[global_allocator]
    static GLOBAL_ALLOC: linked_list_allocator::LockedHeap =
        linked_list_allocator::LockedHeap::empty();

    unsafe { GLOBAL_ALLOC.lock().init(heap_bottom, heap_size) };
}

// fn init_paging() -> BumpAllocator<kconfig::MEMORY_MODE> {
//     // Step 1: collect memory regions
//     extern "C" {
//         static __text_start: u8;
//         static __text_end: u8;
//         static __rodata_start: u8;
//         static __rodata_end: u8;
//         static __stack_start: u8;
//
//     }

//     let loader_executable_region = unsafe {
//         let start = PhysicalAddress::new(addr_of!(__text_start) as usize);
//         let end = PhysicalAddress::new(addr_of!(__text_end) as usize);
//         start..end
//     };

//     let loader_read_only_region = unsafe {
//         let start = PhysicalAddress::new(addr_of!(__rodata_start) as usize);
//         let end = PhysicalAddress::new(addr_of!(__rodata_end) as usize);
//         start..end
//     };

//     let loader_read_write_region = unsafe {
//         let start =
//             PhysicalAddress::new(addr_of!(__stack_start) as usize).add(8 * kconfig::PAGE_SIZE);
//         let end = PhysicalAddress::new(addr_of!(__data_end) as usize);
//         log::debug!("read-write {start:?}..{end:?}");
//         start..end
//     };

//     // Step 2: setup alloc
//     let mut alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> = unsafe {
//         BumpAllocator::new(
//             &boot_info.memories,
//             loader_read_write_region
//                 .end
//                 .sub_addr(boot_info.memories[0].start),
//         )
//     };

//     // Step 3: setup mapper
//     let mut mapper = Mapper::new(0, &mut alloc).unwrap();
//     let mut flush = Flush::empty(0);

//     // Step 4: map all of physical memory at PHYS_OFFSET
//     const PHYS_OFFSET: VirtualAddress = unsafe { VirtualAddress::new(0xffff_ffff_0000_0000) };
//     assert_eq!(
//         boot_info.memories.len(),
//         1,
//         "expected only one contiguous memory region"
//     );
//     let mem_phys = boot_info.memories[0].clone();
//     let mem_virt = PHYS_OFFSET.add(mem_phys.start.as_raw())..PHYS_OFFSET.add(mem_phys.end.as_raw());

//     log::debug!("Mapping physical memory {mem_virt:?}=>{mem_phys:?}...");
//     mapper
//         .map_range_with_flush(
//             mem_virt,
//             mem_phys,
//             EntryFlags::READ | EntryFlags::WRITE,
//             &mut flush,
//         )
//         .unwrap();

//     // Step 5: identity map own text region
//     log::debug!("Identity mapping own executable region {loader_executable_region:?}...");
//     mapper
//         .identity_map_range_with_flush(
//             loader_executable_region,
//             EntryFlags::READ | EntryFlags::EXECUTE,
//             &mut flush,
//         )
//         .unwrap();

//     // Step 6: identity map own read-only region
//     log::debug!("Identity mapping own read-only region {loader_read_only_region:?}...");
//     mapper
//         .identity_map_range_with_flush(loader_read_only_region, EntryFlags::READ, &mut flush)
//         .unwrap();

//     // Step 7: identity map own read-write region
//     log::debug!("Identity mapping own read-write region {loader_read_write_region:?}...");
//     mapper
//         .identity_map_range_with_flush(
//             loader_read_write_region,
//             EntryFlags::READ | EntryFlags::WRITE,
//             &mut flush,
//         )
//         .unwrap();

//     let frame_usage = mapper.allocator().frame_usage();
//     log::info!(
//         "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
//         (frame_usage.used * kconfig::PAGE_SIZE) / KIB,
//         (frame_usage.total * kconfig::PAGE_SIZE) / MIB,
//         (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
//     );

//     mapper.activate();

//     // let mut alloc = alloc.consume_init();
//     // let m: Mapper<kconfig::MEMORY_MODE> = Mapper::from_active(0, &mut alloc);
//     // m.root_table().debug_print_table().unwrap();

//     alloc.consume_init()
// }
