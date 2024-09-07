#![no_std]
#![no_main]
#![feature(naked_functions)]
#![feature(maybe_uninit_slice)]

extern crate alloc;
extern crate panic_abort;

mod arch;
mod boot_info;
mod error;

mod kconfig;
mod kernel;
mod machine_info;
mod paging;
mod virt_alloc;

use crate::boot_info::init_boot_info;
use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::paging::{PageTableBuilder, PageTableResult};
use crate::virt_alloc::VirtAllocator;
use core::ops::Range;
use core::ptr::addr_of;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{ptr, slice};
use error::Error;
use kmm::{AddressRangeExt, BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use linked_list_allocator::LockedHeap;
use loader_api::BootInfo;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

pub type Result<T> = core::result::Result<T, Error>;

static BOOT_HART: AtomicUsize = AtomicUsize::new(0);

fn main(hartid: usize) -> ! {
    static INIT: sync::OnceLock<(PageTableResult, &'static BootInfo)> = sync::OnceLock::new();

    log::info!("Hart {hartid} started");

    let (page_table_result, boot_info) = INIT
        .get_or_try_init(init_global)
        .expect("failed to initialize system");

    log::debug!("Activating page table for hart {hartid}...");
    // SAFETY: This will invalidate all pointers and references that aren't on the loader stack
    // (the FDT slice and importantly the frame allocator) so care has to be taken to either
    // not access these anymore (which should be easy, this is one of the last steps we perform before hading off
    // to the kernel) or to map them into virtual memory first!
    unsafe {
        page_table_result.activate_table();
    }

    log::debug!("Initializing TLS region for hart {hartid}...");
    page_table_result.init_tls_region_for_hart(hartid);

    unsafe {
        arch::switch_to_kernel(
            hartid,
            page_table_result.kernel_entry(),
            page_table_result.stack_region_for_hart(hartid),
            page_table_result
                .tls_region_for_hart(hartid)
                .unwrap_or_default()
                .start,
            boot_info,
        )
    }
}

fn init_global() -> Result<(PageTableResult, &'static BootInfo)> {
    let machine_info = arch::machine_info();

    let loader_regions = LoaderRegions::new(machine_info);
    log::trace!("{loader_regions:?}");

    // init frame allocator
    let mut frame_alloc: BumpAllocator<kconfig::MEMORY_MODE> = unsafe {
        BumpAllocator::new_with_lower_bound(
            &machine_info.memories,
            loader_regions.read_write.end,
            VirtualAddress::default(), // while we haven't activated the virtual memory we have not offset
        )
    };

    let mut virt_alloc = VirtAllocator::new(ChaCha20Rng::from_seed(
        machine_info.rng_seed.unwrap()[0..32].try_into().unwrap(),
    ));

    let physical_memory_offset = virt_alloc
        .reserve_range(machine_info.memory_hull().size(), kconfig::PAGE_SIZE)
        .start;

    // Move the FDT to a safe location, so we don't accidentally overwrite it
    log::trace!("copying FDT to safe location...");
    let fdt_offset = allocate_and_copy_fdt(machine_info, &mut frame_alloc, physical_memory_offset)?;

    // init heap allocator
    init_global_allocator(machine_info);

    // decompress & parse kernel
    log::trace!("parsing kernel...");
    let kernel = Kernel::from_compressed(kernel::KERNEL_BYTES, &mut frame_alloc)?;

    log::trace!("initializing page tables...");
    let page_table_result =
        PageTableBuilder::from_alloc(&mut frame_alloc, physical_memory_offset, &mut virt_alloc)?
            .map_kernel(&kernel, machine_info)?
            .map_physical_memory(machine_info)?
            .identity_map_loader(&loader_regions)?
            .print_statistics()
            .result();

    let hartid = BOOT_HART.load(Ordering::Relaxed);

    // init boot info
    let boot_info = init_boot_info(
        &mut frame_alloc,
        hartid,
        &page_table_result,
        fdt_offset,
        &kernel,
        physical_memory_offset,
    )?;

    Ok((page_table_result, boot_info))
}

/// Moves the FDT from wherever the previous bootloader placed it into a properly allocated place,
/// so we don't accidentally override it
///
/// # Errors
///
/// Returns an error if allocation fails.
pub fn allocate_and_copy_fdt(
    machine_info: &MachineInfo,
    alloc: &mut BumpAllocator<kconfig::MEMORY_MODE>,
    physmem_off: VirtualAddress,
) -> Result<VirtualAddress> {
    let frames = machine_info.fdt.len().div_ceil(kconfig::PAGE_SIZE);
    let base = alloc.allocate_frames(frames)?;

    unsafe {
        let dst = slice::from_raw_parts_mut(base.as_raw() as *mut u8, machine_info.fdt.len());

        ptr::copy_nonoverlapping(machine_info.fdt.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok(physmem_off.add(base.as_raw()))
}

fn init_global_allocator(machine_info: &MachineInfo) {
    #[global_allocator]
    static ALLOC: LockedHeap = LockedHeap::empty();

    unsafe {
        ALLOC.lock().init_from_phys_range(&machine_info.memories[0]);
    }
}

/// Information about our own memory regions.
/// Used for identity mapping and calculating available physical memory.
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

        let executable: Range<PhysicalAddress> = {
            PhysicalAddress::new(addr_of!(__text_start) as usize)
                ..PhysicalAddress::new(addr_of!(__text_end) as usize)
        };

        let read_only: Range<PhysicalAddress> = {
            PhysicalAddress::new(addr_of!(__rodata_start) as usize)
                ..PhysicalAddress::new(addr_of!(__rodata_end) as usize)
        };

        let read_write: Range<PhysicalAddress> = {
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
