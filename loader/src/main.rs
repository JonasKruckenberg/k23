// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(naked_functions)]
#![feature(new_range_api)]
#![feature(maybe_uninit_slice)]
#![feature(alloc_layout_extra)]
#![feature(let_chains)]

use crate::boot_info::prepare_boot_info;
use crate::error::Error;
use crate::frame_alloc::FrameAllocator;
use crate::kernel::{INLINED_KERNEL_BYTES, parse_kernel};
use crate::machine_info::MachineInfo;
use crate::mapping::{
    StacksAllocation, TlsAllocation, identity_map_self, map_kernel, map_kernel_stacks,
    map_physical_memory,
};
use arrayvec::ArrayVec;
use core::ffi::c_void;
use core::range::Range;
use core::slice;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::{Barrier, OnceLock};

mod arch;
mod boot_info;
mod error;
mod frame_alloc;
mod kernel;
mod logger;
mod machine_info;
mod mapping;
mod page_alloc;
mod panic;

pub const ENABLE_KASLR: bool = false;
pub const LOG_LEVEL: log::Level = log::Level::Trace;
pub const STACK_SIZE: usize = 32 * arch::PAGE_SIZE;

pub type Result<T> = core::result::Result<T, Error>;

/// # Safety
///
/// The passed `opaque` ptr must point to a valid memory region.
unsafe fn main(hartid: usize, opaque: *const c_void, boot_ticks: u64) -> ! {
    static GLOBAL_INIT: OnceLock<GlobalInitResult> = OnceLock::new();
    let res = GLOBAL_INIT.get_or_init(|| do_global_init(hartid, opaque));

    // Enable the MMU on all harts. Note that this technically reenables it on the initializing hart
    // but there is no harm in that.
    // Safety: there is no safety
    unsafe {
        log::trace!("activating MMU...");
        arch::activate_aspace(res.root_pgtable);
        log::trace!("activated.");
    }

    if let Some(alloc) = &res.maybe_tls_alloc {
        alloc.initialize_for_hart(hartid);
    }

    // Safety: this will jump to the kernel entry
    unsafe { arch::handoff_to_kernel(hartid, boot_ticks, res) }
}

pub struct GlobalInitResult {
    boot_info: *mut loader_api::BootInfo,
    kernel_entry: usize,
    root_pgtable: usize,
    stacks_alloc: StacksAllocation,
    maybe_tls_alloc: Option<TlsAllocation>,
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

    let fdt_phys = {
        let fdt = minfo.fdt.as_ptr_range();
        Range::from(fdt.start as usize..fdt.end as usize)
    };
    let kernel_phys = {
        let fdt = INLINED_KERNEL_BYTES.0.as_ptr_range();
        Range::from(fdt.start as usize..fdt.end as usize)
    };

    // Initialize the frame allocator
    let allocatable_memories = allocatable_memory_regions(&minfo, &self_regions, fdt_phys);
    log::debug!("allocatable memory regions {allocatable_memories:#x?}");
    let mut frame_alloc = FrameAllocator::new(&allocatable_memories);

    // initialize the random number generator
    let rng = ENABLE_KASLR.then_some(ChaCha20Rng::from_seed(
        minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
    ));
    let rng_seed = rng.as_ref().map(|rng| rng.get_seed()).unwrap_or_default();

    // Initialize the page allocator
    let mut page_alloc = page_alloc::init(rng);

    let root_pgtable = frame_alloc
        .allocate_one_zeroed(
            0, // called before translation into higher half
        )
        .unwrap();

    // Identity map the loader itself (this binary).
    //
    // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
    // as opposed to m-mode where it would take effect after the jump to s-mode.
    // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
    // We will then unmap the loader in the kernel.
    identity_map_self(root_pgtable, &mut frame_alloc, &self_regions).unwrap();

    // Map the physical memory into kernel address space.
    //
    // This will be used by the kernel to access the page tables, BootInfo struct and maybe
    // more in the future.
    let (phys_off, phys_map) =
        map_physical_memory(root_pgtable, &mut frame_alloc, &mut page_alloc, &minfo).unwrap();

