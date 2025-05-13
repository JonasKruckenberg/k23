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
#![feature(std_internals, panic_can_unwind, formatting_options)]
#![feature(step_trait)]
#![feature(box_into_inner)]
#![feature(let_chains)]
#![feature(array_chunks)]
#![feature(iter_array_chunks)]
#![feature(iter_next_chunk)]
#![feature(if_let_guard)]
#![feature(allocator_api)]
#![expect(dead_code, reason = "TODO")] // TODO remove
#![feature(asm_unwind)]

extern crate alloc;
extern crate panic_unwind;

mod allocator;
mod arch;
mod backtrace;
mod bootargs;
mod cpu_local;
mod device_tree;
mod irq;
mod mem;
mod metrics;
mod runtime;
mod shell;
#[cfg(test)]
mod tests;
mod time;
mod tracing;
mod util;
mod wasm;

use crate::backtrace::Backtrace;
use crate::device_tree::device_tree;
use crate::mem::bootstrap_alloc::BootstrapAllocator;
use abort::abort;
use arrayvec::ArrayVec;
use async_kit::time::{Instant, Ticks};
use cfg_if::cfg_if;
use core::cell::Cell;
use core::range::Range;
use core::slice;
use cpu_local::cpu_local;
use loader_api::{BootInfo, LoaderConfig, MemoryRegionKind};
use mem::frame_alloc;
use mem::PhysicalAddress;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::{Once, OnceLock};

/// The size of the stack in pages
pub const STACK_SIZE_PAGES: u32 = 256; // TODO find a lower more appropriate value
/// The size of the trap handler stack in pages
pub const TRAP_STACK_SIZE_PAGES: usize = 64; // TODO find a lower more appropriate value
/// The initial size of the kernel heap in pages.
///
/// This initial size should be small enough so the loaders less sophisticated allocator can
/// doesn't cause startup slowdown & inefficient mapping, but large enough so we can bootstrap
/// our own virtual memory subsystem. At that point we are no longer reliant on this initial heap
/// size and can dynamically grow the heap as needed.
pub const INITIAL_HEAP_SIZE_PAGES: usize = 4096 * 2; // 32 MiB

pub type Result<T> = anyhow::Result<T>;

cpu_local!(
    pub static CPUID: Cell<usize> = Cell::new(usize::MAX);
);
pub static BOOT_INFO: OnceLock<&'static BootInfo> = OnceLock::new();

#[used(linker)]
#[unsafe(link_section = ".loader_config")]
static LOADER_CONFIG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = STACK_SIZE_PAGES;
    cfg
};

#[unsafe(no_mangle)]
fn _start(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    BOOT_INFO.get_or_init(|| boot_info);

    panic_unwind::set_hook(|info| {
        tracing::error!("CPU {info}");

        // FIXME 32 seems adequate for unoptimized builds where the callstack can get quite deep
        //  but (at least at the moment) is absolute overkill for optimized builds. Sadly there
        //  is no good way to do conditional compilation based on the opt-level.
        const MAX_BACKTRACE_FRAMES: usize = 32;

        let backtrace = backtrace::__rust_end_short_backtrace(|| {
            Backtrace::<MAX_BACKTRACE_FRAMES>::capture().unwrap()
        });
        tracing::error!("{backtrace}");

        if backtrace.frames_omitted {
            tracing::warn!("Stack trace was larger than backtrace buffer, omitted some frames.");
        }
    });

    // Unwinding expects at least one landing pad in the callstack, but capturing all unwinds that
    // bubble up to this point is also a good idea since we can perform some last cleanup and
    // print an error message.
    let res = panic_unwind::catch_unwind(|| {
        backtrace::__rust_begin_short_backtrace(|| kmain(cpuid, boot_info, boot_ticks));
    });

    match res {
        Ok(_) => arch::exit(0),
        // If the panic propagates up to this catch here there is nothing we can do, this is a terminal
        // failure.
        Err(_) => {
            tracing::error!("unrecoverable kernel panic");
            abort()
        }
    }
}

