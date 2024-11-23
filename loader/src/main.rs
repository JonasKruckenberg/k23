#![no_std]
#![no_main]

// What we need to do
// - setup stack ptr
// - fill stack with canary pattern
// - disable interrupts
// - zero BSS
// - initialize logger
// - parse DTB
// - identity map self
// - map physical memory
// - map kernel elf
//      - map load segments
//      - allocate & map TLS segment
//      - apply relocations
//      - process RELRO segments
// - map kernel stacks
// - switch to kernel address space















// mod arch;
// mod boot_info;
// mod error;
// mod kernel;
// mod machine_info;
// mod page_alloc;
// 
// pub const STACK_SIZE_PAGES: usize = 32;
// pub const LOG_LEVEL: log::Level = log::Level::Trace;
// pub const ENABLE_KASLR: bool = false;
// 
// use crate::error::Error;
// use crate::machine_info::MachineInfo;
// use crate::page_alloc::PageAllocator;
// use cfg_if::cfg_if;
// use core::ops::Range;
// use core::ptr::addr_of;
// use loader_api::BootInfo;
// use pmm::{AddressRangeExt, Arch as _, ArchFlags, BumpAllocator, PhysicalAddress, VirtualAddress};
// use rand::SeedableRng;
// use rand_chacha::ChaCha20Rng;
// use crate::boot_info::init_boot_info;
// 
// pub type Result<T> = core::result::Result<T, Error>;
// 
// fn main(hartid: usize, minfo: &'static MachineInfo) -> ! {
//     static INIT: sync::OnceLock<()> = sync::OnceLock::new();
// 
//     INIT.get_or_try_init(|| -> Result<_> {
//         
//         
//         
//         
//         
//         
//         
//         todo!()
//     })
//     .expect("failed to initialize global system state");
// 
//     todo!()
// }
// 
// 
