    // Activate the MMU with the address space we have built so far.
    // the rest of the address space setup will happen in virtual memory (mostly so that we
    // can correctly apply relocations without having to do expensive virt to phys queries)
    // Safety: there is no safety
    unsafe {
        log::trace!("activating MMU...");
        arch::activate_aspace(root_pgtable);
        log::trace!("activated.");
    }

    // Safety: The kernel elf file is inlined into the loader executable as part of the build setup
    // which means we just need to parse it here.
    let kernel = parse_kernel(unsafe {
        let base = phys_off.checked_add(kernel_phys.start).unwrap();

        slice::from_raw_parts(base as *mut u8, INLINED_KERNEL_BYTES.0.len())
    })
    .unwrap();
    // print the elf sections for debugging purposes
    log::debug!("\n{kernel}");

    let (kernel_virt, maybe_tls_alloc) = map_kernel(
        root_pgtable,
        &mut frame_alloc,
        &mut page_alloc,
        &kernel,
        &minfo,
        phys_off,
    )
    .unwrap();

    log::trace!("KASLR: Kernel image at {:#x}", kernel_virt.start);

    let stacks_alloc = map_kernel_stacks(
        root_pgtable,
        &mut frame_alloc,
        &mut page_alloc,
        &minfo,
        usize::try_from(kernel._loader_config.kernel_stack_size_pages).unwrap(),
        phys_off,
    )
    .unwrap();

    let frame_usage = frame_alloc.frame_usage();
    log::debug!(
        "Mapping complete, permanently used {} KiB.",
        (frame_usage * arch::PAGE_SIZE) / 1024,
    );

    let boot_info = prepare_boot_info(
        frame_alloc,
        phys_off,
        phys_map,
        kernel_virt,
        maybe_tls_alloc.as_ref().map(|alloc| alloc.template.clone()),
        Range::from(self_regions.executable.start..self_regions.read_write.end),
        kernel_phys,
        fdt_phys,
        minfo.hart_mask,
        rng_seed,
    )
    .unwrap();

    let kernel_entry = kernel_virt
        .start
        .checked_add(usize::try_from(kernel.elf_file.header.pt2.entry_point()).unwrap())
        .unwrap();

    GlobalInitResult {
        boot_info,
        kernel_entry,
        root_pgtable,
        maybe_tls_alloc,
        stacks_alloc,
        barrier: Barrier::new(minfo.hart_mask.count_ones() as usize),
    }
}

#[derive(Debug)]
struct SelfRegions {
    pub executable: Range<usize>,
    pub read_only: Range<usize>,
    pub read_write: Range<usize>,
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
                start: &raw const __text_start as usize,
                end: &raw const __text_end as usize,
            },
            read_only: Range {
                start: &raw const __rodata_start as usize,
                end: &raw const __rodata_end as usize,
            },
            read_write: Range {
                start: &raw const __bss_start as usize,
                end: (&raw const __stack_start as usize)
                    + (minfo.hart_mask.count_ones() as usize * STACK_SIZE),
            },
        }
    }
}

fn allocatable_memory_regions(
    minfo: &MachineInfo,
    self_regions: &SelfRegions,
    fdt: Range<usize>,
) -> ArrayVec<Range<usize>, 16> {
    let mut temp: ArrayVec<Range<usize>, 16> = minfo.memories.clone();

    let mut exclude = |to_exclude: Range<usize>| {
        for mut region in temp.take() {
            if to_exclude.contains(&region.start) && to_exclude.contains(&region.end) {
                // remove region
                continue;
            } else if region.contains(&to_exclude.start) && region.contains(&to_exclude.end) {
                temp.push(Range::from(region.start..to_exclude.start));
                temp.push(Range::from(to_exclude.end..region.end));
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

    exclude(Range::from(
        self_regions.executable.start..self_regions.read_write.end,
    ));

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

    temp.sort_unstable_by_key(|region| region.start);

    #[cfg(debug_assertions)]
    for (i, region) in temp.iter().enumerate() {
        for (j, other) in temp.iter().enumerate() {
            if i == j {
                continue;
            }

            assert!(
                !other.contains(&region.start) && !other.contains(&(region.end - 1)),
                "regions {region:#x?} and {other:#x?} overlap"
            );
        }
    }

    temp
}
