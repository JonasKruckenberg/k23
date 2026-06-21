// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![feature(thread_local)]
#![feature(debug_closure_helpers)]
#![feature(iter_next_chunk)]
#![feature(allocator_api)]
#![feature(slice_partition_dedup)]
#![expect(dead_code, reason = "TODO")] // TODO remove

extern crate alloc;
extern crate panic_unwind;

mod allocator;
mod arch;
mod backtrace;
mod bootargs;
mod device_tree;
mod irq;
mod mem;
mod metrics;
mod shell;
mod state;
#[cfg(test)]
mod tests;
mod tracing;
mod util;
mod wasm;

use core::range::Range;
use core::time::Duration;

use abort::abort;
use arrayvec::ArrayVec;
use cfg_if::cfg_if;
use fastrand::FastRand;
use kasync::executor::{Executor, Worker};
use kasync::time::{Instant, Ticks, Timer};
use loader_api::{BootInfo, LoaderConfig};
use mem::frame_alloc;
use mem_core::PhysicalAddress;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

use crate::backtrace::Backtrace;
use crate::device_tree::DeviceTree;
use crate::mem::bootstrap_alloc::BootstrapAllocator;
use crate::state::{CpuLocal, Global};

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

#[used(linker)]
#[unsafe(link_section = ".loader_config")]
static LOADER_CONFIG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = STACK_SIZE_PAGES;
    cfg
};

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    panic_unwind::set_hook(|info| {
        log::error!("CPU {info}");

        // FIXME 32 seems adequate for unoptimized builds where the callstack can get quite deep
        //  but (at least at the moment) is absolute overkill for optimized builds. Sadly there
        //  is no good way to do conditional compilation based on the opt-level.
        const MAX_BACKTRACE_FRAMES: usize = 32;

        match backtrace::__rust_end_short_backtrace(Backtrace::<MAX_BACKTRACE_FRAMES>::capture) {
            Ok(bt) => {
                log::error!("{bt}");

                if bt.frames_omitted {
                    log::warn!(
                        "Stack trace was larger than backtrace buffer, omitted some frames."
                    );
                }
            }
            Err(err) => log::error!("backtrace unavailable: {err}"),
        }
    });

    // Unwinding expects at least one landing pad in the callstack, but capturing all unwinds that
    // bubble up to this point is also a good idea since we can perform some last cleanup and
    // print an error message.
    let res = panic_unwind::catch_unwind(|| {
        backtrace::__rust_begin_short_backtrace(|| kmain(boot_info));
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

fn kmain(boot_info: &'static BootInfo) {
    let cpuid = boot_info.boot_cpu_id;
    let boot_ticks = boot_info.boot_ticks;

    // perform EARLY per-cpu, architecture-specific initialization
    // (e.g. resetting the FPU)
    arch::per_cpu_init_early();
    tracing::per_cpu_init_early(cpuid);

    assert_eq!(
        boot_info.version,
        loader_api::BOOT_INFO_VERSION,
        "loader/kernel BootInfo version mismatch"
    );

    let fdt_phys = boot_info.fdt.expect("loader did not provide FDT");
    let mut rng = ChaCha20Rng::from_seed(boot_info.rng_seed);

    let global = state::try_init_global(|| {
        // set up the basic functionality of the tracing subsystem as early as possible
        tracing::init_early();

        // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
        // is available
        let allocatable_memories = allocatable_memory_regions(boot_info);
        log::info!("allocatable memories: {:?}", allocatable_memories);
        let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

        // initializing the global allocator
        allocator::init(&mut boot_alloc, boot_info);

        // Safety: the loader passes a valid FDT virtual address via the physmap.
        let device_tree =
            unsafe { DeviceTree::from_fdt(boot_info.physmap.phys_to_virt(fdt_phys))? };
        log::debug!("{device_tree:?}");

        let bootargs = bootargs::parse(&device_tree)?;

        // initialize the backtracing subsystem after the allocator has been set up
        // since setting up the symbolization context requires allocation
        backtrace::init(boot_info, bootargs.backtrace);

        // fully initialize the tracing subsystem now that we can allocate
        tracing::init(bootargs.log);

        // perform global, architecture-specific initialization
        let arch = arch::init();

        // Hand the still-free regions (after the initial-heap carve-out) to the frame
        // allocator. `boot_alloc.free_regions()` reports usable bytes that haven't been
        // burned during early boot, so the buddy never re-hands those out.
        let free_regions: loader_api::MemoryRegions = boot_alloc
            .free_regions()
            .map(|range| loader_api::MemoryRegion {
                range,
                kind: loader_api::MemoryRegionKind::Usable,
            })
            .collect();
        let frame_alloc = frame_alloc::init(boot_alloc, free_regions);

        // wire the frame allocator into the kernel heap's OOM handler so the heap can grow
        // automatically. Must come after `frame_alloc::init`; safe to come before `mem::init`
        // since the OOM handler doesn't touch the kernel address space.
        allocator::late_init(frame_alloc, bootargs.heap_max);

        // initialize the virtual memory subsystem
        mem::init(boot_info, &mut rng, frame_alloc).unwrap();

        let cpu = arch::device::cpu::Cpu::new(&device_tree, cpuid)?;

        // single-CPU at handoff is the current contract.
        let executor = Executor::with_capacity(1).unwrap();
        let timer = Timer::new(Duration::from_millis(1), cpu.clock);

        Ok(Global {
            time_origin: Instant::from_ticks(&timer, Ticks(boot_ticks)),
            timer,
            executor,
            device_tree,
            boot_info,
            arch,
        })
    })
    .unwrap();

    // perform LATE per-cpu, architecture-specific initialization
    // (e.g. setting the trap vector and enabling interrupts)
    let arch_state = arch::per_cpu_init_late(&global.device_tree, cpuid).unwrap();

    state::init_cpu_local(CpuLocal {
        id: cpuid,
        arch: arch_state,
    });

    tracing::info!(
        "Booted in ~{:?} ({:?} in k23)",
        Instant::now(&global.timer).duration_since(Instant::ZERO),
        Instant::from_ticks(&global.timer, Ticks(boot_ticks)).elapsed(&global.timer)
    );

    let mut worker2 = Worker::new(&global.executor, FastRand::from_seed(rng.next_u64())).unwrap();

    cfg_if! {
        if #[cfg(test)] {
            arch::block_on(worker2.run(tests::run_tests(global))).unwrap().unwrap().unwrap().exit_if_failed();
        } else {
            shell::init(&global.device_tree, &global.executor, 1);
            arch::block_on(worker2.run(futures::future::pending::<()>())).unwrap().unwrap_err(); // the only way `run` can return is when the executor is closed
        }
    }
}

/// Builds a list of memory regions from the boot info that are usable for allocation.
///
/// The regions handed off by the loader are guaranteed non-overlapping and sorted by start
/// address; this function coalesces contiguous neighbours into one entry.
fn allocatable_memory_regions(
    boot_info: &BootInfo,
) -> ArrayVec<Range<PhysicalAddress>, { loader_api::MAX_MEMORY_REGIONS }> {
    let mut out: ArrayVec<_, _> = boot_info
        .memory_regions
        .iter()
        .filter(|region| region.kind.is_usable())
        .map(|region| region.range)
        .collect();

    // partition_dedup_by keeps the first of each "same" pair; mutating prev to absorb next
    // and returning true folds runs of contiguous regions into a single entry.
    let dedup_len = {
        let (dedup, _) = out.partition_dedup_by(|next, prev| {
            if prev.end == next.start {
                prev.end = next.end;
                true
            } else {
                false
            }
        });
        dedup.len()
    };
    out.truncate(dedup_len);

    out
}
