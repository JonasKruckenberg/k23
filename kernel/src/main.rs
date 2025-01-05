#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![feature(naked_functions)]
#![feature(thread_local)]
#![feature(never_type)]
#![feature(new_range_api)]
#![feature(maybe_uninit_slice)]
#![feature(debug_closure_helpers)]
#![feature(maybe_uninit_fill)]
#![allow(internal_features)]
#![feature(std_internals)]
#![feature(panic_can_unwind)]
#![feature(fmt_internals)]

extern crate alloc;

mod allocator;
mod arch;
mod error;
mod logger;
mod machine_info;
mod panic;
mod time;

use crate::error::Error;
use crate::machine_info::{HartLocalMachineInfo, MachineInfo};
use arrayvec::ArrayVec;
use core::alloc::Layout;
use core::cell::{RefCell};
use core::range::Range;
use core::{ptr, slice};
use loader_api::{BootInfo, MemoryRegionKind};
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::{BootstrapAllocator, FrameAllocator};
use mmu::PhysicalAddress;
use sync::OnceLock;
use thread_local::thread_local;

/// The log level for the kernel
pub const LOG_LEVEL: log::Level = log::Level::Trace;
/// The size of the stack in pages
pub const STACK_SIZE_PAGES: usize = 256;
/// The size of the trap handler stack in pages
pub const TRAP_STACK_SIZE_PAGES: usize = 16;
/// The initial size of the kernel heap in pages.
///
/// This initial size should be small enough so the loaders less sophisticated allocator can
/// doesn't cause startup slowdown & inefficient mapping, but large enough so we can bootstrap
/// our own virtual memory subsystem. At that point we are no longer reliant on this initial heap
/// size and can dynamically grow the heap as needed.
pub const INITIAL_HEAP_SIZE_PAGES: usize = 2048; // 32 MiB

pub type Result<T> = core::result::Result<T, Error>;

pub static MACHINE_INFO: OnceLock<MachineInfo> = OnceLock::new();

thread_local!(
    pub static HART_LOCAL_MACHINE_INFO: RefCell<HartLocalMachineInfo> = RefCell::new(HartLocalMachineInfo::default());
);

pub fn main(hartid: usize, boot_info: &'static BootInfo) -> ! {
    // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
    // is available
    let allocatable_memories = allocatable_memory_regions(boot_info);
    let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);
    boot_alloc.set_phys_offset(boot_info.physical_address_offset);
    
    // initializing the global allocator
    allocator::init(&mut boot_alloc, boot_info);
    
    // initialize thread-local storage
    // done after global allocator initialization since TLS destructors are registered in a heap
    // allocated Vec
    init_tls(&mut boot_alloc, boot_info);

    // initialize the logger
    // done after TLS initialization since we maintain per-hart host stdio channels
    logger::init_hart(hartid);
    logger::init(LOG_LEVEL.to_level_filter());

    log::debug!("\n{boot_info}");
    log::trace!("Allocatable memory regions: {allocatable_memories:?}");

    // perform per-hart, architecture-specific initialization
    // (e.g. setting the trap vector and resetting the FPU)
    arch::per_hart_init();

    // perform global, architecture-specific initialization
    arch::init();

    let fdt = locate_device_tree(boot_info);

    // TODO move this into a init function
    let minfo = MACHINE_INFO
        .get_or_try_init(|| unsafe { MachineInfo::from_dtb(fdt) })
        .unwrap();
    log::debug!("\n{minfo}");

    // TODO move this into a per_hart_init function
    let hart_local_minfo = unsafe { HartLocalMachineInfo::from_dtb(hartid, fdt).unwrap() };
    log::debug!("\n{hart_local_minfo}");

    log::trace!("Hello from hart {}", hartid);

    HART_LOCAL_MACHINE_INFO.set(hart_local_minfo);
    // frame_alloc::init(boot_alloc, boot_info.physical_address_offset);

    // TODO init frame allocation (requires boot info)
    //      - init PMM zones
    //          - init PMM arenas
    //              - init buddy allocator
    // TODO init kernel address space (requires global allocator)
    // TODO init TLS (requires kernel address space)

    // - `pmm_init()`
    //     - [all][global]   init arenas/sections
    //         - for each reported memory region
    //             - calculate & create bookkeeping slice (region size / page size) * size_of::<Page>()
    //             - mark bookkeeping pages as wired
    //             - initialize buddy allocator with non-bookkeeping pages
    //             - create arena/section
    //                 struct Arena { pages: &'static [Page] }
    //         - for each loader-used region => mark as used & wired
    //         - for each used region => mark as used & wired
    // - [all][global] parse cmdline
    // - [all][global] `vm::init()` init virtual memory management
    // - [all][global] `lockup::init()` initialize lockup detector
    // - [all][global] `topology::init()` initialize the system topology
    // - initialize other parts of the kernel
    // - kickoff the scheduler
    // - `platform_init()`
    //     - using system topology -> start other harts in the system
    // - `arch_late_init_percpu()`
    //     - IF RiscvFeatureVector => setup the vector hardware
    // - `kernel_shell_init()`
    // - `userboot_init()`

    arch::exit(0);
}

