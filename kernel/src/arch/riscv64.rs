use crate::boot_info::{BootInfo, BOOT_INFO};
use core::arch::asm;
use core::ptr::{addr_of_mut, NonNull};
use uart_16550::SerialPort;
// use crate::arch;
// use core::marker::PhantomData;
// use core::mem::align_of_val;
// use core::ops::Range;
// use riscv::register::satp;
// use vmm::{
//     AddressRangeExt, BumpAllocator, EntryFlags, Error, Flush, FrameAllocator, FrameUsage, Mapper,
//     Mode, PhysicalAddress, VirtualAddress,
// };

pub const STACK_SIZE_PAGES: usize = 16;
pub const PAGE_SIZE: usize = 4096;

// const PHYS_OFFSET: usize = 0xffff_ffff_0000_0000;
// const KIB: usize = 1024;
// const MIB: usize = 1024 * KIB;
// const GIB: usize = 1024 * MIB;

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

        // init_paging(&info);

        // for i in 0..info.cpus {
        //     if i != hartid {
        //         sbicall::hsm::start_hart(i, _start as usize, opaque as usize).unwrap();
        //     }
        // }

        info
    });

    crate::kmain(hartid, info);

    // unsafe {
    //     relocate(hartid, &info, PHYS_OFFSET);
    // }
}

// fn init_paging(boot_info: &BootInfo) {
//     extern "C" {
//         static __text_start: u8;
//         static __text_end: u8;
//         static __stack_start: u8;
//         static __rodata_start: u8;
//         static __rodata_end: u8;
//     }
//
//     let stack_start = unsafe { addr_of!(__stack_start) as usize };
//     let text_start = unsafe { addr_of!(__text_start) as usize };
//     let text_end = unsafe { addr_of!(__text_end) as usize };
//     let rodata_start = unsafe { addr_of!(__rodata_start) as usize };
//     let rodata_end = unsafe { addr_of!(__rodata_end) as usize };
//
//     let stack_region = unsafe {
//         let start = PhysicalAddress::new(stack_start);
//
//         start..start.add(STACK_SIZE_PAGES * PAGE_SIZE * boot_info.cpus)
//     };
//
//     let kernel_executable_region =
//         unsafe { PhysicalAddress::new(text_start)..PhysicalAddress::new(text_end) };
//
//     let kernel_read_only_region =
//         unsafe { PhysicalAddress::new(rodata_start)..PhysicalAddress::new(rodata_end) };
//
//     let kernel_read_write_region = unsafe { PhysicalAddress::new(rodata_end)..stack_region.end };
//
//     debug_assert!(stack_region.start < stack_region.end);
//     debug_assert!(kernel_executable_region.start < kernel_executable_region.end);
//     debug_assert!(kernel_read_only_region.start < kernel_read_only_region.end);
//     debug_assert!(kernel_read_write_region.start < kernel_read_write_region.end);
//
//     // Step 1: collect memory regions
//     // these are all the addresses we can use for allocation
//     // which should get this info from the DTB ideally
//     let regions = [boot_info.memory.clone()];
//
//     log::debug!("physical memory regions {regions:?}");
//
//     // Step 2: initialize allocator
//     let mut bump_alloc = unsafe {
//         BumpAllocator::new(
//             &regions,
//             stack_region.end.sub_addr(boot_info.memory.start),
//             |phys| VirtualAddress::new(phys.as_raw()),
//         )
//     };
//
//     // Step 4: create mapper
//     let mut mapper: Mapper<vmm::Riscv64Sv39> = Mapper::new(0, &mut bump_alloc).unwrap();
//
//     let mut flush = Flush::empty(0);
//
//     // Identity mapping all of physcial memory
//     let physmem_virt = unsafe {
//         let range = boot_info.memory.clone().add(PHYS_OFFSET);
//
//         VirtualAddress::new(range.start.as_raw())..VirtualAddress::new(range.end.as_raw())
//     };
//     mapper
//         .map_range_with_flush(
//             physmem_virt,
//             boot_info.memory.clone(),
//             EntryFlags::READ | EntryFlags::WRITE,
//             &mut flush,
//         )
//         .unwrap();
//
//     let mut map_kernel_region = |region_phys: Range<PhysicalAddress>, flags: EntryFlags| {
//         log::trace!("Identity mapping kernel region {region_phys:?}...");
//         mapper
//             .identity_map_range_with_flush(region_phys.clone(), flags, &mut flush)
//             .unwrap();
//
//         let region_virt = unsafe {
//             let range = region_phys.clone().add(PHYS_OFFSET);
//
//             VirtualAddress::new(range.start.as_raw())..VirtualAddress::new(range.end.as_raw())
//         };
//
//         log::trace!("Mapping kernel region {region_virt:?}=>{region_phys:?}...");
//         mapper
//             .remap_range_with_flush(region_virt, region_phys, flags, &mut flush)
//             .unwrap();
//     };
//
//     map_kernel_region(
//         kernel_executable_region,
//         EntryFlags::READ | EntryFlags::EXECUTE,
//     );
//     map_kernel_region(kernel_read_only_region, EntryFlags::READ);
//     map_kernel_region(
//         kernel_read_write_region,
//         EntryFlags::READ | EntryFlags::WRITE,
//     );
//
//     let mut mmio_alloc = MmioAlloc::default();
//
//     let mut map_mmio_region = |region_phys: Range<PhysicalAddress>| {
//         log::trace!("Identity mapping MMIO region {region_phys:?}");
//         mapper
//             .identity_map_range_with_flush(
//                 region_phys.clone(),
//                 EntryFlags::READ | EntryFlags::WRITE,
//                 &mut flush,
//             )
//             .unwrap();
//
//         let region_virt = {
//             let start = mmio_alloc.allocate_pages(region_phys.size() / vmm::Riscv64Sv39::PAGE_SIZE);
//             start..start.add(region_phys.size())
//         };
//
//         log::trace!("Mapping MMIO region {region_virt:?}=>{region_phys:?}");
//         mapper
//             .map_range_with_flush(
//                 region_virt.clone(),
//                 region_phys,
//                 EntryFlags::READ | EntryFlags::WRITE,
//                 &mut flush,
//             )
//             .unwrap();
//
//         region_virt
//     };
//
//     let _uart_mmio_virt = map_mmio_region(boot_info.serial.reg.clone().align(PAGE_SIZE));
//
//     // Map DTB
//     let fdt_phys = unsafe {
//         let start = PhysicalAddress::new(boot_info.dtb.as_ptr() as usize);
//         start..start.add(boot_info.dtb.len())
//     };
//
//     let fdt_virt = {
//         let start = mmio_alloc.allocate_pages(boot_info.dtb.len() / vmm::Riscv64Sv39::PAGE_SIZE);
//         start..start.add(boot_info.dtb.len())
//     };
//
//     log::trace!("Mapping DTB region {fdt_virt:?}=>{fdt_phys:?}");
//     mapper
//         .map_range_with_flush(
//             fdt_virt,
//             fdt_phys,
//             EntryFlags::READ | EntryFlags::WRITE,
//             &mut flush,
//         )
//         .unwrap();
//
//     // we don't need to flush since it's the first time we activate the table
//     unsafe { flush.ignore() };
//
//     log::debug!("activating page table...");
//     mapper.activate();
//
//     let frame_usage = bump_alloc.frame_usage();
//     log::info!(
//         "Kernel mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
//         (frame_usage.used * vmm::Riscv64Sv39::PAGE_SIZE) / KIB,
//         (frame_usage.total * vmm::Riscv64Sv39::PAGE_SIZE) / MIB,
//         (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
//     );
//
//     // TODO relocate kernel
//     // TODO Unmap identity mapped regions
// }

