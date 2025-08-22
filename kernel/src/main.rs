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
extern crate panic_unwind2;

#[cfg(target_arch = "x86_64")]
macro_rules! debug_print {
    ($msg:expr) => {
        for &byte in $msg.as_bytes() {
            unsafe {
                core::arch::asm!(
                    "out dx, al",
                    in("al") byte,
                    in("dx") 0x3f8u16,
                    options(nomem, preserves_flags)
                );
            }
        }
    };
}

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

use crate::backtrace::Backtrace;
use crate::device_tree::DeviceTree;
use crate::mem::bootstrap_alloc::BootstrapAllocator;
use crate::state::{CpuLocal, Global};
use abort::abort;
use arrayvec::ArrayVec;
use cfg_if::cfg_if;
use core::range::Range;
use core::slice;
use fastrand::FastRand;
use kasync::executor::{Executor, Worker};
use kasync::time::{Instant, Ticks};
use loader_api::{BootInfo, LoaderConfig, MemoryRegionKind};
use mem::PhysicalAddress;
use mem::frame_alloc;
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

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

// This is the real kernel entry from the loader
// On x86_64, we need an assembly trampoline to preserve register values
#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
extern "C" fn _start(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    _rust_start_impl(cpuid, boot_info, boot_ticks)
}

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
extern "C" fn _rust_start(cpuid: usize, boot_info_ptr: usize, boot_ticks: u64) -> ! {
    let boot_info = unsafe { &*(boot_info_ptr as *const BootInfo) };
    _rust_start_impl(cpuid, boot_info, boot_ticks)
}