fn init_tls(boot_alloc: &mut BootstrapAllocator, boot_info: &BootInfo) {
    if let Some(template) = &boot_info.tls_template {
        let layout = Layout::from_size_align(template.mem_size, PAGE_SIZE).unwrap();
        let phys = boot_alloc.allocate_contiguous(layout).unwrap();

        // Use the phys_map to access the newly allocated TLS region
        let virt = boot_info
            .physical_address_offset
            .checked_add(phys.get())
            .unwrap();

        // Copy any TDATA from the template to the new TLS region
        if template.file_size != 0 {
            let src: &[u8] =
                unsafe { slice::from_raw_parts(template.start_addr.as_ptr(), template.file_size) };

            let dst = unsafe {
                slice::from_raw_parts_mut(
                    virt.checked_add(template.mem_size).unwrap().as_mut_ptr(),
                    template.file_size,
                )
            };

            log::trace!(
                "Copying tdata from {:?} to {:?}",
                src.as_ptr_range(),
                dst.as_ptr_range()
            );
            debug_assert_eq!(src.len(), dst.len());
            unsafe {
                ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
            }
        }

        arch::set_thread_ptr(virt);
    }
}

/// Builds a list of memory regions from the boot info that are usable for allocation.
///
/// The regions passed by the loader are guaranteed to be non-overlapping, but might not be
/// sorted and might not be optimally "packed". This function will both sort regions and
/// attempt to compact the list by merging adjacent regions.
fn allocatable_memory_regions(boot_info: &BootInfo) -> ArrayVec<Range<PhysicalAddress>, 16> {
    let temp: ArrayVec<Range<PhysicalAddress>, 16> = boot_info
        .memory_regions()
        .iter()
        .filter_map(|region| region.kind.is_usable().then_some(region.range))
        .collect();

    // merge adjacent regions
    let mut out: ArrayVec<Range<PhysicalAddress>, 16> = ArrayVec::new();
    'outer: for region in temp {
        for other in &mut out {
            if region.start == other.end {
                other.end = region.end;
                continue 'outer;
            }
            if region.end == other.start {
                other.start = region.start;
                continue 'outer;
            }
        }

        out.push(region.clone());
    }

    out
}

fn locate_device_tree(boot_info: &'static BootInfo) -> *const u8 {
    let fdt = boot_info
        .memory_regions()
        .iter()
        .find(|region| region.kind == MemoryRegionKind::FDT)
        .expect("no FDT region");

    boot_info
        .physical_address_offset
        .checked_add(fdt.range.start.get())
        .unwrap()
        .as_ptr()
}
