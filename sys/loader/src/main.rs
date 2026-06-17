// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(ptr_as_uninit)]
#![feature(slice_partition_dedup)]

mod arch;
mod error;
mod frame_alloc;
mod kernel;
mod machine_info;
mod mapping;

use core::range::Range;
use core::slice;
use core::time::Duration;

use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};
use mem_core::{
    AddressRangeExt, Flush, HardwareAddressSpace, PhysMap, PhysicalAddress, VirtualAddress,
};
use uefi::boot::{AllocateType, memory_map};
use uefi::mem::memory_map::{MemoryMap, MemoryMapOwned, MemoryType};

use crate::error::Error;
use crate::frame_alloc::UefiFrameAlloc;
use crate::kernel::{Kernel, RelocatedKernel, StagedKernel};
use crate::machine_info::MachineInfo;

pub type Result<T> = core::result::Result<T, Error>;

#[uefi::entry]
fn main() -> uefi::Status {
    use uefi::prelude::*;

    // Step 1: Initialize UEFI logger
    uefi::helpers::init().unwrap();

    match init() {
        Ok(()) => Status::SUCCESS,
        Err(err) => {
            log::error!("{err}");
            Status::LOAD_ERROR
        }
    }
}

fn init() -> Result<()> {
    arch::init();

    let boot_ticks = arch::get_ticks();

    // Step 1: discover basic machine information required for boot
    let mut minfo = machine_info::discover()?;

    // Step 1b: relocate the firmware tables into loader-owned memory *before* any
    // deep-stack UEFI work (e.g. file I/O) can overrun them or `ExitBootServices`
    // can reclaim them. See `MachineInfo::stage_tables`.
    minfo.stage_tables()?;
    log::info!("{minfo:?}");

    let memory_map = memory_map(MemoryType::LOADER_DATA)?;

    let identity_physmap = PhysMap::new_identity(physical_memory_regions(&memory_map));

    let mut aspace = {
        let arch = mem_core::arch::riscv64::Riscv64Sv39::new(0);
        HardwareAddressSpace::new(arch, &identity_physmap, UefiFrameAlloc)?
    };

    let mut flush = Flush::new();

    // Step 2: locate, parse and validate the kernel ELF from the ESP
    let (kernel_file, debuginfo_file) = kernel::locate()?;
    let kernel = Kernel::from_files(kernel_file, debuginfo_file)?;
    log::debug!("parsed kernel");

    // Step 3: stage (allocate and copy) the kernel into physical memory
    let kernel = kernel.stage()?;
    log::debug!("staged kernel");

    let aspace_layout = layout_kernel_aspace(
        &kernel,
        256 * 4096,
        aspace.granule_size(),
        physical_memory_regions(&memory_map),
        aspace.granule_size(),
    );
    log::debug!("kernel address space layout {aspace_layout:?}");

    // Step 5: relocate the kernel
    let kernel = kernel.relocate(aspace_layout.kernel_image.clone())?;
    log::debug!("relocated kernel");

    // Instantiate the boot hart's TLS block from the relocated image.
    let boot_hart_tls = kernel.instantiate_tls_block()?;

    let boot_hart_stack = instantiate_stack(256 * 4096, aspace.granule_size())?;

    let boot_info = instantiate_boot_info(
        &minfo,
        &aspace_layout,
        &kernel,
        boot_ticks,
        aspace.granule_size(),
    )?;

    log::debug!("mapping kernel...");
    mapping::map_kernel_image(
        &mut aspace,
        &aspace_layout,
        &kernel,
        &identity_physmap,
        &mut flush,
    )?;
    log::debug!("mapped kernel");

    log::debug!("mapping boot hart TLS block...");
    mapping::map_tls_block(
        &mut aspace,
        &aspace_layout,
        boot_hart_tls,
        &identity_physmap,
        &mut flush,
    )?;
    log::debug!("mapped boot hart TLS block");

    log::debug!("mapping boot hart stack...");
    mapping::map_stack(
        &mut aspace,
        &aspace_layout,
        boot_hart_stack,
        &identity_physmap,
        &mut flush,
    )?;
    log::debug!("mapped boot hart stack");

    log::debug!("mapping boot info...");
    mapping::map_boot_info(
        &mut aspace,
        &aspace_layout,
        boot_info,
        &identity_physmap,
        &mut flush,
    )?;
    log::debug!("mapped boot info");

    log::debug!("mapping physical memory...");
    mapping::map_physical_memory(&mut aspace, &aspace_layout, &identity_physmap, &mut flush)?;
    log::debug!("mapped physical memory");

    log::debug!("mapping handoff trampoline...");
    boot_info.handoff_trampoline_virt =
        mapping::map_handoff_trampoline(&mut aspace, &identity_physmap, &mut flush)?;
    log::debug!("mapped handoff trampoline");

    unsafe {
        flush.ignore();
    }

    log::debug!("exiting boot services...");

    let memory_map = unsafe { uefi::boot::exit_boot_services(None) };

    boot_info.memory_regions = collect_memory_regions(memory_map)?;

    unsafe {
        arch::handoff(aspace_layout, &kernel, aspace);
    }

    uefi::boot::stall(Duration::from_secs(10));
    Ok(())
}

