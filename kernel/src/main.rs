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
#![feature(let_chains)]
#![feature(array_chunks)]
#![feature(iter_array_chunks)]
#![feature(iter_next_chunk)]
#![feature(if_let_guard)]
#![feature(allocator_api)]
#![expect(dead_code, reason = "TODO")] // TODO remove
#![expect(edition_2024_expr_fragment_specifier, reason = "vetted")]

extern crate alloc;

mod allocator;
mod arch;
mod device_tree;
mod error;
mod executor;
mod hart_local;
mod logger;
mod metrics;
mod panic;
mod time;
mod traps;
mod util;
mod vm;
mod wasm;

use crate::device_tree::device_tree;
use crate::error::Error;
use crate::vm::bootstrap_alloc::BootstrapAllocator;
use arrayvec::ArrayVec;
use core::cell::Cell;
use core::range::Range;
use core::slice;
use hart_local::thread_local;
use loader_api::{BootInfo, LoaderConfig, MemoryRegionKind};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sync::Once;
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

thread_local!(
    pub static HARTID: Cell<usize> = Cell::new(usize::MAX);
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
    HARTID.set(hartid);

    // perform EARLY per-hart, architecture-specific initialization
    // (e.g. resetting the FPU)
    arch::per_hart_init_early();

    let (fdt, fdt_region_phys) = locate_device_tree(boot_info);

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

        let minfo = device_tree::init(fdt).unwrap();
        log::debug!("{minfo:?}");

        // initialize the global frame allocator
        // at this point we have parsed and processed the flattened device tree, so we pass it to the
        // frame allocator for reuse
        frame_alloc::init(boot_alloc, fdt_region_phys);

        // let mut rng = ChaCha20Rng::from_seed(minfo.rng_seed.unwrap()[0..32].try_into().unwrap());
        let mut rng = ChaCha20Rng::from_seed([0; 32]);

        // initialize the virtual memory subsystem
        vm::init(boot_info, &mut rng).unwrap();

        // initialize the executor

        // if we're executing tests we don't want idle harts to park indefinitely, instead the
        // runtime should just shut down
        let shutdown_on_idle = cfg!(test);

        executor::init(
            boot_info.hart_mask.count_ones() as usize,
            &mut rng,
            shutdown_on_idle,
        );
    });

    // perform EARLY per-hart, architecture-specific initialization
    // (e.g. setting the trap vector and enabling interrupts)
    arch::per_hart_init_late(device_tree()).unwrap();

    log::info!(
        "Booted in ~{:?} ({:?} in k23)",
        Instant::now().duration_since(Instant::ZERO),
        Instant::from_ticks(boot_ticks).elapsed()
    );

    let _ = executor::run(executor::current(), hartid, || {
        executor::current().spawn(async move {
            log::info!("Hello from hart {}", hartid);
        });
    });

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
        hart_local::destructors::run();
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

fn locate_device_tree(boot_info: &BootInfo) -> (&'static [u8], Range<PhysicalAddress>) {
    let fdt = boot_info
        .memory_regions
        .iter()
        .find(|region| region.kind == MemoryRegionKind::FDT)
        .expect("no FDT region");

    let base = boot_info
        .physical_address_offset
        .checked_add(fdt.range.start)
        .unwrap() as *const u8;

    // Safety: we need to trust the bootinfo data is correct
    let slice =
        unsafe { slice::from_raw_parts(base, fdt.range.end.checked_sub(fdt.range.start).unwrap()) };
    (
        slice,
        Range::from(PhysicalAddress::new(fdt.range.start)..PhysicalAddress::new(fdt.range.end)),
    )
}
