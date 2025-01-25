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
#![feature(box_into_inner)]
#![expect(dead_code, reason = "TODO")] // TODO remove
#![expect(edition_2024_expr_fragment_specifier, reason = "vetted")]

extern crate alloc;

mod allocator;
mod arch;
mod error;
mod executor;
mod logger;
mod machine_info;
mod metrics;
mod panic;
mod thread_local;
mod time;
mod trap_handler;
mod util;
mod vm;
mod wasm;

use crate::error::Error;
use crate::machine_info::{HartLocalMachineInfo, MachineInfo};
use crate::vm::bootstrap_alloc::BootstrapAllocator;
use arrayvec::ArrayVec;
use core::cell::RefCell;
use core::range::Range;
use loader_api::{BootInfo, LoaderConfig, MemoryRegionKind};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sync::{Once, OnceLock};
use thread_local::thread_local;
use time::Instant;
use vm::frame_alloc;
use vm::PhysicalAddress;

/// The log level for the kernel
pub const LOG_LEVEL: log::Level = log::Level::Trace;
/// The size of the stack in pages
pub const STACK_SIZE_PAGES: u32 = 128; // TODO find a lower more appropriate value
/// The size of the trap handler stack in pages
pub const TRAP_STACK_SIZE_PAGES: usize = 64; // TODO find a lower more appropriate value
/// The initial size of the kernel heap in pages.
///
/// This initial size should be small enough so the loaders less sophisticated allocator can
/// doesn't cause startup slowdown & inefficient mapping, but large enough so we can bootstrap
/// our own virtual memory subsystem. At that point we are no longer reliant on this initial heap
/// size and can dynamically grow the heap as needed.
pub const INITIAL_HEAP_SIZE_PAGES: usize = 4096 * 2; // 32 MiB

pub type Result<T> = core::result::Result<T, Error>;

pub static MACHINE_INFO: OnceLock<MachineInfo> = OnceLock::new();

thread_local!(
    pub static HART_LOCAL_MACHINE_INFO: RefCell<HartLocalMachineInfo> =
        RefCell::new(HartLocalMachineInfo::default());
);

#[used(linker)]
#[unsafe(link_section = ".loader_config")]
static LOADER_CONFIG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = STACK_SIZE_PAGES;
    cfg
};

#[unsafe(no_mangle)]
fn _start(hartid: usize, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    // initialize the hart local state of the logger before enabling it, so it is ready as soon as
    // logging is turned on
    logger::per_hart_init(hartid);

    // perform EARLY per-hart, architecture-specific initialization
    // (e.g. resetting the FPU)
    arch::per_hart_init_early();

    let fdt = locate_device_tree(boot_info);

    static SYNC: Once = Once::new();
    SYNC.call_once(|| {
        // initialize the global logger as early as possible
        logger::init(LOG_LEVEL.to_level_filter());

        // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
        // is available
        let allocatable_memories = allocatable_memory_regions(boot_info);
        let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

        // initializing the global allocator
        allocator::init(&mut boot_alloc, boot_info);

        // initialize the panic backtracing subsystem after the allocator has been set up
        // since setting up the symbolization context requires allocation
        panic::init(boot_info);

        // perform global, architecture-specific initialization
        arch::init();

        // // TODO move this into a init function
        let minfo = MACHINE_INFO
            .get_or_try_init(|| {
                // Safety: we have to trust the loader mapped the fdt correctly
                unsafe { MachineInfo::from_dtb(fdt) }
            })
            .unwrap();
        log::debug!("\n{minfo}");

        // initialize the global frame allocator
        frame_alloc::init(boot_alloc);

        let mut rng = ChaCha20Rng::from_seed(minfo.rng_seed.unwrap()[0..32].try_into().unwrap());

        // initialize the virtual memory subsystem
        vm::init(boot_info, &mut rng).unwrap();

        // initialize the executor
        executor::init(boot_info.hart_mask.count_ones() as usize, &mut rng);
    });

    // // Safety: we have to trust the loader mapped the fdt correctly
    let hart_local_minfo = unsafe { HartLocalMachineInfo::from_dtb(hartid, fdt).unwrap() };
    log::debug!("\n{hart_local_minfo}");
    HART_LOCAL_MACHINE_INFO.set(hart_local_minfo);

    // perform EARLY per-hart, architecture-specific initialization
    // (e.g. setting the trap vector and enabling interrupts)
    arch::per_hart_init_late();

    log::info!(
        "Booted in ~{:?} ({:?} in k23)",
        Instant::now().duration_since(Instant::ZERO),
        Instant::from_ticks(boot_ticks).elapsed()
    );

    executor::run(executor::current(), hartid).unwrap();

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
