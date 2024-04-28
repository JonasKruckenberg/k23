#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, split_array)]

use crate::paging::PageTableResult;
use boot_info::BootInfo;
use core::ops::Range;
use core::{ptr, slice};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sync::Once;
use vmm::{
    AddressRangeExt, BumpAllocator, FrameAllocator, Mode, PhysicalAddress, VirtualAddress, INIT,
};

mod arch;
mod boot_info;
mod elf;
mod logger;
mod paging;
mod panic;

pub mod kconfig {
    // Configuration constants and statics defined by the build script
    include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));
}

#[repr(C)]
#[derive(Debug)]
pub struct KernelArgs {
    boot_hart: u32,
    fdt_virt: VirtualAddress,
    stack_start: VirtualAddress,
    stack_end: VirtualAddress,
    page_alloc_offset: VirtualAddress,
    frame_alloc_offset: usize,
}

fn main(hartid: usize, boot_info: &'static BootInfo) -> ! {
    log::debug!("Hart {hartid} started");

    static INIT: Once<(PageTableResult, Range<VirtualAddress>)> = Once::new();

    let (page_table_result, fdt_virt) = INIT.get_or_init(|| {
        let own_regions = own_regions(boot_info);
        log::trace!("{own_regions:?}");

        // Safety: The boot_info module ensures the memory entries are in the right order
        let mut alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> = unsafe {
            BumpAllocator::new_with_lower_bound(&boot_info.memories, 0, own_regions.read_write.end)
        };

        let fdt_phys = allocate_and_copy(&mut alloc, boot_info.fdt);
        log::trace!("Copied FDT to {fdt_phys:?}");

        let fdt_virt = kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.start)
            ..kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.end);

        // // 1. Verify kernel signature
        let kernel = verify_kernel_signature(kconfig::VERIFYING_KEY, kconfig::KERNEL_IMAGE);
        log::info!("Successfully verified kernel image signature");

        // TODO decompress kernel

        // 2. Copy kernel to top of physmem
        let kernel = allocate_and_copy(&mut alloc, kernel);
        log::trace!("Copied kernel to {kernel:?}");

        let kernel_sections = elf::parse(unsafe {
            slice::from_raw_parts(kernel.start.as_raw() as *const _, kernel.size())
        });

        let res = paging::init(alloc, boot_info, kernel_sections).unwrap();

        (res, fdt_virt)
    });

    log::debug!("Hart {hartid} Activating page table...");
    page_table_result.activate_table();

    let hartmem = page_table_result.hartmem_virt(hartid);
    let stack_virt = hartmem.start
        ..hartmem
            .start
            .add(kconfig::STACK_SIZE_PAGES_KERNEL * kconfig::PAGE_SIZE);
    let tls_virt = hartmem
        .start
        .add(kconfig::STACK_SIZE_PAGES_KERNEL * kconfig::PAGE_SIZE)..hartmem.end;

    let kargs = KernelArgs {
        boot_hart: boot_info.boot_hart,
        fdt_virt: fdt_virt.start,
        stack_start: stack_virt.start,
        stack_end: stack_virt.end,
        page_alloc_offset: unsafe { VirtualAddress::new(kconfig::MEMORY_MODE::PHYS_OFFSET) }
            .sub(hartmem.size() * boot_info.cpus),
        frame_alloc_offset: page_table_result.frame_alloc_offset,
    };

    log::trace!("Hart {hartid} kargs {kargs:?}");

    unsafe {
        arch::kernel_entry(
            hartid,
            stack_virt.end,
            tls_virt.start,
            page_table_result.kernel_entry_virt,
            &kargs,
        );
    };
}

/// Allocates enough space using the BumpAllocator and copies the given bytes into it
fn allocate_and_copy(
    alloc: &mut BumpAllocator<INIT<kconfig::MEMORY_MODE>>,
    src: &[u8],
) -> Range<PhysicalAddress> {
    let frames = src.len().div_ceil(kconfig::PAGE_SIZE);
    let base = alloc.allocate_frames(frames).unwrap();

    unsafe {
        let dst = slice::from_raw_parts_mut(base.as_raw() as *mut u8, src.len());

        ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    base..base.add(src.len())
}

fn verify_kernel_signature<'a>(
    verifying_key: &[u8; ed25519_dalek::PUBLIC_KEY_LENGTH],
    kernel_image: &'a [u8],
) -> &'a [u8] {
    let verifying_key = VerifyingKey::from_bytes(verifying_key).unwrap();
    let (signature, kernel) = kernel_image.split_at(Signature::BYTE_SIZE);
    let signature = Signature::from_slice(signature).unwrap();

    verifying_key
        .verify(kernel, &signature)
        .expect("failed to verify kernel image signature");

    kernel
}
