// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(maybe_uninit_slice)]
#![feature(alloc_layout_extra)]

use core::ffi::c_void;
use core::mem;
use core::ops::Range;
use core::ptr::NonNull;

use arrayvec::ArrayVec;
use kmem_core::bootstrap::BootstrapAllocator;
use kmem_core::{AddressRangeExt, Arch, Flush, FrameAllocator, PhysicalAddress, VirtualAddress, KIB};
use loader_api::{BootInfoBuilder, MemoryRegion, MemoryRegionKind};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::{Barrier, OnceLock};

use crate::boot_info::prepare_boot_info;
use crate::error::Error;
use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::mapping::{
    identity_map_self, map_kernel, map_kernel_stacks, StacksAllocation, TlsAllocation,
};

mod arch;
mod boot_info;
mod error;
mod kernel;
mod logger;
mod machine_info;
mod mapping;
mod page_alloc;
mod panic;

pub const ENABLE_KASLR: bool = false;
pub const LOG_LEVEL: log::Level = log::Level::Trace;
pub const STACK_SIZE: usize = 32 * KIB;

pub type Result<T> = core::result::Result<T, Error>;

/// # Safety
///
/// The passed `opaque` ptr must point to a valid memory region.
unsafe fn main(hartid: usize, opaque: *const c_void, boot_ticks: u64) -> ! {
    static GLOBAL_INIT: OnceLock<GlobalInitResult> = OnceLock::new();
    let res = GLOBAL_INIT.get_or_init(|| do_global_init(hartid, opaque));

    // Enable the MMU on all harts. Note that this technically reenables it on the initializing hart
    // but there is no harm in that.
    log::trace!("activating MMU...");
    unsafe {
        res.boot_info.as_ref().address_space.activate();
    }
    log::trace!("activated.");

    if let Some(alloc) = &res.maybe_tls_allocation {
        alloc.initialize_for_hart(hartid);
    }

    // Safety: this will jump to the kernel entry
    unsafe { arch::handoff_to_kernel(hartid, boot_ticks, res) }
}

pub struct GlobalInitResult {
    boot_info: NonNull<loader_api::BootInfo>,
    kernel_entry: VirtualAddress,
    stacks_allocation: StacksAllocation,
    maybe_tls_allocation: Option<TlsAllocation>,
    barrier: Barrier,
}

// Safety: *mut BootInfo isn't Send but `GlobalInitResult` will only ever we read from, so this is fine.
unsafe impl Send for GlobalInitResult {}
// Safety: *mut BootInfo isn't Send but `GlobalInitResult` will only ever we read from, so this is fine.
unsafe impl Sync for GlobalInitResult {}

fn do_global_init(hartid: usize, opaque: *const c_void) -> GlobalInitResult {
    logger::init(LOG_LEVEL.to_level_filter());
    // Safety: TODO
    let minfo = unsafe { MachineInfo::from_dtb(opaque).expect("failed to parse machine info") };
    log::debug!("\n{minfo}");

    arch::start_secondary_harts(hartid, &minfo).unwrap();

    let self_regions = SelfRegions::collect(&minfo);
    log::debug!("{self_regions:#x?}");

    let fdt_phys = Range::from_start_len(
        PhysicalAddress::from_ptr(minfo.fdt.as_ptr()),
        minfo.fdt.len(),
    );

    // initialize the random number generator
    let rng = ENABLE_KASLR.then_some(ChaCha20Rng::from_seed(
        minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
    ));
    let rng_seed = rng.as_ref().map(|rng| rng.get_seed()).unwrap_or_default();
    
    // Initialize the frame allocator
    let allocatable_memories = allocatable_memory_regions(&minfo, &self_regions, fdt_phys.clone());
    log::debug!("allocatable memory regions {allocatable_memories:#x?}");
    let mut frame_alloc = BootstrapAllocator::new(allocatable_memories, 4096); // TODO fix

    let mut flush = Flush::new();
    let mut aspace = arch::init_address_space(&frame_alloc, &mut flush).unwrap();

    // Map the physical memory into kernel address space.
    //
    // This will be used by the kernel to access the page tables, BootInfo struct and maybe
    // more in the future.
    aspace
        .map_physical_memory(&frame_alloc, &mut flush)
        .unwrap();

    // Identity map the loader itself (this binary).
    //
    // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
    // as opposed to m-mode where it would take effect after the jump to s-mode.
    // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
    // We will then unmap the loader in the kernel.
    identity_map_self(&mut aspace, frame_alloc.by_ref(), &self_regions, &mut flush).unwrap();

    // Activate the MMU with the address space we have built so far.
    // the rest of the address space setup will happen in virtual memory (mostly so that we
    // can correctly apply relocations without having to do expensive virt to phys queries)
    log::trace!("activating MMU...");
    let mut aspace = unsafe { aspace.finish_bootstrap_and_activate() };
    log::trace!("activated.");

    flush.flush(aspace.arch());

    let kernel = Kernel::from_static(aspace.arch()).unwrap();
    // print the elf sections for debugging purposes
    log::debug!("\n{kernel}");

    // Initialize the page allocator
    let mut page_alloc = page_alloc::PageAllocator::new(rng);

    let mut flush = Flush::new();

    let (kernel_virt, maybe_tls_allocation) = map_kernel(
        &mut aspace,
        frame_alloc.by_ref(),
        &mut page_alloc,
        &kernel,
        &minfo,
        &mut flush,
    )
    .unwrap();

    log::trace!("KASLR: Kernel image at {:?}", kernel_virt.start);

    let stacks_allocation = map_kernel_stacks(
        &mut aspace,
        frame_alloc.by_ref(),
        &mut page_alloc,
        &minfo,
        usize::try_from(kernel._loader_config.kernel_stack_size_pages).unwrap(),
        &mut flush,
    )
    .unwrap();

    log::debug!(
        "Mapping complete, permanently used {} KiB.",
        frame_alloc.usage() / 1024,
    );

    let mut boot_info_builder = BootInfoBuilder::new(aspace)
        .with_cpu_mask(minfo.hart_mask)
        .with_physical_memory_map(physical_memory_map)
        .with_kernel_virt(kernel_virt.clone())
        .with_kernel_phys(kernel.phys_range())
        .with_rng_seed(rng_seed);

    if let Some(tls_allocation) = &maybe_tls_allocation {
        boot_info_builder = boot_info_builder.with_tls_template(tls_allocation.template.clone());
    }

    for used_region in frame_alloc.used_regions() {
        boot_info_builder = boot_info_builder.with_memory_region(MemoryRegion {
            range: used_region,
            kind: MemoryRegionKind::Loader,
        });
    }

    // Report the free regions as usable.
    for free_region in frame_alloc.free_regions() {
        boot_info_builder = boot_info_builder.with_memory_region(MemoryRegion {
            range: free_region,
            kind: MemoryRegionKind::Usable,
        });
    }

    // Most of the memory occupied by the loader is not needed once the kernel is running,
    // but the kernel itself lies somewhere in the loader memory.
    //
    // We can still mark the range before and after the kernel as usable.
    boot_info_builder = boot_info_builder.with_memory_region(MemoryRegion {
        range: self_regions.total_range().start..kernel.phys_range().start,
        kind: MemoryRegionKind::Usable,
    });
    boot_info_builder = boot_info_builder.with_memory_region(MemoryRegion {
        range: kernel.phys_range().end..self_regions.total_range().end,
        kind: MemoryRegionKind::Usable,
    });

    // Report the flattened device tree as a separate region.
    boot_info_builder = boot_info_builder.with_memory_region(MemoryRegion {
        range: fdt_phys,
        kind: MemoryRegionKind::FDT,
    });

    let kernel_entry = kernel_virt
        .start
        .add(usize::try_from(kernel.elf_file.header.pt2.entry_point()).unwrap());

    GlobalInitResult {
        boot_info: boot_info_builder.finish_and_allocate(frame_alloc).unwrap(),
        kernel_entry,
        maybe_tls_allocation,
        stacks_allocation,
        barrier: Barrier::new(minfo.hart_mask.count_ones() as usize),
    }
}

