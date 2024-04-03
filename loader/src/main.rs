#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

extern crate alloc;

mod arch;
mod boot_info;
mod elf;
mod externs;
mod logger;
mod panic;
mod stack;

pub mod kconfig {
    // Configuration constants and statics defined by the build script
    include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));
}

use crate::arch::BOOT_STACK;
use crate::boot_info::BootInfo;
use crate::elf::ElfSections;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::ops::Range;
use core::ptr::addr_of;
use spin::Mutex;
use vmm::{
    BumpAllocator, EntryFlags, Flush, FrameAllocator, FrameUsage, Mapper, Mode, PhysicalAddress,
    INIT,
};

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;

#[global_allocator]
static GLOBAL_ALLOCATOR: GAlloc<INIT<kconfig::MEMORY_MODE>> = GAlloc::EMPTY;

fn main(hartid: usize, boot_info: BootInfo<'static>) -> ! {
    let own_image_regions = ImageRegions::from_self();

    log::debug!("{own_image_regions:?}");

    let alloc: BumpAllocator<INIT<kconfig::MEMORY_MODE>> = unsafe {
        BumpAllocator::new(
            boot_info.memories[0].clone(),
            own_image_regions
                .read_write
                .end
                .sub_addr(boot_info.memories[0].start),
        )
    };

    GLOBAL_ALLOCATOR.init(alloc);

    let kernel = decompress_kernel().leak();
    let kernel_sections = elf::parse(&kernel);
    log::debug!("{kernel_sections:?}");

    let text_begin = unsafe {
        core::slice::from_raw_parts(kernel_sections.text.phys.start.as_raw() as *const u8, 100)
    };
    log::debug!(
        "beginning of kernel text section {:?}.. {text_begin:?}",
        kernel_sections.text.phys.start
    );

    init_paging(&boot_info, own_image_regions, kernel_sections.clone());

    let text_begin = unsafe {
        core::slice::from_raw_parts(kernel_sections.text.virt.start.as_raw() as *const u8, 100)
    };
    log::debug!(
        "beginning of kernel text section {:?}.. {text_begin:?}",
        kernel_sections.text.virt.start
    );

    let stack_usage = BOOT_STACK.usage();
    log::debug!(
        "Stack usage: {} KiB of {} KiB total ({:.3}%). High Watermark: {} KiB.",
        (stack_usage.used) / KIB,
        (stack_usage.total) / KIB,
        (stack_usage.used as f64 / stack_usage.total as f64) * 100.0,
        (stack_usage.high_watermark) / KIB,
    );

    unsafe {
        let kernel_entry: unsafe extern "C" fn(hartid: usize, opaque: *const u8) =
            core::mem::transmute(kernel_sections.text.virt.start);
        kernel_entry(hartid, boot_info.fdt.as_ptr())
    }

    unreachable!()
}

struct GAlloc<M>(Mutex<Option<BumpAllocator<M>>>);

impl<M> GAlloc<M> {
    pub const EMPTY: Self = Self(Mutex::new(None));

    pub fn init(&self, alloc: BumpAllocator<M>) {
        self.0.lock().replace(alloc);
    }
}

unsafe impl<M: Mode> alloc::alloc::GlobalAlloc for GAlloc<M> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut alloc = self.0.lock();
        let alloc = alloc.as_mut().expect("heap not initialized");

        let num_frames = layout.size().div_ceil(M::PAGE_SIZE);

        let ptr = alloc.allocate_frames(num_frames).unwrap();
        ptr.as_raw() as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        unimplemented!()
    }
}

impl<M: Mode> FrameAllocator<M> for GAlloc<M> {
    fn allocate_frames(&mut self, frames: usize) -> Result<PhysicalAddress, vmm::Error> {
        let alloc = self.0.get_mut();
        let alloc = alloc.as_mut().expect("heap not initialized");
        alloc.allocate_frames(frames)
    }

    fn deallocate_frames(
        &mut self,
        base: PhysicalAddress,
        frames: usize,
    ) -> Result<(), vmm::Error> {
        let alloc = self.0.get_mut();
        let alloc = alloc.as_mut().expect("heap not initialized");
        alloc.deallocate_frames(base, frames)
    }

    fn frame_usage(&self) -> FrameUsage {
        let alloc = self.0.lock();
        let alloc = alloc.as_ref().expect("heap not initialized");
        alloc.frame_usage()
    }
}

