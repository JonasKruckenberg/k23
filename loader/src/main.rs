#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, split_array)]

extern crate alloc;

use crate::boot_info::BootInfo;
use core::ptr::{addr_of, addr_of_mut};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use linked_list_allocator::LockedHeap;
use vmm::{Mode, PhysicalAddress, VirtualAddress};

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
pub struct KernelArgs {
    boot_hart: usize,
    fdt: VirtualAddress,
    kernel_start: VirtualAddress,
    kernel_end: VirtualAddress,
    stack_start: VirtualAddress,
    stack_end: VirtualAddress,
    alloc_offset: usize,
}

/// ## Virtual Memory Layout
///
/// STACK_OFFSET = PHYS_OFFSET - (num_cpus * stack_size_pages * page_size)
///
/// ### Sv39
/// | Address Range                           | Size    | Description                                         |
/// |-----------------------------------------|---------|-----------------------------------------------------|
/// | 0x0000000000000000..=0x0000003fffffffff |  256 GB | user-space virtual memory                           |
/// | 0x0000004000000000..=0xffffffbfffffffff | ~16K PB | hole of non-canonical virtual memory addresses      |
/// |                                         |         | kernel-space virtual memory                         |
/// | 0xffffffc000000000..=0xffffffd7fffefffe |  ~96 GB | unused                                              |
/// |       STACK_OFFSET..=0xffffffd7ffffffff |         | kernel stack                                        |
/// | 0xffffffd800000000..=0xffffffe080000000 |  124 GB | direct mapping of all physical memory (PHYS_OFFSET) |
/// | 0xffffffff80000000..=0xffffffffffffffff |    2 GB | kernel (KERN_OFFSET)                                |
///
///
/// ### Sv48
/// | Address Range                           | Size    | Description                                         |
/// |-----------------------------------------|---------|-----------------------------------------------------|
/// | 0x0000000000000000..=0x00007fffffffffff |  128 TB | user-space virtual memory                           |
/// | 0x0000800000000000..=0xffff7fffffffffff | ~16K PB | hole of non-canonical virtual memory addresses      |
/// |                                         |         | kernel-space virtual memory                         |
/// | 0xffff800000000000..=0xffffbfff7ffefffe |  ~64 TB | unused                                              |
/// |       STACK_OFFSET..=0xffffbfff7fffffff |         | kernel stack                                        |
/// | 0xffffbfff80000000..=0xffffffff7fffffff |   64 TB | direct mapping of all physical memory (PHYS_OFFSET) |
/// | 0xffffffff80000000..=0xffffffffffffffff |    2 GB | kernel (KERN_OFFSET)                                |
///
///
/// ### Sv57
/// | Address Range                           | Size    | Description                                         |
/// |-----------------------------------------|---------|-----------------------------------------------------|
/// | 0x0000000000000000..=0x00ffffffffffffff |   64 PB | user-space virtual memory                           |
/// | 0x0100000000000000..=0xfeffffffffffffff | ~16K PB | hole of non-canonical virtual memory addresses      |
/// |                                         |         | kernel-space virtual memory                         |
/// | 0xff00000000000000..=0xff7fffff7ffefffe |  ~32 PB | unused                                              |
/// |       STACK_OFFSET..=0xff7fffff7fffffff |         | kernel stack                                        |
/// | 0xff7fffff80000000..=0xffffffff7fffffff |   32 PB | direct mapping of all physical memory (PHYS_OFFSET) |
/// | 0xffffffff80000000..=0xffffffffffffffff |    2 GB | kernel (KERN_OFFSET)                                |
///
fn main(hartid: usize, boot_info: BootInfo) -> ! {
    // 1. Verify kernel signature
    let compressed_kernel = verify_kernel_signature(kconfig::VERIFYING_KEY, kconfig::KERNEL_IMAGE);
    log::info!("successfully verified kernel image signature");

    // 2. Init global allocator
    init_global_alloc(&boot_info);

    // 3. decompress kernel
    let kernel = lz4_flex::decompress_size_prepended(compressed_kernel)
        .expect("failed to decompress kernel");
    log::info!("successfully decompressed kernel");

    let kernel_regions = elf::parse(&kernel);
    let kernel_start = kernel_regions.text.virt.start;
    let kernel_end = kernel_regions.data.virt.end;
    let kernel_entry = kernel_regions.entry;

    let (alloc_offset, fdt_addr, kernel_stack_virt) =
        paging::init(&boot_info, kernel_regions).expect("failed to set up page tables");

    let args = KernelArgs {
        boot_hart: hartid,
        fdt: fdt_addr,
        kernel_start,
        kernel_end,
        stack_start: kernel_stack_virt.start,
        stack_end: kernel_stack_virt.end,

        alloc_offset,
    };

    unsafe {
        let args_ptr =
            kconfig::MEMORY_MODE::phys_to_virt(PhysicalAddress::new(addr_of!(args) as usize));

        arch::kernel_entry(kernel_stack_virt.end, kernel_entry, args_ptr)
    }
}

fn init_global_alloc(boot_info: &BootInfo) {
    #[global_allocator]
    static ALLOC: LockedHeap = LockedHeap::empty();

    extern "C" {
        static mut __data_end: u8;
    }

    let data_end = unsafe { addr_of_mut!(__data_end) };

    // INVARIANT: We assume that memories[0] is the *same* region the loader got placed in AND that the loader is placed at the start of that region.
    let heap_size = boot_info.memories[0].end.as_raw() - data_end as usize;
    unsafe {
        ALLOC.lock().init(data_end, heap_size);
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

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

/// # Safety
///
/// This is an extern
#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Loader stack is corrupted")
}