fn _rust_start_impl(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    
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

    // HACK: Skip tracing init for x86_64 for now
    #[cfg(not(target_arch = "x86_64"))]
    tracing::per_cpu_init_early(cpuid);

    let (fdt, fdt_region_phys) = locate_device_tree(boot_info);

    #[cfg(target_arch = "x86_64")]
    debug_print!("after locate_device_tree\n");

    let mut rng = ChaCha20Rng::from_seed(boot_info.rng_seed);

    #[cfg(target_arch = "x86_64")]
    debug_print!("created RNG\n");

    let global = state::try_init_global(|| {
        #[cfg(target_arch = "x86_64")]
        debug_print!("in try_init_global\n");
        // set up the basic functionality of the tracing subsystem as early as possible

        // TODO: Skip tracing init for x86_64 for now
        #[cfg(not(target_arch = "x86_64"))]
        tracing::init_early();

        // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
        // is available
        #[cfg(target_arch = "x86_64")]
        debug_print!("getting allocatable memories\n");
        
        let allocatable_memories = allocatable_memory_regions(boot_info);

        #[cfg(target_arch = "x86_64")]
        debug_print!("got allocatable memories\n");

        // TODO: Skip tracing for x86_64 for now
        #[cfg(not(target_arch = "x86_64"))]
        tracing::info!("allocatable memories: {:?}", allocatable_memories);

        let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

        #[cfg(target_arch = "x86_64")]
        debug_print!("created boot allocator\n");

        // initializing the global allocator
        allocator::init(&mut boot_alloc, boot_info);
        
        #[cfg(target_arch = "x86_64")]
        debug_print!("initialized global allocator\n");

        #[cfg(target_arch = "x86_64")]
        debug_print!("parsing device tree\n");
        
        let device_tree = DeviceTree::parse(fdt)?;
        
        #[cfg(target_arch = "x86_64")]
        debug_print!("parsed device tree\n");
        
        #[cfg(not(target_arch = "x86_64"))]
        tracing::debug!("{device_tree:?}");

        let bootargs = bootargs::parse(&device_tree)?;
        
        #[cfg(target_arch = "x86_64")]
        debug_print!("parsed bootargs\n");

        #[cfg(target_arch = "x86_64")]
        debug_print!("initializing backtrace\n");

        // initialize the backtracing subsystem after the allocator has been set up
        // since setting up the symbolization context requires allocation
        backtrace::init(boot_info, bootargs.backtrace);
        
        #[cfg(target_arch = "x86_64")]
        debug_print!("initialized backtrace\n");

        // fully initialize the tracing subsystem now that we can allocate
        // HACK: Skip for x86_64 for now
        #[cfg(not(target_arch = "x86_64"))]
        tracing::init(bootargs.log);

        #[cfg(target_arch = "x86_64")]
        debug_print!("calling arch::init\n");

        // perform global, architecture-specific initialization
        let arch = arch::init();
        

        // initialize the global frame allocator
        // at this point we have parsed and processed the flattened device tree, so we pass it to the
        // frame allocator for reuse

        debug_print!("calling frame_alloc::init\n");

        let frame_alloc = frame_alloc::init(boot_alloc, fdt_region_phys);


        // initialize the virtual memory subsystem
        debug_print!("calling mem::init\n");

        mem::init(boot_info, &mut rng, frame_alloc).unwrap();

        // perform LATE per-cpu, architecture-specific initialization
        // (e.g. setting the trap vector and enabling interrupts)
        let cpu = arch::device::cpu::Cpu::new(&device_tree, cpuid)?;

        debug_print!("calling Cpu::new\n");

        let executor = Executor::new(boot_info.cpu_mask.count_ones() as usize, cpu.clock.clone());

        debug_print!("calling Executor::new\n");

        Ok(Global {
            time_origin: Instant::from_ticks(&cpu.clock, Ticks(boot_ticks)),
            clock: cpu.clock,
            executor,
            device_tree,
            boot_info,
            arch,
        })
    })
    .unwrap();

    debug_print!("calling init_cpu_local\n");


    // perform LATE per-cpu, architecture-specific initialization
    // (e.g. setting the trap vector and enabling interrupts)
    let arch = arch::per_cpu_init_late(&global.device_tree, cpuid).unwrap();

    state::init_cpu_local(CpuLocal { id: cpuid, arch });

    #[cfg(not(target_arch = "x86_64"))]
    tracing::info!(
        "Booted in ~{:?} ({:?} in k23)",
        Instant::now(&global.clock).duration_since(Instant::ZERO),
        Instant::from_ticks(&global.clock, Ticks(boot_ticks)).elapsed(&global.clock)
    );

    let mut worker = Worker::new(
        &global.executor,
        cpuid,
        arch::Park::new(cpuid),
        FastRand::from_seed(rng.next_u64()),
    );

    cfg_if! {
        if #[cfg(test)] {
            if cpuid == 0 {
                worker.block_on(tests::run_tests(global)).exit_if_failed();
                global.executor.stop();
            } else {
                worker.run();
            }
        } else {
            #[cfg(target_arch = "x86_64")]
            debug_print!("calling shell::init\n");
            
            shell::init(
                &global.device_tree,
                &global.executor,
                boot_info.cpu_mask.count_ones() as usize,
            );
            
            #[cfg(target_arch = "x86_64")]
            debug_print!("calling worker.run\n");
            
            worker.run();
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
    
    #[cfg(target_arch = "x86_64")]
    {
        debug_print!("FDT slice length: ");
        let len = slice.len();
        if len > 0 {
            let mut n = len;
            let mut digits = [0u8; 20];
            let mut i = 0;
            while n > 0 {
                digits[i] = b'0' + (n % 10) as u8;
                n /= 10;
                i += 1;
            }
            while i > 0 {
                i -= 1;
                unsafe {
                    core::arch::asm!(
                        "out dx, al",
                        in("al") digits[i],
                        in("dx") 0x3f8u16,
                        options(nomem, preserves_flags)
                    );
                }
            }
        }
        debug_print!(" bytes\n");
        
        // Check FDT magic number
        if slice.len() >= 4 {
            debug_print!("FDT magic: ");
            for i in 0..4 {
                let byte = slice[i];
                for j in (0..2).rev() {
                    let nibble = (byte >> (j * 4)) & 0xF;
                    let ch = if nibble < 10 {
                        b'0' + nibble as u8
                    } else {
                        b'a' + (nibble - 10) as u8
                    };
                    unsafe {
                        core::arch::asm!(
                            "out dx, al",
                            in("al") ch,
                            in("dx") 0x3f8u16,
                            options(nomem, preserves_flags)
                        );
                    }
                }
            }
            debug_print!("\n");
        }
    }
    (
        slice,
        Range::from(PhysicalAddress::new(fdt.range.start)..PhysicalAddress::new(fdt.range.end)),
    )
}