// struct BootstrapState<A>
// where
//     A: pmm::Arch,
//     [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
// {
//     pub pmm: A,
//     pub frame_alloc: BumpAllocator<'static, A>,
//     pub page_alloc: PageAllocator<A>,
//     pub loader_phys: Range<PhysicalAddress>,
//     pub physmap: Range<VirtualAddress>,
// }
// 
// fn bootstrap_mmu(minfo: &MachineInfo) -> Result<BootstrapState<pmm::Riscv64Sv39>> {
//     let loader_regions = LoaderRegions::new(minfo);
// 
//     let mut frame_alloc = unsafe {
//         BumpAllocator::new_with_lower_bound(&minfo.memories, loader_regions.read_write.end)
//     };
// 
//     let mut page_alloc = if ENABLE_KASLR {
//         PageAllocator::new(ChaCha20Rng::from_seed(
//             minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
//         ))
//     } else {
//         PageAllocator::new_no_kaslr()
//     };
// 
//     let mut pmm = pmm::Riscv64Sv39::new(&mut frame_alloc, 0, VirtualAddress::default())?;
// 
//     let loader_phys = identity_map_loader(&mut pmm, &mut frame_alloc, loader_regions)?;
//     let physmap = map_physical_memory(&mut pmm, &mut frame_alloc, &mut page_alloc, minfo)?;
// 
//     pmm.activate()?;
// 
//     Ok(Bootstrap {
//         pmm: pmm::Riscv64Sv39::from_active(0, physmap.start)?,
//         frame_alloc,
//         page_alloc,
//         loader_phys,
//         physmap,
//     })
// }
// 
// pub fn identity_map_loader<A>(
//     arch: &mut A,
//     frame_alloc: &mut BumpAllocator<A>,
//     loader_regions: LoaderRegions,
// ) -> crate::Result<Range<PhysicalAddress>>
// where
//     A: pmm::Arch,
// {
//     log::trace!(
//         "Identity mapping own executable region {:?}...",
//         loader_regions.executable
//     );
//     arch.identity_map_contiguous(
//         frame_alloc,
//         loader_regions.executable.clone(),
//         ArchFlags::READ | ArchFlags::EXECUTE,
//     )?;
// 
//     log::trace!(
//         "Identity mapping own read-only region {:?}...",
//         loader_regions.read_only
//     );
//     arch.identity_map_contiguous(
//         frame_alloc,
//         loader_regions.read_only.clone(),
//         ArchFlags::READ,
//     )?;
// 
//     log::trace!(
//         "Identity mapping own read-write region {:?}...",
//         loader_regions.read_write
//     );
//     arch.identity_map_contiguous(
//         frame_alloc,
//         loader_regions.read_write.clone(),
//         ArchFlags::READ | ArchFlags::WRITE,
//     )?;
// 
//     Ok(loader_regions.executable.start..loader_regions.read_write.end)
// }
// 
// fn get_alignment_for_size<A>(size: usize) -> usize
// where
//     A: pmm::Arch,
// {
//     for lvl in 0..A::PAGE_TABLE_LEVELS {
//         let page_size = 1 << (A::PAGE_SHIFT + lvl * A::PAGE_ENTRY_SHIFT);
// 
//         if size <= page_size {
//             return page_size;
//         }
//     }
// 
//     unreachable!()
// }
// 
// pub fn map_physical_memory<A>(
//     arch: &mut A,
//     frame_alloc: &mut BumpAllocator<A>,
//     page_alloc: &mut PageAllocator<A>,
//     minfo: &MachineInfo,
// ) -> crate::Result<Range<VirtualAddress>>
// where
//     A: pmm::Arch,
//     [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
// {
//     let physmem_hull = minfo.memory_hull();
//     let alignment = get_alignment_for_size::<A>(physmem_hull.size());
// 
//     let physmap_virt = page_alloc.reserve_range(minfo.memory_hull().size(), alignment);
// 
//     log::trace!("physmap {physmap_virt:?}");
//     for region_phys in &minfo.memories {
//         let region_virt = physmap_virt.start.add(region_phys.start.as_raw())
//             ..physmap_virt.start.add(region_phys.end.as_raw());
// 
//         log::trace!("Mapping physical memory region {region_virt:?} => {region_phys:?}...");
//         arch.map_contiguous(
//             frame_alloc,
//             region_virt,
//             region_phys.clone(),
//             ArchFlags::READ | ArchFlags::WRITE,
//         )?;
//     }
// 
//     Ok(physmap_virt)
// }
// 
// #[derive(Debug)]
// pub struct LoaderRegions {
//     pub executable: Range<PhysicalAddress>,
//     pub read_only: Range<PhysicalAddress>,
//     pub read_write: Range<PhysicalAddress>,
// }
// 
// impl LoaderRegions {
//     #[must_use]
//     pub fn new<A>(machine_info: &MachineInfo) -> Self
//     where
//         A: pmm::Arch,
//     {
//         extern "C" {
//             static __text_start: u8;
//             static __text_end: u8;
//             static __rodata_start: u8;
//             static __rodata_end: u8;
//             static __bss_start: u8;
//             static __stack_start: u8;
//         }
// 
//         let executable: Range<PhysicalAddress> = {
//             PhysicalAddress::new(addr_of!(__text_start) as usize)
//                 ..PhysicalAddress::new(addr_of!(__text_end) as usize)
//         };
// 
//         let read_only: Range<PhysicalAddress> = {
//             PhysicalAddress::new(addr_of!(__rodata_start) as usize)
//                 ..PhysicalAddress::new(addr_of!(__rodata_end) as usize)
//         };
// 
//         let read_write: Range<PhysicalAddress> = {
//             let start = PhysicalAddress::new(addr_of!(__bss_start) as usize);
//             let stack_start = PhysicalAddress::new(addr_of!(__stack_start) as usize);
// 
//             start..stack_start.add(machine_info.cpus * STACK_SIZE_PAGES * A::PAGE_SIZE)
//         };
// 
//         LoaderRegions {
//             executable,
//             read_only,
//             read_write,
//         }
//     }
// }
// 
// fn setup_kernel_address_space() -> Result<()> {
//     todo!()
// }

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let location = info.location().map(|l| l.file()).unwrap_or("<unknown>");
    let line = info.location().map(|l| l.line()).unwrap_or(0);
    let col = info.location().map(|l| l.column()).unwrap_or(0);

    log::error!(
        "hart panicked at {location}:{line}:{col}: \n{}",
        info.message()
    );

    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}


