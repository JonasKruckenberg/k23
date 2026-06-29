// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![feature(ptr_as_uninit)]
#![feature(never_type)]

mod arch;
mod error;
pub mod fdt;
mod kernel;
mod machine_info;
mod mapping;

use core::alloc::Layout;
use core::range::Range;
use core::slice;

pub use arch::disable_interrupts;
pub use error::Error;
pub use kernel::ImageSource;
use loader_api::{BootInfo, MemoryRegion, UartInfo};
pub use machine_info::{DiscoveredUart, MachineInfo};
use mem_core::{AddressRangeExt, FrameAllocator, PhysMap, PhysicalAddress, VirtualAddress};
use mem_mmu::{Flush, HardwareAddressSpace, Size1GiB};

use crate::kernel::{Kernel, RelocatedKernel, StagedKernel};

pub type Result<T> = core::result::Result<T, Error>;

const MEMORY_REGIONS_MAX: usize = 128;
const BOOT_HART_STACK_SIZE: usize = 256 * 4096;

/// Setup machine environment and boot the provided kernel.
///
/// # Errors
///
/// If parsing firmware tables, parsing or booting the kernel,
/// establishing the kernel address space or anything else fails `Err` will
/// be returned.
///
/// The caller should deal with this failure as appropriate. E.g. aborting the boot process,
/// or choosing a different kernel image.
pub fn boot<S: ImageSource, A: FrameAllocator>(
    minfo: &MachineInfo,
    physical_memory_regions: &[MemoryRegion],
    kernel: S,
    debug_info: Option<S>,
    frame_alloc: A,
    finalize: impl FnOnce(A) -> loader_api::MemoryRegions,
) -> crate::Result<!> {
    let boot_ticks = arch::get_ticks();

    let identity_physmap =
        PhysMap::new_identity::<Size1GiB>(physical_memory_regions.iter().map(|r| r.range));
    let arch = mem_core::arch::riscv64::Riscv64Sv39::new(0);
    let mut aspace = HardwareAddressSpace::new(arch, &identity_physmap, &frame_alloc)?;
    let mut flush = Flush::new();
    let granule = aspace.granule_size();

    // Step 2: parse and validate the kernel ELF
    log::debug!("parsing kernel...");
    let kernel = Kernel::from_parts(kernel, debug_info, granule)?;
    log::debug!("parsed kernel");

    // Step 3: stage (allocate and copy) the kernel into physical memory
    log::debug!("staging kernel...");
    let kernel = kernel.stage(&frame_alloc, granule)?;
    log::debug!("staged kernel");

    let aspace_layout = layout_kernel_aspace(
        &kernel,
        BOOT_HART_STACK_SIZE,
        aspace.granule_size(),
        physical_memory_regions.iter().map(|r| r.range),
        minfo.uart.map(|uart| uart.regs),
        aspace.granule_size(),
    );
    log::debug!("kernel address space layout {aspace_layout:?}");

    // Step 5: relocate the kernel
    let kernel = kernel.relocate(aspace_layout.kernel_image)?;
    log::debug!("relocated kernel");

    // Step 6: instantiate resources
    let boot_hart_tls = kernel.instantiate_tls_block(&frame_alloc, granule)?;

    let boot_hart_stack =
        instantiate_stack(BOOT_HART_STACK_SIZE, &frame_alloc, aspace.granule_size())?;

    let boot_info = instantiate_boot_info(
        minfo,
        &aspace_layout,
        &kernel,
        boot_ticks,
        &frame_alloc,
        aspace.granule_size(),
    )?;

    // Step 7: map kernel & resources into the kernel address space
    log::debug!("mapping kernel...");
    mapping::map_kernel_image(
        &mut aspace,
        &aspace_layout,
        &kernel,
        &identity_physmap,
        &frame_alloc,
        &mut flush,
    )?;
    log::debug!("mapped kernel");

    log::debug!("mapping boot hart TLS block...");
    mapping::map_tls_block(
        &mut aspace,
        &aspace_layout,
        boot_hart_tls,
        &identity_physmap,
        &frame_alloc,
        &mut flush,
    )?;
    log::debug!("mapped boot hart TLS block");

    log::debug!("mapping boot hart stack...");
    mapping::map_stack(
        &mut aspace,
        &aspace_layout,
        boot_hart_stack,
        &identity_physmap,
        &frame_alloc,
        &mut flush,
    )?;
    log::debug!("mapped boot hart stack");

    log::debug!("mapping boot info...");
    mapping::map_boot_info(
        &mut aspace,
        &aspace_layout,
        boot_info,
        &identity_physmap,
        &frame_alloc,
        &mut flush,
    )?;
    log::debug!("mapped boot info");

    if let (Some(virt), Some(uart)) = (aspace_layout.uart, minfo.uart) {
        log::debug!("mapping UART {} => {}...", uart.regs.start, virt.start);
        mapping::map_uart(
            &mut aspace,
            virt,
            uart.regs,
            &identity_physmap,
            &frame_alloc,
            &mut flush,
        )?;
        log::debug!("mapped UART");
    }

    log::debug!("mapping physical memory...");
    mapping::map_physical_memory(
        &mut aspace,
        &aspace_layout,
        &identity_physmap,
        &frame_alloc,
        &mut flush,
    )?;
    log::debug!("mapped physical memory");

    log::debug!("mapping handoff trampoline...");
    boot_info.handoff_trampoline_virt =
        mapping::map_handoff_trampoline(&mut aspace, &identity_physmap, &frame_alloc, &mut flush)?;
    log::debug!("mapped handoff trampoline");

    // Safety: we're flushing the entire aspace in `arch::handoff` anyway
    unsafe {
        flush.ignore();
    }

    boot_info.memory_regions = finalize(frame_alloc);

    // Safety: we have
    // 1. loaded and mapped the kernel into memory, applied relocations
    // 2. initialized the boot hart stack & tls block
    // 3. mapped physical memory
    // 4. mapped the discovered UART MMIO region
    // 5. disabled interrupts during init
    unsafe {
        arch::handoff(aspace_layout, &kernel, aspace);
    }

    // NB: handoff cannot return
}

