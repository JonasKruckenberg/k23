// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::error::Error;
use crate::frame_alloc::FrameAllocator;
use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::range::Range;
use core::slice;
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind, MemoryRegions, TlsTemplate};

pub fn prepare_boot_info(
    mut frame_alloc: FrameAllocator,
    physical_address_offset: usize,
    physical_memory_map: Range<usize>,
    kernel_virt: Range<usize>,
    maybe_tls_template: Option<TlsTemplate>,
    loader_phys: Range<usize>,
    kernel_phys: Range<usize>,
    fdt_phys: Range<usize>,
    boot_ticks: u64,
) -> crate::Result<*mut BootInfo> {
    let frame = frame_alloc
        .allocate_contiguous_zeroed(
            Layout::from_size_align(arch::PAGE_SIZE, arch::PAGE_SIZE).unwrap(),
        )
        .ok_or(Error::NoMemory)?;
    let page = physical_address_offset.checked_add(frame).unwrap();

    let memory_regions =
        init_boot_info_memory_regions(page, frame_alloc, fdt_phys, loader_phys.clone());

    let mut boot_info = BootInfo::new(memory_regions);
    boot_info.physical_address_offset = physical_address_offset;
    boot_info.physical_memory_map = physical_memory_map;
    boot_info.tls_template = maybe_tls_template;
    boot_info.kernel_virt = kernel_virt;
    boot_info.kernel_phys = kernel_phys;
    boot_info.boot_ticks = boot_ticks;

    let boot_info_ptr = page as *mut BootInfo;
    unsafe { boot_info_ptr.write(boot_info) }

    Ok(boot_info_ptr)
}

fn init_boot_info_memory_regions(
    page: usize,
    frame_alloc: FrameAllocator,
    fdt_phys: Range<usize>,
    loader_phys: Range<usize>,
) -> MemoryRegions {
    let regions: &mut [MaybeUninit<MemoryRegion>] = unsafe {
        let base = page.checked_add(size_of::<BootInfo>()).unwrap();
        let len = (arch::PAGE_SIZE - size_of::<BootInfo>()) / size_of::<MemoryRegion>();

        slice::from_raw_parts_mut(base as *mut MaybeUninit<MemoryRegion>, len)
    };

    let mut len = 0;
    let mut push_region = |region: MemoryRegion| {
        regions[len].write(region);
        len += 1;
    };

    // Report the memory we consumed during startup as used.
    for used_region in frame_alloc.used_regions() {
        push_region(MemoryRegion {
            range: used_region,
            kind: MemoryRegionKind::Loader,
        });
    }

    // Report the free regions as usable.
    for free_region in frame_alloc.free_regions() {
        push_region(MemoryRegion {
            range: free_region,
            kind: MemoryRegionKind::Usable,
        });
    }

    // The memory occupied by the loader is not needed once the kernel is running.
    // Mark it as usable.
    push_region(MemoryRegion {
        range: loader_phys,
        kind: MemoryRegionKind::Usable,
    });

    // Report the flattened device tree as a separate region.
    push_region(MemoryRegion {
        range: fdt_phys,
        kind: MemoryRegionKind::FDT,
    });

    // Truncate the slice to include only initialized elements
    let regions = unsafe { MaybeUninit::slice_assume_init_mut(&mut regions[0..len]) };

    // Sort the memory regions by start address, we do this now in the loader
    // because the BootInfo struct will be passed as a read-only static reference to the kernel.
    regions.sort_unstable_by_key(|region| region.range.start);

    MemoryRegions::from(regions)
}
