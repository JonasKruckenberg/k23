#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, maybe_uninit_slice, used_with_arg)]
#![allow(clippy::items_after_statements, clippy::needless_continue)]

mod arch;
mod logger;
mod machine_info;
mod mapping;
mod payload;

// Configuration constants and statics defined by the build script
include!(concat!(env!("OUT_DIR"), "/gen.rs"));

pub mod kconfig {
    #[allow(non_camel_case_types)]
    pub type MEMORY_MODE = vmm::Riscv64Sv39;
    pub const STACK_SIZE_PAGES: usize = 32;
    pub const PAGE_SIZE: usize = <MEMORY_MODE as ::vmm::Mode>::PAGE_SIZE;
}

use crate::machine_info::MachineInfo;
use crate::mapping::{set_up_mappings, Mappings};
use crate::payload::Payload;
use core::mem::MaybeUninit;
use core::ops::Range;
use core::ptr::addr_of;
use core::{ptr, slice};
use kstd::sync::OnceLock;
use linked_list_allocator::LockedHeap;
use loader_api::{MemoryRegion, MemoryRegionKind};
use vmm::{
    AddressRangeExt, BumpAllocator, FrameAllocator, Mode, PhysicalAddress, VirtualAddress, INIT,
};

#[global_allocator]
static ALLOC: LockedHeap = LockedHeap::empty();

fn main(hartid: usize, machine_info: &'static MachineInfo) -> ! {
    static MAPPINGS: OnceLock<Mappings> = OnceLock::new();

    log::info!("Hart {hartid} started");

    let mappings = MAPPINGS.get_or_init(|| {
        let own_regions = LoaderRegions::new(machine_info);
        log::trace!("{own_regions:?}");

        // Safety: The machine_info module ensures the memory entries are in the right order
        let mut alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> = unsafe {
            BumpAllocator::new_with_lower_bound(&machine_info.memories, own_regions.read_write.end)
        };

        unsafe {
            ALLOC.lock().init(
                machine_info.memories[0].start.as_raw() as *mut u8,
                machine_info.memories[0].size(),
            );
        }

        let (fdt_phys, fdt_virt) = allocate_and_copy_fdt(machine_info, &mut alloc).unwrap();

        let payload = Payload::from_signed_and_compressed(PAYLOAD, VERIFYING_KEY, &mut alloc);
        payload.assert_cpu_compatible(fdt_phys.as_raw() as *const u8);

        let mut mappings =
            set_up_mappings(&payload, machine_info, &own_regions, fdt_virt, &mut alloc).unwrap();

        let memory_regions = mappings.finalize_memory_regions(|_, raw_regions| {
            let mut next_region = 0;
            let mut push_region = |region: MemoryRegion| {
                raw_regions[next_region].write(region);
                next_region += 1;
            };

            for used_region in alloc.used_regions() {
                push_region(MemoryRegion {
                    range: used_region,
                    kind: MemoryRegionKind::Loader,
                });
            }

            for free_region in alloc.free_regions() {
                push_region(MemoryRegion {
                    range: free_region,
                    kind: MemoryRegionKind::Usable,
                });
            }

            unsafe { MaybeUninit::slice_assume_init_mut(&mut raw_regions[0..next_region]) }
        });

        mappings.finalize_boot_info(machine_info, memory_regions);

        mappings
    });

    mappings.activate_table();
    mappings.initialize_tls_region_for_hart(hartid);

    unsafe {
        arch::kernel_entry(
            mappings.entry_point(),
            mappings
                .tls_region_for_hart(hartid)
                .unwrap_or_default()
                .start,
            hartid,
            mappings.stack_region_for_hart(hartid),
            mappings.boot_info(),
        )
    }
}

/// Moves the FDT from wherever the previous bootloader placed it into a properly allocated place,
/// so we don't accidentally override it
///
/// # Errors
///
/// Returns an error if allocation fails.
pub fn allocate_and_copy_fdt(
    machine_info: &MachineInfo,
    alloc: &mut BumpAllocator<INIT<kconfig::MEMORY_MODE>>,
) -> Result<(PhysicalAddress, VirtualAddress), vmm::Error> {
    let frames = machine_info.fdt.len().div_ceil(kconfig::PAGE_SIZE);
    let base = alloc.allocate_frames(frames)?;

    unsafe {
        let dst = slice::from_raw_parts_mut(base.as_raw() as *mut u8, machine_info.fdt.len());

        ptr::copy_nonoverlapping(machine_info.fdt.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok((base, kconfig::MEMORY_MODE::phys_to_virt(base)))
}

#[derive(Debug)]
pub struct LoaderRegions {
    pub executable: Range<PhysicalAddress>,
    pub read_only: Range<PhysicalAddress>,
    pub read_write: Range<PhysicalAddress>,
}

impl LoaderRegions {
    #[must_use]
    pub fn new(machine_info: &MachineInfo) -> Self {
        extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __bss_start: u8;
            static __stack_start: u8;
        }

        let executable: Range<PhysicalAddress> = unsafe {
            PhysicalAddress::new(addr_of!(__text_start) as usize)
                ..PhysicalAddress::new(addr_of!(__text_end) as usize)
        };

        let read_only: Range<PhysicalAddress> = unsafe {
            PhysicalAddress::new(addr_of!(__rodata_start) as usize)
                ..PhysicalAddress::new(addr_of!(__rodata_end) as usize)
        };

        let read_write: Range<PhysicalAddress> = unsafe {
            let start = PhysicalAddress::new(addr_of!(__bss_start) as usize);
            let stack_start = PhysicalAddress::new(addr_of!(__stack_start) as usize);

            start
                ..stack_start
                    .add(machine_info.cpus * kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE)
        };

        LoaderRegions {
            executable,
            read_only,
            read_write,
        }
    }
}
