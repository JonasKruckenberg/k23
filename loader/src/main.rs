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

use crate::boot_info::prepare_boot_info;
use crate::error::Error;
use crate::frame_alloc::FrameAllocator;
use crate::kernel::{parse_kernel, INLINED_KERNEL_BYTES};
use crate::machine_info::MachineInfo;
use crate::mapping::{identity_map_self, map_kernel, map_physical_memory};
use arrayvec::ArrayVec;
use core::alloc::Layout;
use core::ffi::c_void;
use core::range::Range;
use core::{ptr, slice};

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

pub type Result<T> = core::result::Result<T, Error>;

/// # Safety
///
/// The passed `opaque` ptr must point to a valid memory region.
pub unsafe fn main(hartid: usize, opaque: *const c_void, boot_ticks: u64) -> ! {
    // zero out the BSS section
    unsafe extern "C" {
        static mut __bss_zero_start: u64;
        static mut __bss_end: u64;
    }
    unsafe {
        // Zero BSS section
        let mut ptr = &raw mut __bss_zero_start;
        let end = &raw mut __bss_end;
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }

    logger::init(LOG_LEVEL.to_level_filter());

    let minfo = unsafe { MachineInfo::from_dtb(opaque).expect("failed to parse machine info") };
    log::debug!("\n{minfo}");

    let self_regions = SelfRegions::collect();
    log::debug!("{self_regions:#x?}");

    // Initialize the frame allocator
    let allocatable_memories = allocatable_memory_regions(&minfo, &self_regions);
    let mut frame_alloc = FrameAllocator::new(&allocatable_memories);

    // Initialize the page allocator
    let mut page_alloc = page_alloc::init(&minfo);

    let fdt_phys = allocate_and_copy(&mut frame_alloc, minfo.fdt).unwrap();
    let kernel_phys = allocate_and_copy(&mut frame_alloc, &INLINED_KERNEL_BYTES.0).unwrap();

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
    unsafe {
        log::trace!("activating MMU...");
        arch::activate_aspace(root_pgtable);
        log::trace!("activated.");
    }
    frame_alloc.set_phys_offset(phys_off);

    // The kernel elf file is inlined into the loader executable as part of the build setup
    // which means we just need to parse it here.
    let kernel = parse_kernel(unsafe {
        let base = phys_off.checked_add(kernel_phys.start).unwrap();
        let len = kernel_phys.end.checked_sub(kernel_phys.start).unwrap();

        slice::from_raw_parts(base as *mut u8, len)
    })
    .unwrap();
    // print the elf sections for debugging purposes
    log::debug!("\n{kernel}");

    let (kernel_virt, maybe_tls_template) = map_kernel(
        root_pgtable,
        &mut frame_alloc,
        &mut page_alloc,
        &kernel,
        phys_off,
    )
    .unwrap();

    log::trace!("KASLR: Kernel image at {:#x}", kernel_virt.start);

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
        maybe_tls_template,
        Range::from(self_regions.executable.start..self_regions.read_write.end),
        kernel_phys,
        fdt_phys,
        boot_ticks,
    )
    .unwrap();

    let kernel_entry = kernel_virt
        .start
        .checked_add(usize::try_from(kernel.elf_file.header.pt2.entry_point()).unwrap())
        .unwrap();

    unsafe { arch::handoff_to_kernel(hartid, boot_info, kernel_entry) }
}

#[derive(Debug)]
struct SelfRegions {
    pub executable: Range<usize>,
    pub read_only: Range<usize>,
    pub read_write: Range<usize>,
}

impl SelfRegions {
    pub fn collect() -> Self {
        unsafe extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __bss_start: u8;
            static __data_end: u8;
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
                end: &raw const __data_end as usize,
            },
        }
    }
}

fn allocatable_memory_regions(
    minfo: &MachineInfo,
    self_regions: &SelfRegions,
) -> ArrayVec<Range<usize>, 16> {
    let mut out = ArrayVec::new();
    let to_exclude = Range::from(self_regions.executable.start..self_regions.read_write.end);

    for mut region in minfo.memories.clone() {
        if to_exclude.contains(&region.start) && to_exclude.contains(&region.end) {
            // remove region
            continue;
        } else if region.contains(&to_exclude.start) && region.contains(&to_exclude.end) {
            out.push(Range::from(region.start..to_exclude.start));
            out.push(Range::from(to_exclude.end..region.end));
        } else if to_exclude.contains(&region.start) {
            region.start = to_exclude.end;
            out.push(region);
        } else if to_exclude.contains(&region.end) {
            region.end = to_exclude.start;
            out.push(region);
        } else {
            out.push(region);
        }
    }

    out
}

fn allocate_and_copy(frame_alloc: &mut FrameAllocator, src: &[u8]) -> Result<Range<usize>> {
    let layout = Layout::from_size_align(src.len(), arch::PAGE_SIZE).unwrap();
    let base = frame_alloc
        .allocate_contiguous(layout)
        .ok_or(Error::NoMemory)?;

    unsafe {
        let dst = slice::from_raw_parts_mut(base as *mut u8, src.len());

        ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok(Range::from(base..base.checked_add(layout.size()).unwrap()))
}
