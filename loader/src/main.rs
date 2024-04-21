#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, split_array)]

use crate::elf::ElfSections;
use crate::paging::Mapper;
use boot_info::BootInfo;
use core::ops::Range;
use core::{ptr, slice};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use spin::{Barrier, Mutex, Once, RwLock};
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
    hartmems_virt_start: VirtualAddress,
    frame_alloc_offset: usize,
}

fn main(hartid: usize, boot_info: &'static BootInfo) -> ! {
    log::debug!("Hart {hartid} started");

    static INIT: Once<(Mapper, Range<VirtualAddress>)> = Once::new();

    let (mapper, fdt_virt) = INIT.call_once(|| {
        // Safety: The boot_info module ensures the memory entries are in the right order
        let mut alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> =
            unsafe { BumpAllocator::new(&boot_info.memories, 0) };

        let fdt_phys = allocate_and_copy(&mut alloc, boot_info.fdt);
        log::trace!("Copied FDT to {fdt_phys:?}");

        let fdt_virt = kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.start)
            ..kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.end);

        // 1. Verify kernel signature
        let kernel = verify_kernel_signature(kconfig::VERIFYING_KEY, kconfig::KERNEL_IMAGE);
        log::info!("Successfully verified kernel image signature");

        // TODO decompress kernel

        // 2. Copy kernel to top of physmem
        let kernel = allocate_and_copy(&mut alloc, kernel);
        log::trace!("Copied kernel to {kernel:?}");

        let kernel_sections = elf::parse(unsafe { kernel.as_slice() });

        let mut mapper = Mapper::new(alloc, boot_info, kernel_sections).unwrap();

        mapper.map_physical_memory().unwrap();
        mapper.identity_map_loader().unwrap();
        mapper.map_kernel_sections().unwrap();

        for hartid in 0..boot_info.cpus {
            let hartmem_phys = mapper.map_hartmem(hartid).unwrap();

            copy_tdata(mapper.kernel_sections(), hartmem_phys)
        }

        const KIB: usize = 1024;
        const MIB: usize = 1024 * KIB;

        let frame_usage = mapper.frame_usage();
        log::info!(
            "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
            (frame_usage.used * kconfig::PAGE_SIZE) / KIB,
            (frame_usage.total * kconfig::PAGE_SIZE) / MIB,
            (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
        );

        (mapper, fdt_virt)
    });

    log::debug!("Hart {hartid} Activating page table...");
    mapper.activate_page_table();

    let hartmem = mapper.hartmem_virt(hartid);
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
        hartmems_virt_start: unsafe { VirtualAddress::new(kconfig::MEMORY_MODE::PHYS_OFFSET) }
            .sub(hartmem.size() * boot_info.cpus),
        frame_alloc_offset: mapper.frame_alloc_offset(),
    };

    log::trace!("Hart {hartid} kargs {kargs:?}");

    unsafe {
        arch::kernel_entry(
            hartid,
            stack_virt.end,
            tls_virt.start,
            mapper.kernel_sections().entry,
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

fn copy_tdata(kernel_sections: &ElfSections, hartmem_phys: Range<PhysicalAddress>) {
    unsafe {
        let src = slice::from_raw_parts(
            kernel_sections.tdata.phys.start.as_raw() as *const u8,
            kernel_sections.tdata.phys.size(),
        );

        let tdata_addr = hartmem_phys.end.sub(src.len());
        let dst = slice::from_raw_parts_mut(tdata_addr.as_raw() as *mut u8, src.len());

        log::trace!(
            "Copying tdata from {:?} to {:?}",
            src.as_ptr_range(),
            dst.as_ptr_range()
        );

        ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }
}