// //
// // fn main(hartid: usize, minfo: &'static MachineInfo) -> ! {
// //     static INIT: sync::OnceLock<(KernelAddressSpace<pmm::Riscv64Sv39>, PhysicalAddress)> =
// //         sync::OnceLock::new();
// //
// //     let (kernel_aspace, boot_info) = INIT
// //         .get_or_try_init(
// //             || -> Result<(KernelAddressSpace<pmm::Riscv64Sv39>, PhysicalAddress)> {
// //                 log::info!("welcome to k23 v{}", env!("CARGO_PKG_VERSION"));
// //
// //                 let loader_regions = LoaderRegions::new::<pmm::Riscv64Sv39>(minfo);
// //
// //                 let mut frame_alloc: BumpAllocator<pmm::Riscv64Sv39> = unsafe {
// //                     BumpAllocator::new_with_lower_bound(
// //                         &minfo.memories,
// //                         loader_regions.read_write.end,
// //                     )
// //                 };
// //
// //                 let mut page_alloc = if ENABLE_KASLR {
// //                     PageAllocator::new(ChaCha20Rng::from_seed(
// //                         minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
// //                     ))
// //                 } else {
// //                     PageAllocator::new_no_kaslr()
// //                 };
// //
// //                 let mut pmm =
// //                     pmm::Riscv64Sv39::new(&mut frame_alloc, 0, VirtualAddress::default())?;
// //
// //                 let loader_phys = identity_map_loader(&mut pmm, &mut frame_alloc, loader_regions)?;
// //                 let physmap =
// //                     map_physical_memory(&mut pmm, &mut frame_alloc, &mut page_alloc, minfo)?;
// //
// //                 pmm.activate()?;
// //
// //                 let pmm = pmm::Riscv64Sv39::from_active(0, physmap.start)?;
// //
// //                 // TODO map kernel address space
// //                 //      TODO map kernel elf
// //                 //      TODO allocate and map TLS
// //                 //      TODO map stacks
// //                 //
// //
// //                 todo!()
// //
// //                 //
// //                 //
// //                 // // Move the device tree blob from wherever random place the previous bootloader put it
// //                 // // into a properly allocated place so we don't accidentally override it
// //                 // let fdt = alloc_and_copy_fdt(minfo, &mut frame_alloc)?;
// //                 //
// //                 // // Parse the inlined kernel ELF file
// //                 // let kernel = parse_inlined_kernel()?;
// //                 //
// //                 // // Initialize the kernel address space
// //                 // let kernel_aspace = KernelAddressSpace::new(
// //                 //     pmm_arch,
// //                 //     &mut frame_alloc,
// //                 //     &kernel,
// //                 //     loader_regions,
// //                 //     minfo,
// //                 // )?;
// //                 //
// //                 // // Set up the BootInfo struct that we will pass on to the kernel
// //                 // let boot_info = boot_info::init_boot_info(
// //                 //     &mut frame_alloc,
// //                 //     hartid,
// //                 //     &kernel_aspace,
// //                 //     &kernel,
// //                 //     fdt,
// //                 // )?;
// //                 //
// //                 // Ok((kernel_aspace, boot_info))
// //             },
// //         )
// //         .expect("failed global initialization");
// //

// //
// //     log::debug!("[HART {hartid}] Initializing TLS region...");
// //     kernel_aspace.init_tls_region_for_hart(hartid);
// //
// //     // unsafe {
// //     //     let ptr: *const u64 = kernel_aspace.kernel_virt().start.add(0x0000000000009570).as_raw() as _;
// //     //     log::trace!("{:?}", ptr.read());
// //     // }
// //
// //     panic!();
// //
// //     // Safety: We essentially jump to arbitrary memory here. But we have no choice
// //     // other than to rely on `KernelAddressSpace::entry_virt` being correct.
// //     unsafe {
// //         arch::handoff_to_kernel(
// //             hartid,
// //             kernel_aspace.entry_virt(),
// //             kernel_aspace.stack_region_for_hart(hartid),
// //             kernel_aspace
// //                 .tls_region_for_hart(hartid)
// //                 .unwrap_or_default()
// //                 .start,
// //             // TODO make fn
// //             kernel_aspace.physmap().start.add(boot_info.as_raw()),
// //         )
// //     }
// // }
// //
// // // FIXME this should ideally not be necessary, we should leave a hole in the
// // // memory regions it is fine to allocate from instead.
// // pub fn alloc_and_copy_fdt<A>(
// //     machine_info: &MachineInfo,
// //     alloc: &mut BumpAllocator<A>,
// // ) -> Result<PhysicalAddress>
// // where
// //     A: pmm::Arch,
// // {
// //     let frames = machine_info.fdt.len().div_ceil(A::PAGE_SIZE);
// //     let base = alloc.allocate_frames_contiguous(frames)?;
// //
// //     unsafe {
// //         let dst = slice::from_raw_parts_mut(base.as_raw() as *mut u8, machine_info.fdt.len());
// //
// //         ptr::copy_nonoverlapping(machine_info.fdt.as_ptr(), dst.as_mut_ptr(), dst.len());
// //     }
// //
// //     Ok(base)
// // }
// //
