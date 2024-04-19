#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, split_array)]

extern crate alloc;

use crate::paging::MappingResult;
use boot_info::BootInfo;
use core::mem;
use core::ptr::addr_of_mut;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use linked_list_allocator::LockedHeap;
use spin::Once;
use vmm::{BumpAllocator, EntryFlags, Mapper, Mode, VirtualAddress, INIT};

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

#[repr(C, align(16))]
#[derive(Clone)]
pub struct KernelArgs {
    boot_hart: usize,
    fdt_virt: VirtualAddress,
    kernel_start: VirtualAddress,
    kernel_end: VirtualAddress,
    stacks_start: VirtualAddress,
    stacks_end: VirtualAddress,
    frame_alloc_offset: usize,
}

fn main(hartid: usize, boot_info: &'static BootInfo) -> ! {
    log::debug!("hello from hart {hartid} {boot_info:?}");

    static KERNEL_ARGS: Once<(
        VirtualAddress,
        Mapper<INIT<kconfig::MEMORY_MODE>, BumpAllocator<'static, INIT<kconfig::MEMORY_MODE>>>,
        KernelArgs,
    )> = Once::new();

    let (kernel_entry, mapper, kargs) = KERNEL_ARGS.call_once(|| {
        // 1. Verify kernel signature
        let compressed_kernel =
            verify_kernel_signature(kconfig::VERIFYING_KEY, kconfig::KERNEL_IMAGE);
        log::info!("successfully verified kernel image signature");

        // 2. Init global allocator
        init_global_alloc(&boot_info);

        // 3. decompress kernel
        let kernel = lz4_flex::decompress_size_prepended(compressed_kernel)
            .expect("failed to decompress kernel")
            .leak(); // leaking the kernel here so the allocator doesn't attempt to free it
        log::info!("successfully decompressed kernel");

        let kernel_sections = elf::parse(&kernel);

        let MappingResult {
            mapper,
            fdt_virt,
            kernel_stacks_virt,
        } = paging::init(&boot_info, &kernel_sections).expect("failed to set up page tables");

        let args = KernelArgs {
            boot_hart: boot_info.boot_hart as usize,
            fdt_virt,
            kernel_start: kernel_sections.text.virt.start,
            kernel_end: kernel_sections.data.virt.end,
            stacks_start: kernel_stacks_virt.start,
            stacks_end: kernel_stacks_virt.end,
            frame_alloc_offset: mapper.allocator().offset(),
        };

        (kernel_sections.entry, mapper, args)
    });

    log::debug!("activating page table...");
    mapper.activate();
    log::debug!("success");

    // determine the right stack ptr
    let mut stack_ptr = kargs
        .stacks_end
        .sub(hartid * kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE);

    unsafe {
        let kargs_size = mem::size_of::<KernelArgs>() + mem::align_of::<KernelArgs>();

        let kargs_ptr = stack_ptr.sub(kargs_size).as_raw() as *mut KernelArgs;
        stack_ptr = stack_ptr.sub(kargs_size);

        core::ptr::write(kargs_ptr, kargs.clone());

        arch::kernel_entry(
            hartid,
            stack_ptr,
            *kernel_entry,
            VirtualAddress::new(kargs_ptr as usize),
        );
    }
}

fn init_global_alloc(boot_info: &BootInfo) {
    #[global_allocator]
    static ALLOC: LockedHeap = LockedHeap::empty();

    extern "C" {
        static mut __data_end: u8;
    }

    let heap_base = unsafe {
        addr_of_mut!(__data_end) as usize
            + (boot_info.cpus * kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE)
    };

    // INVARIANT: We assume that memories[0] is the *same* region the loader got placed in AND that the loader is placed at the start of that region.
    let heap_size = boot_info.memories[0].end.as_raw() - heap_base;

    log::debug!("loader heap {:#x?}", heap_base..(heap_base + heap_size));

    unsafe {
        ALLOC.lock().init(heap_base as *mut u8, heap_size);
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