fn instantiate_stack(
    stack_size: usize,
    frame_alloc: &impl FrameAllocator,
    granule: usize,
) -> Result<&'static mut [u8]> {
    let block =
        frame_alloc.allocate_contiguous(Layout::from_size_align(stack_size, granule).unwrap())?;

    #[expect(
        clippy::cast_ptr_alignment,
        reason = "`allocate` produces page-size aligned allocations (4KiB), so block is trivially aligned to u64"
    )]
    {
        // Safety: we just allocated the memory, `allocate_pages` ensures the memory range is valid and initialized
        let block = unsafe {
            slice::from_raw_parts_mut(
                block.as_mut_ptr().cast::<u64>(),
                stack_size / size_of::<u64>(),
            )
        };

        block.fill(0xACE0BACE);
    }

    // Safety: we just allocated the memory, `allocate_pages` ensures the memory range is valid and initialized
    let block = unsafe { slice::from_raw_parts_mut(block.as_mut_ptr(), stack_size) };

    Ok(block)
}

fn instantiate_boot_info(
    minfo: &MachineInfo,
    aspace_layout: &KernelAspaceLayout,
    kernel: &RelocatedKernel,
    boot_ticks: u64,
    frame_alloc: &impl FrameAllocator,
    granule: usize,
) -> crate::Result<&'static mut BootInfo> {
    let block = frame_alloc
        .allocate_contiguous(Layout::from_size_align(size_of::<BootInfo>(), granule).unwrap())?;

    // Safety: we just allocated the memory, `allocate_pages` ensures the memory range is valid and initialized
    let block = unsafe {
        block
            .as_non_null()
            .unwrap()
            .cast::<BootInfo>()
            .as_uninit_mut()
    };

    let boot_info = block.write(BootInfo::new(aspace_layout.physmap.clone()));
    boot_info.boot_cpu_id = minfo.boot_hart_id;
    boot_info.boot_ticks = boot_ticks;
    boot_info.rng_seed = minfo.rng_seed;
    boot_info.firmware_tables = minfo.firmware_tables.clone();
    boot_info.uart = build_uart_info(minfo.uart, aspace_layout.uart, granule);
    boot_info.kernel_virt = aspace_layout.kernel_image;
    boot_info.tls_template = kernel.tls_template().clone();
    boot_info.kernel_debuginfo_phys = kernel.debug_info_phys();

    // let (time, rtc_caps) = uefi::runtime::get_time_and_caps()?;
    // log::debug!("{time:?} {rtc_caps:?}");

    Ok(boot_info)
}

/// Combine the discovered UART (physical register block + driver params) with
/// the virtual range reserved for it into the [`UartInfo`] handed to the kernel.
///
/// The register block may start at a sub-page offset within its mapping; that
/// offset is preserved so `regs.start` points at the actual registers.
fn build_uart_info(
    discovered: Option<machine_info::DiscoveredUart>,
    virt: Option<Range<VirtualAddress>>,
    granule: usize,
) -> Option<UartInfo> {
    let (discovered, virt) = discovered.zip(virt)?;
    let page_offset = discovered.regs.start.get() - discovered.regs.start.align_down(granule).get();

    Some(UartInfo {
        regs: Range::from_start_len(virt.start.add(page_offset), discovered.regs.len()),
        clock_frequency: discovered.clock_frequency,
        baud_rate: discovered.baud_rate,
        reg_shift: discovered.reg_shift,
        reg_io_width: discovered.reg_io_width,
        irq_num: discovered.irq_num,
    })
}

#[derive(Debug)]
struct KernelAspaceLayout {
    pub physmap: PhysMap,
    pub kernel_image: Range<VirtualAddress>,
    pub boot_hart_tls: Range<VirtualAddress>,
    pub boot_hart_stack: Range<VirtualAddress>,
    pub boot_info: Range<VirtualAddress>,
    pub uart: Option<Range<VirtualAddress>>,
}

fn layout_kernel_aspace(
    kernel: &StagedKernel,
    boot_hart_stack_size: usize,
    stack_guard_region: usize,
    physical_memory_regions: impl ExactSizeIterator<Item = Range<PhysicalAddress>>,
    uart_phys: Option<Range<PhysicalAddress>>,
    granule: usize,
) -> KernelAspaceLayout {
    const BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000);

    let physmap = PhysMap::new::<Size1GiB>(BASE, physical_memory_regions);

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

    let uart = uart_phys.map(|regs| {
        let len = regs.align_out(granule).len();
        Range::from_start_len(boot_info.end, len).align_out(granule)
    });

    KernelAspaceLayout {
        physmap,
        kernel_image,
        boot_hart_tls,
        boot_hart_stack,
        boot_info,
        uart,
    }
}
