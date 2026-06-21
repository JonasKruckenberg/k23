// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Construct the [`BootInfo`] handed off to the kernel.
//!
//! The loader as it stands today is throwaway scaffolding kept just functional enough to feed the
//! new `BootInfo` surface. The upcoming loader rewrite will replace it wholesale; this file only
//! exists to bridge the gap.

use core::alloc::Layout;
use core::range::Range;

use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind, TlsTemplate};
use mem_core::{PhysMap, PhysicalAddress, VirtualAddress};

use crate::arch;
use crate::frame_alloc::FrameAllocator;

#[expect(clippy::too_many_arguments, reason = "loader is throwaway")]
pub fn prepare_boot_info(
    mut frame_alloc: FrameAllocator,
    physmap: PhysMap,
    kernel_virt: Range<VirtualAddress>,
    maybe_tls_template: Option<TlsTemplate>,
    kernel_debuginfo_phys: Range<PhysicalAddress>,
    fdt_phys: Range<PhysicalAddress>,
    boot_cpu_id: usize,
    boot_ticks: u64,
    rng_seed: [u8; 32],
) -> crate::Result<*mut BootInfo> {
    let phys_off = physmap
        .range_virt()
        .start
        .sub(physmap.range_phys().start.get());
    let frame = frame_alloc.allocate_contiguous_zeroed(
        Layout::from_size_align(arch::PAGE_SIZE, arch::PAGE_SIZE).unwrap(),
        phys_off,
    )?;
    let page = phys_off.add(frame.get());

    let mut boot_info = BootInfo::new(physmap);
    boot_info.boot_cpu_id = boot_cpu_id;
    boot_info.boot_ticks = boot_ticks;
    boot_info.fdt = Some(fdt_phys.start);
    boot_info.kernel_virt = kernel_virt;
    boot_info.tls_template = maybe_tls_template.unwrap_or_default();
    boot_info.kernel_debuginfo_phys = Some(kernel_debuginfo_phys);
    boot_info.rng_seed = rng_seed;
    boot_info.memory_regions = collect_free_regions(&frame_alloc);

    #[expect(
        clippy::cast_ptr_alignment,
        reason = "`page` is actually page aligned, so this is perfectly fine"
    )]
    let boot_info_ptr = page.as_mut_ptr().cast::<BootInfo>();
    // Safety: we just allocated the boot info frame
    unsafe { boot_info_ptr.write(boot_info) }

    Ok(boot_info_ptr)
}

/// Report the remaining free regions reported by the bump allocator as `Usable`.
///
/// The new `BootInfo` schema's `MemoryRegions` is just a sorted, non-overlapping list of physical
/// runs. The kernel reserves its own image/FDT/debuginfo via the dedicated `BootInfo` fields, so
/// we only need to expose the *free* regions here.
fn collect_free_regions(frame_alloc: &FrameAllocator) -> loader_api::MemoryRegions {
    let mut regions: loader_api::MemoryRegions = frame_alloc
        .free_regions()
        .filter(|r| !r.is_empty())
        .map(|range| MemoryRegion {
            range,
            kind: MemoryRegionKind::Usable,
        })
        .collect();

    regions.sort_unstable_by_key(|region| region.range.start);
    regions
}
