// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![feature(thread_local, never_type)]
#![feature(debug_closure_helpers)]
#![expect(internal_features, reason = "panic internals")]
#![feature(std_internals, panic_can_unwind, formatting_options)]
#![feature(step_trait)]
#![feature(box_into_inner)]
#![feature(array_chunks)]
#![feature(iter_array_chunks)]
#![feature(iter_next_chunk)]
#![feature(if_let_guard)]
#![feature(allocator_api)]
#![expect(dead_code, reason = "TODO")] // TODO remove
#![feature(asm_unwind)]

extern crate alloc;
extern crate panic_unwind2;

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

use core::ops::Range;
use core::slice;
use core::time::Duration;

use abort::abort;
use arrayvec::ArrayVec;
use cfg_if::cfg_if;
use kasync::executor::{Executor, Worker};
use kasync::time::{Instant, Ticks, Timer};
use kfastrand::FastRand;
use loader_api::{BootInfo, LoaderConfig, MemoryRegionKind};
use mem::{PhysicalAddress, frame_alloc};
use rand::{RngCore, SeedableRng};
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
fn _start(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    panic_unwind2::set_hook(|info| {
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
    let res = panic_unwind2::catch_unwind(|| {
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
    // perform EARLY per-cpu, architecture-specific initialization
    // (e.g. resetting the FPU)
    arch::per_cpu_init_early();
    tracing::per_cpu_init_early(cpuid);

    let (fdt, fdt_region_phys) = locate_device_tree(boot_info);
    let mut rng = ChaCha20Rng::from_seed(boot_info.rng_seed);

    let global = state::try_init_global(|| {
        // set up the basic functionality of the tracing subsystem as early as possible
        tracing::init_early();

        // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
        // is available
        let allocatable_memories = allocatable_memory_regions(boot_info);
        tracing::info!("allocatable memories: {:?}", allocatable_memories);
        let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

        // initializing the global allocator
        allocator::init(&mut boot_alloc, boot_info);

        let device_tree = DeviceTree::parse(fdt)?;
        tracing::debug!("{device_tree:?}");

        let bootargs = bootargs::parse(&device_tree)?;

        // initialize the backtracing subsystem after the allocator has been set up
        // since setting up the symbolization context requires allocation
        backtrace::init(boot_info, bootargs.backtrace);

        // fully initialize the tracing subsystem now that we can allocate
        tracing::init(bootargs.log);

        // perform global, architecture-specific initialization
        let arch = arch::init();

        // initialize the global frame allocator
        // at this point we have parsed and processed the flattened device tree, so we pass it to the
        // frame allocator for reuse
        let frame_alloc = frame_alloc::init(boot_alloc, fdt_region_phys);

        // initialize the virtual memory subsystem
        mem::init(boot_info, &mut rng, frame_alloc).unwrap();

        // perform LATE per-cpu, architecture-specific initialization
        // (e.g. setting the trap vector and enabling interrupts)
        let cpu = arch::device::cpu::Cpu::new(&device_tree, cpuid)?;

        let executor = Executor::with_capacity(boot_info.cpu_mask.count_ones() as usize).unwrap();
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
            if cpuid == 0 {
                arch::block_on(worker2.run(tests::run_tests(global))).unwrap().exit_if_failed();
            } else {
                arch::block_on(worker2.run(futures::future::pending::<()>())).unwrap_err(); // the only way `run` can return is when the executor is closed
            }
        } else {
            shell::init(
                &global.device_tree,
                &global.executor,
                boot_info.cpu_mask.count_ones() as usize,
            );
            arch::block_on(worker2.run(futures::future::pending::<()>())).unwrap_err(); // the only way `run` can return is when the executor is closed
        }
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
            let range =
                PhysicalAddress::new(region.range.start)..PhysicalAddress::new(region.range.end);

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
        PhysicalAddress::new(fdt.range.start)..PhysicalAddress::new(fdt.range.end),
    )
}
