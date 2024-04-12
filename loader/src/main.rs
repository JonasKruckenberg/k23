#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, split_array)]

extern crate alloc;

use crate::boot_info::BootInfo;
use core::ptr::{addr_of, addr_of_mut};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use linked_list_allocator::LockedHeap;
use vmm::{
    AddressRangeExt, BumpAllocator, EntryFlags, Flush, FrameAllocator, Mapper, Mode,
    PhysicalAddress, VirtualAddress, INIT,
};

mod arch;
mod boot_info;
mod elf;
mod logger;

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

    // 4. setup frame allocator
    //      - start at top of physmem, works downwards

    let mut alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> =
        BumpAllocator::new(&boot_info.memories);

    let mut mapper = Mapper::new(0, &mut alloc).expect("failed to initialize page table mapper");
    let mut flush = Flush::empty(0);

    for region_phys in &boot_info.memories {
        let region_virt = kconfig::MEMORY_MODE::phys_to_virt(region_phys.start)
            ..kconfig::MEMORY_MODE::phys_to_virt(region_phys.end);

        log::debug!("Mapping physical memory region {region_virt:?} => {region_phys:?}...");
        mapper
            .map_range_with_flush(
                region_virt,
                region_phys.clone(),
                EntryFlags::READ | EntryFlags::WRITE,
                &mut flush,
            )
            .unwrap();
    }

    let fdt_phys = unsafe {
        let base = PhysicalAddress::new(boot_info.fdt.as_ptr() as usize);

        (base..base.add(boot_info.fdt.len())).align(kconfig::PAGE_SIZE)
    };
    let fdt_virt = kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.start)
        ..kconfig::MEMORY_MODE::phys_to_virt(fdt_phys.end);

    log::debug!("Mapping fdt region {fdt_virt:?} => {fdt_phys:?}...");
    mapper
        .map_range_with_flush(fdt_virt.clone(), fdt_phys, EntryFlags::READ, &mut flush)
        .unwrap();

    extern "C" {
        static __text_start: u8;
        static __text_end: u8;
        static __rodata_start: u8;
        static __rodata_end: u8;
        static __stack_start: u8;
        static __data_end: u8;
    }

    let own_executable_region = unsafe {
        PhysicalAddress::new(addr_of!(__text_start) as usize)
            ..PhysicalAddress::new(addr_of!(__text_end) as usize)
    };
    let own_read_only_region = unsafe {
        PhysicalAddress::new(addr_of!(__rodata_start) as usize)
            ..PhysicalAddress::new(addr_of!(__rodata_end) as usize)
    };
    let own_read_write_region = unsafe {
        PhysicalAddress::new(addr_of!(__stack_start) as usize)
            ..PhysicalAddress::new(addr_of!(__data_end) as usize)
    };

    log::debug!("Identity mapping own executable region {own_executable_region:?}...");
    mapper
        .identity_map_range_with_flush(
            own_executable_region,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut flush,
        )
        .unwrap();

    log::debug!("Identity mapping own read-only region {own_read_only_region:?}...");
    mapper
        .identity_map_range_with_flush(own_read_only_region, EntryFlags::READ, &mut flush)
        .unwrap();

    log::debug!("Identity mapping own read-write region {own_read_write_region:?}...");
    mapper
        .identity_map_range_with_flush(
            own_read_write_region,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    let kernel_regions = elf::parse(&kernel);

    log::debug!(
        "Mapping kernel text region {:?} => {:?}...",
        kernel_regions.text.virt,
        kernel_regions.text.phys
    );
    mapper
        .map_range_with_flush(
            kernel_regions.text.virt.clone(),
            kernel_regions.text.phys,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Mapping kernel rodata region {:?} => {:?}...",
        kernel_regions.rodata.virt,
        kernel_regions.rodata.phys
    );
    mapper
        .map_range_with_flush(
            kernel_regions.rodata.virt,
            kernel_regions.rodata.phys,
            EntryFlags::READ,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Mapping kernel bss region {:?} => {:?}...",
        kernel_regions.bss.virt,
        kernel_regions.bss.phys
    );
    mapper
        .map_range_with_flush(
            kernel_regions.bss.virt,
            kernel_regions.bss.phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Mapping kernel data region {:?} => {:?}...",
        kernel_regions.data.virt,
        kernel_regions.data.phys
    );
    mapper
        .map_range_with_flush(
            kernel_regions.data.virt.clone(),
            kernel_regions.data.phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    let kernel_stack_frames = boot_info.cpus * kconfig::STACK_SIZE_PAGES_KERNEL;

    let kernel_stack_phys = {
        let base = mapper
            .allocator_mut()
            .allocate_frames(kernel_stack_frames)
            .unwrap();
        base..base.add(kernel_stack_frames * kconfig::PAGE_SIZE)
    };

    let kernel_stack_virt = unsafe {
        let end = VirtualAddress::new(kconfig::MEMORY_MODE::PHYS_OFFSET);

        end.sub(kernel_stack_frames * kconfig::PAGE_SIZE)..end
    };

    mapper
        .map_range_with_flush(
            kernel_stack_virt.clone(),
            kernel_stack_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    log::debug!("activating page table...");
    mapper.activate();

    let frame_usage = alloc.frame_usage();
    log::info!(
        "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * kconfig::PAGE_SIZE) / 1024,
        (frame_usage.total * kconfig::PAGE_SIZE) / (1024 * 1024),
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );

    let args = KernelArgs {
        boot_hart: hartid,
        fdt: fdt_virt.start,
        kernel_start: kernel_regions.text.virt.start,
        kernel_end: kernel_regions.data.virt.end,
        stack_start: kernel_stack_virt.start,
        stack_end: kernel_stack_virt.end,

        alloc_offset: alloc.offset(),
    };

    unsafe {
        arch::kernel_entry(
            args.kernel_end.as_raw(),
            kernel_regions.entry.as_raw(),
            &args,
        )
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

#[panic_handler]
#[no_mangle]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("LOADER PANIC {}", info);

    arch::halt()
}

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Loader stack is corrupted")
}