fn kmain(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) {
    CPUID.set(cpuid);

    // perform EARLY per-cpu, architecture-specific initialization
    // (e.g. resetting the FPU)
    arch::per_cpu_init_early();

    let (fdt, fdt_region_phys) = locate_device_tree(boot_info);
    let mut rng = ChaCha20Rng::from_seed(boot_info.rng_seed);

    static SYNC: Once = Once::new();
    SYNC.call_once(|| {
        // set up the basic functionality of the tracing subsystem as early as possible
        tracing::init_early();

        // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
        // is available
        let allocatable_memories = allocatable_memory_regions(boot_info);
        tracing::info!("allocatable memories: {:?}", allocatable_memories);
        let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

        // initializing the global allocator
        allocator::init(&mut boot_alloc, boot_info);

        let devtree = device_tree::init(fdt).unwrap();
        tracing::debug!("{devtree:?}");

        let bootargs = bootargs::parse(devtree).unwrap();

        // initialize the backtracing subsystem after the allocator has been set up
        // since setting up the symbolization context requires allocation
        backtrace::init(boot_info, bootargs.backtrace);

        // fully initialize the tracing subsystem now that we can allocate
        tracing::init(bootargs.log);

        // perform global, architecture-specific initialization
        arch::init_early();

        // initialize the global frame allocator
        // at this point we have parsed and processed the flattened device tree, so we pass it to the
        // frame allocator for reuse
        let frame_alloc = frame_alloc::init(boot_alloc, fdt_region_phys);

        // initialize the virtual memory subsystem
        mem::init(boot_info, &mut rng, frame_alloc).unwrap();
    });

    // perform LATE per-cpu, architecture-specific initialization
    // (e.g. setting the trap vector and enabling interrupts)
    arch::per_cpu_init_late(device_tree()).unwrap();

    // now that clocks are online we can make the tracing subsystem print out timestamps
    tracing::per_cpu_init_late(Instant::from_ticks(Ticks(boot_ticks)));

    // initialize the executor
    let _rt = runtime::init(boot_info.cpu_mask.count_ones() as usize);

    tracing::info!(
        "Booted in ~{:?} ({:?} in k23)",
        Instant::now().duration_since(Instant::ZERO),
        Instant::from_ticks(Ticks(boot_ticks)).elapsed()
    );

    cfg_if! {
        if #[cfg(test)] {
            if cpuid == 0 {
                _rt.block_on(tests::run_tests()).exit_if_failed();
                // _rt.shutdown();
            } else {
                runtime::Worker::new(_rt, cpuid, &mut rng).run();
            }
        } else {
            shell::init(
                device_tree(),
                _rt,
                boot_info.cpu_mask.count_ones() as usize,
            ).unwrap();
            runtime::Worker::new(_rt, cpuid, &mut rng).run();
        }
    }

    //         // FIXME we want orderly execution of tests, so below we pick a random thread for execution
    //         //  and force all others to spinwait, which... isn't great but it works. Ideally this
    //         //  should be replaced with something that uses the async runtime to spawn and distribute tests
    //
    //         let t = if cpuid == 0 {
    //             futures::future::Either::Left(async { let _ = tests::run_tests(); })
    //         } else {
    //             futures::future::Either::Right(core::future::pending())
    //         };
    //
    //         scheduler::Worker::new(_sched, cpuid, &mut rng, t).run().unwrap();
    //     } else {
    //         scheduler::Worker::new(_sched, cpuid, &mut rng, core::future::pending()).run().unwrap();
    //     }
    // }

    // if cpuid == 0 {
    //     sched.spawn(async move {
    //         tracing::debug!("before timeout");
    //         let start = Instant::now();
    //         let res =
    //             time::timeout(Duration::from_secs(1), time::sleep(Duration::from_secs(5))).await;
    //         tracing::debug!("after timeout {res:?}");
    //         assert!(res.is_err());
    //         assert_eq!(start.elapsed().as_secs(), 1);
    //
    //         tracing::debug!("before timeout");
    //         let start = Instant::now();
    //         let res =
    //             time::timeout(Duration::from_secs(5), time::sleep(Duration::from_secs(1))).await;
    //         tracing::debug!("after timeout {res:?}");
    //         assert!(res.is_ok());
    //         assert_eq!(start.elapsed().as_secs(), 1);
    //
    //         tracing::debug!("sleeping for 1 sec...");
    //         let start = Instant::now();
    //         time::sleep(Duration::from_secs(1)).await;
    //         assert_eq!(start.elapsed().as_secs(), 1);
    //         tracing::debug!("slept 1 sec! {:?}", start.elapsed());
    //
    //
    //         #[cfg(test)]
    //         scheduler::scheduler().shutdown();
    //     });
    //
    //     // scheduler::scheduler().spawn(async move {
    //     //     tracing::debug!("Point A");
    //     //     scheduler::yield_now().await;
    //     //     tracing::debug!("Point B");
    //     // });
    // let mut aspace = KERNEL_ASPACE.get().unwrap().lock();
    // let mut mmap = UserMmap::new_zeroed(&mut aspace, 2 * 4096, 4096).unwrap();
    //
    // sched.spawn(KERNEL_ASPACE.get().unwrap(), async move {
    //     let ptr = mmap.as_mut_ptr();
    //     unsafe {
    //         ptr.write(17);
    //         assert_eq!(mmap.as_ptr().read(), 17);
    //     }
    //     // unsafe { asm!("ld zero, 0(zero)") };
    // });
    // }

    // wasm::test();

    // - [all][global] parse cmdline
    // - [all][global] `lockup::init()` initialize lockup detector
    // - [all][global] `topology::init()` initialize the system topology
    // - `kernel_shell_init()`
    // - `userboot_init()`
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