fn instantiate_boot_info(
    minfo: &MachineInfo,
    aspace_layout: &KernelAspaceLayout,
    kernel: &RelocatedKernel,
    boot_ticks: u64,
    granule: usize,
) -> crate::Result<&'static mut BootInfo> {
    let boot_info_pages = size_of::<BootInfo>().div_ceil(granule);
    debug_assert!(boot_info_pages > 0);

    let block = uefi::boot::allocate_pages(
        AllocateType::AnyPages,
        MemoryType::RESERVED,
        boot_info_pages,
    )?
    .cast::<BootInfo>();

    let block = unsafe { block.as_uninit_mut() };

    let boot_info = block.write(BootInfo::new(aspace_layout.physmap.clone()));
    boot_info.boot_cpu_id = minfo.boot_hart_id;
    boot_info.boot_ticks = boot_ticks;
    boot_info.rng_seed = minfo.rng_seed;
    boot_info.acpi_rsdp = minfo.raw_rsdp;
    boot_info.fdt = minfo.raw_fdt;
    boot_info.smbios3 = minfo.raw_smbios3;
    boot_info.kernel_virt = aspace_layout.kernel_image.clone();
    boot_info.tls_template = kernel.tls_template().clone();
    boot_info.kernel_debuginfo_phys = kernel.debug_info_phys().clone();

    let (time, rtc_caps) = uefi::runtime::get_time_and_caps()?;
    log::debug!("{time:?} {rtc_caps:?}");

    Ok(boot_info)
}

fn physical_memory_regions<'a>(
    memory_map: &'a MemoryMapOwned,
) -> impl Iterator<Item = Range<PhysicalAddress>> + use<'a> {
    memory_map.entries().map(|desc| {
        let start = PhysicalAddress::new(usize::try_from(desc.phys_start).unwrap());
        let len = usize::try_from(desc.page_count).unwrap() * uefi::boot::PAGE_SIZE;

        Range::from_start_len(start, len)
    })
}

fn collect_memory_regions(memory_map: MemoryMapOwned) -> Result<loader_api::MemoryRegions> {
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

        let start = PhysicalAddress::new(usize::try_from(desc.phys_start).unwrap());
        let len = usize::try_from(desc.page_count).unwrap() * uefi::boot::PAGE_SIZE;

        // TODO preserve the reported memory region attributes (caechable, write through, write combine, write back, etc)

        regions
            .try_push(MemoryRegion {
                range: Range::from_start_len(start, len),
                kind,
            })
            .expect("too many memory regions in memory map"); // TODO error
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

    Ok(regions)
}

fn instantiate_stack(boot_hart_stack_size: usize, granule: usize) -> Result<&'static mut [u8]> {
    let stack_size = boot_hart_stack_size.next_multiple_of(granule);
    let stack_pages = stack_size / granule;
    debug_assert!(stack_pages > 0);

    let block =
        uefi::boot::allocate_pages(AllocateType::AnyPages, MemoryType::RESERVED, stack_pages)?;

    {
        let block = unsafe {
            slice::from_raw_parts_mut(block.as_ptr().cast::<u64>(), stack_size / size_of::<u64>())
        };

        block.fill(0xACE0BACE);
    }

    let block = unsafe { slice::from_raw_parts_mut(block.as_ptr(), stack_size) };

    Ok(block)
}

#[derive(Debug)]
struct KernelAspaceLayout {
    pub physmap: PhysMap,
    pub kernel_image: Range<VirtualAddress>,
    pub boot_hart_tls: Range<VirtualAddress>,
    pub boot_hart_stack: Range<VirtualAddress>,
    pub boot_info: Range<VirtualAddress>,
}

fn layout_kernel_aspace(
    kernel: &StagedKernel,
    boot_hart_stack_size: usize,
    stack_guard_region: usize,
    physical_memory_regions: impl Iterator<Item = Range<PhysicalAddress>>,
    granule: usize,
) -> KernelAspaceLayout {
    const BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000);

    let physmap = PhysMap::new(BASE, physical_memory_regions);

    let kernel_image =
        Range::from_start_len(physmap.range_virt().end, kernel.size()).align_out(granule);

    let boot_hart_tls =
        Range::from_start_len(kernel_image.end, kernel.tls_template().mem_size).align_out(granule);

    let boot_hart_stack: Range<VirtualAddress> =
        Range::from_start_len(boot_hart_tls.end, boot_hart_stack_size).align_out(granule);

    let boot_info = Range::from_start_len(
        boot_hart_stack.end.add(stack_guard_region),
        size_of::<BootInfo>(),
    )
    .align_out(granule);

    KernelAspaceLayout {
        physmap,
        kernel_image,
        boot_hart_tls,
        boot_hart_stack,
        boot_info,
    }
}
