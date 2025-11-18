// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::Range;
use core::ptr::NonNull;

use arrayvec::ArrayVec;
use kmem_core::bootstrap::BootstrapAllocator;
use kmem_core::{AddressSpace, FrameAllocator, PhysicalAddress, VirtualAddress};
use loader_api::{BootInfo, BootInfoBuilder, MemoryRegion, MemoryRegionKind, TlsTemplate};

#[expect(clippy::too_many_arguments, reason = "")]
pub fn prepare_boot_info(
    address_space: AddressSpace,
    frame_alloc: BootstrapAllocator<spin::RawMutex>,
    physical_memory_map: Range<VirtualAddress>,
    maybe_tls_template: Option<TlsTemplate>,
    loader_phys: Range<PhysicalAddress>,
    kernel_phys: Range<PhysicalAddress>,
    kernel_virt: Range<VirtualAddress>,
    fdt_phys: Range<PhysicalAddress>,
    hart_mask: usize,
    rng_seed: [u8; 32],
) -> crate::Result<NonNull<BootInfo>> {
    let mut memory_regions = ArrayVec::new();

    for used_region in frame_alloc.used_regions() {
        memory_regions.push(MemoryRegion {
            range: used_region,
            kind: MemoryRegionKind::Loader,
        });
    }

    // Report the free regions as usable.
    for free_region in frame_alloc.free_regions() {
        memory_regions.push(MemoryRegion {
            range: free_region,
            kind: MemoryRegionKind::Usable,
        });
    }

    // Most of the memory occupied by the loader is not needed once the kernel is running,
    // but the kernel itself lies somewhere in the loader memory.
    //
    // We can still mark the range before and after the kernel as usable.
    memory_regions.push(MemoryRegion {
        range: loader_phys.start..kernel_phys.start,
        kind: MemoryRegionKind::Usable,
    });
    memory_regions.push(MemoryRegion {
        range: kernel_phys.end..loader_phys.end,
        kind: MemoryRegionKind::Usable,
    });

    // Report the flattened device tree as a separate region.
    memory_regions.push(MemoryRegion {
        range: fdt_phys,
        kind: MemoryRegionKind::FDT,
    });

    BootInfoBuilder::new(address_space)
        .with_cpu_mask(hart_mask)
        .with_physical_memory_map(physical_memory_map)
        .with_kernel_virt(kernel_virt)
        .with_kernel_phys(kernel_phys)
        .with_rng_seed(rng_seed)
        
        .with_memory_region(maybe_tls_template)
        
        .finish_and_allocate(frame_alloc.by_ref())
        .map_err(Into::into)
}
