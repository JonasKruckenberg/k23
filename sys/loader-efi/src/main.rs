// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(unwrap_infallible)]
#![feature(never_type)]
#![feature(slice_partition_dedup)]

extern crate alloc;

mod error;
mod frame_alloc;
mod kernel;
mod logger;
mod machine_info;
mod panic_handler;

use alloc::vec::Vec;
use core::range::Range;

pub use error::Error;
use loader_api::{MemoryRegion, MemoryRegionKind};
use mem_core::{AddressRangeExt, PhysicalAddress};
use uefi::boot::memory_map;
use uefi::mem::memory_map::{MemoryMap, MemoryMapOwned, MemoryType};

use crate::frame_alloc::UefiFrameAlloc;

pub type Result<T> = core::result::Result<T, Error>;

#[uefi::entry]
fn main() -> uefi::Status {
    use uefi::prelude::*;

    let err = run().into_err();

    log::error!("failed to boot kernel: {err}");
    Status::LOAD_ERROR
}

/// Runs the loader: discovers the machine, locates the kernel, and hands off.
///
/// On success this never returns (control passes to the kernel).
///
/// # Errors
///
/// Returns an error if UEFI allocator init, machine/kernel discovery, or the
/// shared loader handoff fails.
fn run() -> Result<!> {
    loader_common::disable_interrupts();

    // init UEFI allocator
    uefi::helpers::init()?;

    logger::init();

    let physical_memory_regions = discover_physical_memory();
    let minfo = machine_info::discover()?;

    let (kernel, debug_info) = kernel::locate()?;

    let finalize = |_frame_alloc: UefiFrameAlloc| -> loader_api::MemoryRegions {
        log::debug!("exiting boot services...");

        // Safety: we're not holding references to miscellaneous boot service allocations
        // and the allocations we do preserve are accounted for by `collect_memory_regions`
        let memory_map = unsafe { uefi::boot::exit_boot_services(None) };

        collect_memory_regions(memory_map)
    };

    loader_common::boot(
        &minfo,
        physical_memory_regions.as_slice(),
        kernel,
        debug_info,
        UefiFrameAlloc,
        finalize,
    )?
}

/// Returns `true` while UEFI boot services are still active.
///
/// After `exit_boot_services` the firmware clears the boot-services pointer in
/// the system table, so this returns `false` and callers must not touch
/// boot-services-backed resources (console protocols, the firmware allocator).
pub(crate) fn are_boot_services_active() -> bool {
    let Some(st) = uefi::table::system_table_raw() else {
        return false;
    };

    // Safety: valid per requirements of `set_system_table`.
    let st = unsafe { st.as_ref() };

    !st.boot_services.is_null()
}

fn discover_physical_memory() -> Vec<MemoryRegion> {
    memory_map(MemoryType::LOADER_DATA)
        .unwrap()
        .entries()
        .map(|desc| {
            let kind = match desc.ty {
                MemoryType::RESERVED | MemoryType::UNUSABLE => MemoryRegionKind::Unusable,
                MemoryType::LOADER_CODE
                | MemoryType::LOADER_DATA
                | MemoryType::BOOT_SERVICES_CODE
                | MemoryType::BOOT_SERVICES_DATA
                | MemoryType::CONVENTIONAL => MemoryRegionKind::Usable,

                MemoryType::RUNTIME_SERVICES_CODE | MemoryType::RUNTIME_SERVICES_DATA => {
                    MemoryRegionKind::Usable
                }

                // TODO handle MMIO, and other memory region types here instead of defaulting to unusable
                _ => MemoryRegionKind::Unusable,
            };

            let start = PhysicalAddress::new(usize::try_from(desc.phys_start).unwrap());
            let len = usize::try_from(desc.page_count).unwrap() * uefi::boot::PAGE_SIZE;
            let range = Range::from_start_len(start, len);

            MemoryRegion { range, kind }
        })
        .collect()
}

fn collect_memory_regions(memory_map: MemoryMapOwned) -> loader_api::MemoryRegions {
    let mut regions = loader_api::MemoryRegions::new();

    for desc in memory_map.entries() {
        let kind = match desc.ty {
            MemoryType::RESERVED | MemoryType::UNUSABLE => MemoryRegionKind::Unusable,
            MemoryType::LOADER_CODE
            | MemoryType::LOADER_DATA
            | MemoryType::BOOT_SERVICES_CODE
            | MemoryType::BOOT_SERVICES_DATA
            | MemoryType::CONVENTIONAL => MemoryRegionKind::Usable,

            MemoryType::RUNTIME_SERVICES_CODE | MemoryType::RUNTIME_SERVICES_DATA => {
                MemoryRegionKind::Usable
            }

            // TODO handle MMIO, and other memory region types here instead of defaulting to unusable
            _ => MemoryRegionKind::Unusable,
        };

        // Runs post-`exit_boot_services`, where a panic has no clean recovery. On all
        // supported (64-bit) targets `usize == u64` so these conversions never fail;
        // skip any region we somehow can't represent rather than panicking.
        let Ok(start) = usize::try_from(desc.phys_start) else {
            debug_assert!(false, "phys_start {:#x} exceeds usize", desc.phys_start);
            continue;
        };
        let Ok(page_count) = usize::try_from(desc.page_count) else {
            debug_assert!(false, "page_count {} exceeds usize", desc.page_count);
            continue;
        };
        let start = PhysicalAddress::new(start);
        let len = page_count * uefi::boot::PAGE_SIZE;

        // TODO preserve the reported memory region attributes (caechable, write through, write combine, write back, etc)
        // /// Supports marking as uncacheable.
        // const UNCACHEABLE = 0x1;
        // /// Supports write-combining.
        // const WRITE_COMBINE = 0x2;
        // /// Supports write-through.
        // const WRITE_THROUGH = 0x4;
        // /// Support write-back.
        // const WRITE_BACK = 0x8;
        // /// Supports marking as uncacheable, exported and
        // /// supports the "fetch and add" semaphore mechanism.
        // const UNCACHABLE_EXPORTED = 0x10;
        // /// Supports write-protection.
        // const WRITE_PROTECT = 0x1000;
        // /// Supports read-protection.
        // const READ_PROTECT = 0x2000;
        // /// Supports disabling code execution.
        // const EXECUTE_PROTECT = 0x4000;

        regions.push(MemoryRegion {
            range: Range::from_start_len(start, len),
            kind,
        });
    }

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

    regions
}
