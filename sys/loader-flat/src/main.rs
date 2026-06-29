// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(unwrap_infallible)]
#![feature(slice_partition_dedup)]

mod arch;
mod error;
mod frame_alloc;
mod kernel;
mod logger;
mod machine_info;
mod panic_handler;

use core::alloc::{GlobalAlloc, Layout};

pub use error::Error;
use mem_core::PhysicalAddress;

use crate::frame_alloc::BumpAllocator;

pub const STACK_SIZE: usize = 128 * arch::PAGE_SIZE;

pub type Result<T> = core::result::Result<T, Error>;

/// # Safety
///
/// The passed `opaque` ptr must point to a valid memory region.
unsafe fn main(boot_hart_id: usize, dtb: PhysicalAddress, _boot_ticks: u64) -> ! {
    loader_common::disable_interrupts();

    logger::init();

    let (minfo, memory_regions) = machine_info::from_dtb::<{ loader_api::MAX_MEMORY_REGIONS }>(
        dtb,
        boot_hart_id,
        arch::PAGE_SIZE,
    )
    .unwrap();

    let (kernel, debug_info) = kernel::locate();

    let frame_alloc =
        BumpAllocator::new::<mem_core::arch::riscv64::Riscv64Sv39>(memory_regions.clone());

    let finalize = |frame_alloc: BumpAllocator<
        spin::RawMutex,
        { loader_api::MAX_MEMORY_REGIONS },
    >|
     -> loader_api::MemoryRegions {
        let mut regions: loader_api::MemoryRegions = frame_alloc
            .used_regions()
            .into_iter()
            .chain(frame_alloc.free_regions())
            .filter(|r| !r.range.is_empty())
            .collect();

        regions.sort_unstable_by_key(|region| region.range.start);

        // merge adjacent regions IFF they have the same attributes
        let (coalesced, _) = regions.partition_dedup_by(|a, b| {
            if a.kind == b.kind && a.range.start <= b.range.end {
                b.range.end = b.range.end.max(a.range.end);
                true
            } else {
                false
            }
        });
        let n = coalesced.len();
        regions.truncate(n);

        log::debug!("finalized regions {regions:?}");

        regions
    };

    let err = loader_common::boot(
        &minfo,
        memory_regions.as_slice(),
        kernel,
        Some(debug_info),
        frame_alloc,
        finalize,
    )
    .into_err();

    log::error!("failed to boot kernel {err}");

    abort::abort()
}

struct PanicAllocator;
// Safety: panicking stub, must never be actually called
unsafe impl GlobalAlloc for PanicAllocator {
    unsafe fn alloc(&self, _: Layout) -> *mut u8 {
        panic!("heap alloc in loader-flat")
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        panic!()
    }
}

#[global_allocator]
static A: PanicAllocator = PanicAllocator;