// #[naked]
// unsafe fn relocate(hartid: usize, boot_info: &'static BootInfo, phys_offset: usize) -> ! {
//     asm!(
//         "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
//         "add    sp, sp, a2", // shift the stack pointer up by `phys_offset`
//         "li     t0, {stack_size}", // load the stack size
//         "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
//         "mul    t0, t0, t1", // multiply the stack size by the hart id to get the offset
//         "add    sp, sp, t0", // add the offset from sp to get the harts stack pointer,
//
//         "la     t0, kmain", // load kmain address
//         "add    t0, t0, a2", // shift the start address up by `phys_offset`
//         "jalr   t0",
//         stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
//         options(noreturn)
//     )
// }

// #[derive(Debug)]
// pub struct MmioAlloc {
//     region: Range<VirtualAddress>,
//     offset: usize,
// }
//
// impl Default for MmioAlloc {
//     fn default() -> Self {
//         let mmio_start = unsafe { VirtualAddress::new(0xffff_ffc8_0000_0000) };
//
//         Self {
//             region: mmio_start..mmio_start.add(64 * GIB),
//             offset: 0,
//         }
//     }
// }
//
// impl MmioAlloc {
//     pub fn new(region: Range<VirtualAddress>) -> Self {
//         Self { region, offset: 0 }
//     }
//
//     pub fn offset(&self) -> usize {
//         self.offset
//     }
//
//     pub fn region(&self) -> &Range<VirtualAddress> {
//         &self.region
//     }
//
//     fn allocate_pages(&mut self, num_pages: usize) -> VirtualAddress {
//         let mut offset = self.offset + num_pages * PAGE_SIZE;
//
//         let region_size = self.region.end.sub_addr(self.region.start);
//
//         if offset < region_size {
//             let page_virt = self.region.start.add(offset);
//             self.offset += num_pages * PAGE_SIZE;
//             return page_virt;
//         }
//         offset -= region_size;
//
//         panic!("oom")
//     }
// }