#[derive(Debug)]
struct SelfRegions {
    pub executable: Range<PhysicalAddress>,
    pub read_only: Range<PhysicalAddress>,
    pub read_write: Range<PhysicalAddress>,
}

impl SelfRegions {
    pub fn collect(minfo: &MachineInfo) -> Self {
        unsafe extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __bss_start: u8;
            static __stack_start: u8;
        }

        SelfRegions {
            executable: Range {
                start: PhysicalAddress::from_ptr(&raw const __text_start),
                end: PhysicalAddress::from_ptr(&raw const __text_end),
            },
            read_only: Range {
                start: PhysicalAddress::from_ptr(&raw const __rodata_start),
                end: PhysicalAddress::from_ptr(&raw const __rodata_end),
            },
            read_write: Range {
                start: PhysicalAddress::from_ptr(&raw const __bss_start),
                end: PhysicalAddress::from_ptr(&raw const __stack_start)
                    .add(minfo.hart_mask.count_ones() as usize * STACK_SIZE),
            },
        }
    }

    pub fn total_range(&self) -> Range<PhysicalAddress> {
        self.executable.start..self.read_write.end
    }
}

fn allocatable_memory_regions(
    minfo: &MachineInfo,
    self_regions: &SelfRegions,
    fdt: Range<PhysicalAddress>,
) -> ArrayVec<Range<PhysicalAddress>, 16> {
    let mut temp: ArrayVec<Range<PhysicalAddress>, 16> = minfo.memories.clone();

    let mut exclude = |to_exclude: Range<PhysicalAddress>| {
        for mut region in mem::take(&mut temp) {
            if to_exclude.contains(&region.start) && to_exclude.contains(&region.end) {
                // remove region
                continue;
            } else if region.contains(&to_exclude.start) && region.contains(&to_exclude.end) {
                temp.push(region.start..to_exclude.start);
                temp.push(to_exclude.end..region.end);
            } else if to_exclude.contains(&region.start) {
                region.start = to_exclude.end;
                temp.push(region);
            } else if to_exclude.contains(&region.end) {
                region.end = to_exclude.start;
                temp.push(region);
            } else {
                temp.push(region);
            }
        }
    };

    exclude(self_regions.executable.start..self_regions.read_write.end);

    exclude(fdt);

    // // merge adjacent regions
    // let mut out: ArrayVec<Range<usize>, 16> = ArrayVec::new();
    // 'outer: for region in temp {
    //     for other in &mut out {
    //         if region.start == other.end {
    //             other.end = region.end;
    //             continue 'outer;
    //         }
    //         if region.end == other.start {
    //             other.start = region.start;
    //             continue 'outer;
    //         }
    //     }
    //
    //     out.push(region);
    // }

    temp
}
