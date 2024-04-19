#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, split_array)]

use crate::paging::Mapper;
use boot_info::BootInfo;
use core::{ptr, slice};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use spin::Once;
use vmm::{BumpAllocator, FrameAllocator, VirtualAddress, INIT};

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
    kernel_start: VirtualAddress,
    kernel_end: VirtualAddress,
    stacks_start: VirtualAddress,
    stacks_end: VirtualAddress,
    frame_alloc_offset: usize,
}

fn main(hartid: usize, boot_info: &'static BootInfo) -> ! {
    log::debug!("hello from hart {hartid} {boot_info:?}");

    static INIT: Once<Mapper> = Once::new();

    let mapper = INIT.call_once(|| {
        // Safety: The boot_info module ensures the memory entries are in the right order
        let mut alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> =
            unsafe { BumpAllocator::new(&boot_info.memories, 0) };

        // 1. Verify kernel signature
        let kernel = verify_kernel_signature(kconfig::VERIFYING_KEY, kconfig::KERNEL_IMAGE);
        log::info!("successfully verified kernel image signature");

        // TODO decompress kernel

        // 2. Copy kernel to top of physmem
        let kernel = copy_kernel(&mut alloc, kernel);
        log::debug!("copied kernel to {:?}", kernel.as_ptr_range());

        let kernel_sections = elf::parse(&kernel);

        let mut mapper = Mapper::new(alloc, &boot_info).expect("failed to setup mapper");
        mapper
            .identity_map_loader()
            .expect("failed to map own regions");
        mapper.map_physical_memory().expect("failed to map physmem");
        mapper
            .map_kernel_sections(&kernel_sections)
            .expect("failed to map kernel sections");
        mapper.map_fdt().expect("failed to map FDT region");
        mapper
            .map_kernel_stacks()
            .expect("failed to map kernel stack regions");

        mapper
    });

    log::debug!("activating page table...");
    let (frame_alloc_offset, fdt_virt, kernel_entry, kernel_virt, stacks_virt) =
        mapper.finish().unwrap();
    log::debug!("success");

    let kargs = KernelArgs {
        boot_hart: boot_info.boot_hart,
        fdt_virt,
        kernel_start: kernel_virt.start,
        kernel_end: kernel_virt.end,
        stacks_start: stacks_virt.start,
        stacks_end: stacks_virt.end,
        frame_alloc_offset,
    };

    // determine the right stack ptr
    let stack_ptr = stacks_virt
        .end
        .sub(hartid * kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE);

    unsafe {
        arch::kernel_entry(hartid, stack_ptr, kernel_entry, &kargs);
    }
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

fn copy_kernel(alloc: &mut BumpAllocator<INIT<kconfig::MEMORY_MODE>>, src: &[u8]) -> &'static [u8] {
    unsafe {
        let frames = src.len().div_ceil(kconfig::PAGE_SIZE);
        let base = alloc.allocate_frames(frames).unwrap();

        let dst = slice::from_raw_parts_mut(base.as_raw() as *mut u8, src.len());

        ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());

        dst
    }
}
