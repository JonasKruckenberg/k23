// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![feature(naked_functions)]
#![feature(thread_local, never_type)]
#![feature(new_range_api)]
#![feature(debug_closure_helpers)]
#![expect(internal_features, reason = "panic internals")]
#![feature(std_internals, panic_can_unwind, fmt_internals)]
#![feature(step_trait)]
#![expect(dead_code, reason = "TODO")] // TODO remove
#![expect(edition_2024_expr_fragment_specifier, reason = "vetted")]

extern crate alloc;

mod allocator;
mod arch;
mod error;
mod logger;
mod machine_info;
mod metrics;
mod panic;
mod thread_local;
mod time;
mod trap_handler;
mod vm;
mod wasm;

use crate::error::Error;
use crate::machine_info::{HartLocalMachineInfo, MachineInfo};
use crate::time::Instant;
use crate::vm::bootstrap_alloc::BootstrapAllocator;
use arrayvec::ArrayVec;
use core::alloc::Layout;
use core::cell::RefCell;
use core::range::Range;
use core::{cmp, slice};
use loader_api::{BootInfo, MemoryRegionKind, TlsTemplate};
use sync::OnceLock;
use thread_local::thread_local;
use vm::frame_alloc;
use vm::{PhysicalAddress, VirtualAddress};

/// The log level for the kernel
pub const LOG_LEVEL: log::Level = log::Level::Trace;
/// The size of the stack in pages
pub const STACK_SIZE_PAGES: usize = 128; // TODO find a lower more appropriate value
/// The size of the trap handler stack in pages
pub const TRAP_STACK_SIZE_PAGES: usize = 64; // TODO find a lower more appropriate value
/// The initial size of the kernel heap in pages.
///
/// This initial size should be small enough so the loaders less sophisticated allocator can
/// doesn't cause startup slowdown & inefficient mapping, but large enough so we can bootstrap
/// our own virtual memory subsystem. At that point we are no longer reliant on this initial heap
/// size and can dynamically grow the heap as needed.
pub const INITIAL_HEAP_SIZE_PAGES: usize = 4096; // 32 MiB

pub type Result<T> = core::result::Result<T, Error>;

pub static MACHINE_INFO: OnceLock<MachineInfo> = OnceLock::new();

thread_local!(
    pub static HART_LOCAL_MACHINE_INFO: RefCell<HartLocalMachineInfo> =
        RefCell::new(HartLocalMachineInfo::default());
);

fn main(hartid: usize, boot_info: &'static BootInfo) -> ! {
    // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
    // is available
    let allocatable_memories = allocatable_memory_regions(boot_info);
    let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

    // initializing the global allocator
    allocator::init(&mut boot_alloc, boot_info);

    // initialize the panic backtracing subsystem after the allocator has been set up
    // since setting up the symbolization context requires allocation
    panic::init(boot_info);

    // initialize thread-local storage
    // done after global allocator initialization since TLS destructors are registered in a heap
    // allocated Vec
    let tls = init_tls(&mut boot_alloc, &boot_info.tls_template);

    // initialize the logger
    // done after TLS initialization since we maintain per-hart host stdio channels
    logger::init_hart(hartid);
    logger::init(LOG_LEVEL.to_level_filter());

    log::debug!("\n{boot_info}");
    log::trace!("Allocatable memory regions: {allocatable_memories:?}");
    log::trace!("Thread pointer: {tls:?}");

    // perform per-hart, architecture-specific initialization
    // (e.g. setting the trap vector and resetting the FPU)
    arch::per_hart_init();

    // perform global, architecture-specific initialization
    arch::init();

    let fdt = locate_device_tree(boot_info);

    // TODO move this into a init function
    let minfo = MACHINE_INFO
        .get_or_try_init(|| {
            // Safety: we have to trust the loader mapped the fdt correctly
            unsafe { MachineInfo::from_dtb(fdt) }
        })
        .unwrap();
    log::debug!("\n{minfo}");

    // Safety: we have to trust the loader mapped the fdt correctly
    let hart_local_minfo = unsafe { HartLocalMachineInfo::from_dtb(hartid, fdt).unwrap() };
    log::debug!("\n{hart_local_minfo}");
    HART_LOCAL_MACHINE_INFO.set(hart_local_minfo);

    frame_alloc::init(boot_alloc);

    // TODO init kernel address space (requires global allocator)

    vm::init(boot_info, minfo).unwrap();

    log::info!(
        "Booted in ~{:?} ({:?} in k23)",
        Instant::now().duration_since(Instant::ZERO),
        Instant::from_ticks(boot_info.boot_ticks).elapsed()
    );
    // wasm::test();

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

    // Run thread-local destructors
    // Safety: after this point thread-locals cannot be accessed anymore anyway
    unsafe {
        thread_local::destructors::run();
    }

    arch::exit(0);
}

fn init_tls(
    boot_alloc: &mut BootstrapAllocator,
    maybe_tls_template: &Option<TlsTemplate>,
) -> Option<VirtualAddress> {
    if let Some(template) = &maybe_tls_template {
        let layout =
            Layout::from_size_align(template.mem_size, cmp::max(template.align, arch::PAGE_SIZE))
                .unwrap();
        let phys = boot_alloc.allocate_contiguous_zeroed(layout).unwrap();

        // Use the phys_map to access the newly allocated TLS region
        let virt = VirtualAddress::from_phys(phys).unwrap();

        if template.file_size != 0 {
            // Safety: We have to trust the loaders BootInfo here
            unsafe {
                let src: &[u8] =
                    slice::from_raw_parts(template.start_addr as *const u8, template.file_size);
                let dst: &mut [u8] =
                    slice::from_raw_parts_mut(virt.as_mut_ptr(), template.file_size);

                // sanity check to ensure our destination allocated memory is actually zeroed.
                // if it's not, that likely means we're about to override something important
                debug_assert!(dst.iter().all(|&x| x == 0));

                dst.copy_from_slice(src);
            }
        }

        arch::set_thread_ptr(virt);
        Some(virt)
    } else {
        None
    }
}

/// Builds a list of memory regions from the boot info that are usable for allocation.
///
/// The regions passed by the loader are guaranteed to be non-overlapping, but might not be
/// sorted and might not be optimally "packed". This function will both sort regions and
/// attempt to compact the list by merging adjacent regions.
fn allocatable_memory_regions(boot_info: &BootInfo) -> ArrayVec<Range<PhysicalAddress>, 16> {
    let temp: ArrayVec<Range<PhysicalAddress>, 16> = boot_info
        .memory_regions
        .iter()
        .filter_map(|region| {
            let range = Range::from(
                PhysicalAddress::new(region.range.start)..PhysicalAddress::new(region.range.end),
            );

            region.kind.is_usable().then_some(range)
        })
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

        out.push(region);
    }

    out
}

fn locate_device_tree(boot_info: &BootInfo) -> *const u8 {
    let fdt = boot_info
        .memory_regions
        .iter()
        .find(|region| region.kind == MemoryRegionKind::FDT)
        .expect("no FDT region");

    boot_info
        .physical_address_offset
        .checked_add(fdt.range.start)
        .unwrap() as *const u8
}