#[derive(Debug)]
struct ImageRegions {
    pub executable: Range<PhysicalAddress>,
    pub read_only: Range<PhysicalAddress>,
    pub read_write: Range<PhysicalAddress>,
}

impl ImageRegions {
    pub fn from_self() -> ImageRegions {
        extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __stack_start: u8;
            static __data_end: u8;
        }

        let executable = unsafe {
            let start = PhysicalAddress::new(addr_of!(__text_start) as usize);
            let end = PhysicalAddress::new(addr_of!(__text_end) as usize);
            start..end
        };

        let read_only = unsafe {
            let start = PhysicalAddress::new(addr_of!(__rodata_start) as usize);
            let end = PhysicalAddress::new(addr_of!(__rodata_end) as usize);
            start..end
        };

        let read_write = unsafe {
            let start =
                PhysicalAddress::new(addr_of!(__stack_start) as usize).add(8 * kconfig::PAGE_SIZE);
            let end = PhysicalAddress::new(addr_of!(__data_end) as usize);
            start..end
        };

        Self {
            executable,
            read_only,
            read_write,
        }
    }
}

fn decompress_kernel() -> Vec<u8> {
    let input = include_bytes!(env!("K23_KERNEL_ARTIFACT"));
    let output = lz4_flex::decompress_size_prepended(input).unwrap();
    log::debug!("decompressed kernel region {:?}", output.as_ptr_range());
    output
}

fn init_paging(
    boot_info: &BootInfo,
    own_image_regions: ImageRegions,
    kernel_sections: ElfSections,
) {
    let mut alloc = GLOBAL_ALLOCATOR.0.lock();
    let alloc = alloc.as_mut().expect("heap not initialized");

    let mut mapper = Mapper::new(0, alloc).unwrap();
    let mut flush = Flush::empty(0);

    // map physical memory at PHYS_OFFSET
    assert_eq!(
        boot_info.memories.len(),
        1,
        "expected only one contiguous memory region"
    );
    let mem_phys = boot_info.memories[0].clone();
    let mem_virt = kconfig::MEMORY_MODE::phys_to_virt(mem_phys.start)
        ..kconfig::MEMORY_MODE::phys_to_virt(mem_phys.end);

    log::debug!("Mapping physical memory {mem_virt:?}=>{mem_phys:?}...");
    mapper
        .map_range_with_flush(
            mem_virt,
            mem_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    // map own regions
    log::debug!(
        "Identity mapping own executable region {:?}...",
        own_image_regions.executable
    );
    mapper
        .identity_map_range_with_flush(
            own_image_regions.executable,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Identity mapping own read-only region {:?}...",
        own_image_regions.read_only
    );
    mapper
        .identity_map_range_with_flush(own_image_regions.read_only, EntryFlags::READ, &mut flush)
        .unwrap();

    log::debug!(
        "Identity mapping own read-write region {:?}...",
        own_image_regions.read_write
    );
    mapper
        .identity_map_range_with_flush(
            own_image_regions.read_write,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    // map kernel regions
    log::debug!(
        "Mapping kernel text region {:?}=>{:?}...",
        kernel_sections.text.virt,
        kernel_sections.text.phys
    );
    mapper
        .map_range_with_flush(
            kernel_sections.text.virt,
            kernel_sections.text.phys,
            EntryFlags::READ | EntryFlags::EXECUTE,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Mapping kernel rodata region {:?}=>{:?}...",
        kernel_sections.rodata.virt,
        kernel_sections.rodata.phys
    );
    mapper
        .map_range_with_flush(
            kernel_sections.rodata.virt,
            kernel_sections.rodata.phys,
            EntryFlags::READ,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Mapping kernel data region {:?}=>{:?}...",
        kernel_sections.data.virt,
        kernel_sections.data.phys
    );
    mapper
        .map_range_with_flush(
            kernel_sections.data.virt,
            kernel_sections.data.phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    log::debug!(
        "Mapping kernel bss region {:?}=>{:?}...",
        kernel_sections.bss.virt,
        kernel_sections.bss.phys
    );
    mapper
        .map_range_with_flush(
            kernel_sections.bss.virt,
            kernel_sections.bss.phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )
        .unwrap();

    mapper.activate();
    flush.flush().unwrap();

    let frame_usage = alloc.frame_usage();
    log::info!(
        "Mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * kconfig::PAGE_SIZE) / KIB,
        (frame_usage.total * kconfig::PAGE_SIZE) / MIB,
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );
}
