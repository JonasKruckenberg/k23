use crate::arch::riscv64::VMM;
use crate::arch::STACK_SIZE_PAGES;
use crate::board_info::BoardInfo;
use crate::sync::{Mutex, Once};
use crate::{KIB, MIB};
use core::ops::Range;
use core::ptr::addr_of;
use kmem::{
    AddressRange, Arch, BitMapAllocator, BumpAllocator, EntryFlags, Flush, FrameAllocator, Mapper,
    PhysicalAddress, VirtualAddress,
};
use smallvec::{smallvec, SmallVec};

const MMIO_OFFSET: usize = 0xffff_ffc8_0000_0000;

pub static FRAME_ALLOC: Once<Mutex<BitMapAllocator<VMM>>> = Once::empty();

static MEMORY_REGIONS: Once<SmallVec<Range<PhysicalAddress>, 16>> = Once::empty();

/// Physical memory regions that are defined by the layout of the kernel disk image
///
/// These must mirror the order and layout defined in the linker file.
struct KernelImageRegions {
    kernel_execute: Range<PhysicalAddress>,
    kernel_read_only: Range<PhysicalAddress>,
    kernel_read_write: Range<PhysicalAddress>,
}

impl KernelImageRegions {
    // TODO make stack area dynamic and use the FrameAlloc system
    pub fn from_board_info(board_info: &BoardInfo) -> Self {
        extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __stack_start: u8;
            static __rodata_start: u8;
            static __eh_frame_end: u8;
        }

        let text_start = unsafe { PhysicalAddress::new(addr_of!(__text_start) as usize) };
        let text_end = unsafe { PhysicalAddress::new(addr_of!(__text_end) as usize) };
        let rodata_start = unsafe { PhysicalAddress::new(addr_of!(__rodata_start) as usize) };
        let eh_frame_end = unsafe { PhysicalAddress::new(addr_of!(__eh_frame_end) as usize) };
        let stack_start = unsafe { PhysicalAddress::new(addr_of!(__stack_start) as usize) };

        let stack_end = stack_start.add(STACK_SIZE_PAGES * VMM::PAGE_SIZE * board_info.cpus);

        Self {
            kernel_execute: text_start..text_end,
            kernel_read_only: rodata_start..eh_frame_end,
            kernel_read_write: text_end..stack_end,
        }
    }

    /// Returns the overall address range covered by the regions irrespective of possible holes.
    pub fn combined_region(&self) -> Range<PhysicalAddress> {
        self.kernel_execute.start..self.kernel_read_write.end
    }
}

pub fn init(board_info: &BoardInfo) -> crate::Result<()> {
    let kernel_image_regions = KernelImageRegions::from_board_info(board_info);

    // collect memory regions we can work with
    // the useful memory starts right after the kernel image region
    MEMORY_REGIONS.get_or_init(|| {
        let mut region = board_info.memory.clone();

        // make sure the region doesn't include the kernel image
        let kernel_region = kernel_image_regions.combined_region();
        if region.start < kernel_region.end {
            region.start = kernel_region.end;
        }

        smallvec!(region)
    });

    let mut bump_alloc: BumpAllocator<VMM> =
        unsafe { BumpAllocator::new(MEMORY_REGIONS.wait(), 0) };
    let mut mapper = Mapper::new(0, &mut bump_alloc, 0)?;
    let mut flush = Flush::empty(0);

    // map physical memory at PHYS_OFFSET
    let board_mem_phys = board_info.memory.clone();
    let board_mem_virt = unsafe {
        log::trace!(
            "mapping physical memory {:?} => {:?}",
            VMM::phys_to_virt(board_info.memory.start)..VMM::phys_to_virt(board_info.memory.end),
            board_info.memory
        );
        VMM::phys_to_virt(board_info.memory.start)..VMM::phys_to_virt(board_info.memory.end)
    };
    log::trace!(
        "Mapping mmio region {:?}..{:?} => {:?}..{:?}",
        board_mem_virt.start,
        board_mem_virt.end,
        board_mem_phys.start,
        board_mem_phys.end
    );
    mapper.map_range_with_flush(
        board_mem_virt,
        board_mem_phys,
        EntryFlags::READ | EntryFlags::WRITE,
        &mut flush,
    )?;

    // helper function to map a kernel region both a PHYS_OFFSET *and* identity map it.
    let mut map_kernel_region =
        |range_phys: Range<PhysicalAddress>, page_flags: EntryFlags<VMM>| -> crate::Result<()> {
            let range_virt =
                unsafe { VMM::phys_to_virt(range_phys.start)..VMM::phys_to_virt(range_phys.end) };

            log::trace!(
                "Mapping kernel region: {:?}..{:?} => {:?}..{:?}",
                range_virt.start,
                range_virt.end,
                range_phys.start,
                range_phys.end
            );
            mapper.map_range_with_flush(range_virt, range_phys.clone(), page_flags, &mut flush)?;

            log::trace!(
                "Identity mapping kernel region: {:?}..{:?}",
                range_phys.start,
                range_phys.end
            );
            mapper.identity_map_range_with_flush(range_phys, page_flags, &mut flush)?;

            Ok(())
        };

    // map kernel
    map_kernel_region(
        kernel_image_regions.kernel_execute.clone(),
        EntryFlags::READ | EntryFlags::EXECUTE,
    )?;
    map_kernel_region(
        kernel_image_regions.kernel_read_only.clone(),
        EntryFlags::READ,
    )?;
    map_kernel_region(
        kernel_image_regions.kernel_read_write.clone(),
        EntryFlags::READ | EntryFlags::WRITE,
    )?;

    // map MMIO region
    let mmio_phys = board_info.serial.mmio_regs.clone().align(VMM::PAGE_SIZE);
    let mmio_virt = unsafe {
        let base = unsafe { VirtualAddress::new(MMIO_OFFSET) };
        let mmio_size = mmio_phys.end.as_raw() - mmio_phys.start.as_raw();
        base..base.add(mmio_size)
    };

    log::trace!(
        "Mapping mmio region {:?}..{:?} => {:?}..{:?}",
        mmio_virt.start,
        mmio_virt.end,
        mmio_phys.start,
        mmio_phys.end
    );
    mapper.map_range_with_flush(
        mmio_virt.clone(),
        mmio_phys,
        EntryFlags::READ | EntryFlags::WRITE,
        &mut flush,
    )?;

    // mapper.root_table().debug_print_table()?;

    log::trace!("activating page table...");
    mapper.activate();

    crate::logger::init_late(mmio_virt.start);

    log::trace!("flushing address translation changes: {flush:?}");
    flush.flush()?;

    let frame_usage = bump_alloc.frame_usage();
    log::debug!(
        "Kernel mapping complete. Permanently used: {} KiB of {} MiB total ({:.3}%).",
        (frame_usage.used * VMM::PAGE_SIZE) / KIB,
        (frame_usage.total * VMM::PAGE_SIZE) / MIB,
        (frame_usage.used as f64 / frame_usage.total as f64) * 100.0
    );

    log::trace!("initializing global frame allocator...");
    let bitmap_alloc = BitMapAllocator::new(bump_alloc)?;

    // bitmap_alloc.debug_print_table();

    FRAME_ALLOC.get_or_init(|| Mutex::new(bitmap_alloc));

    Ok(())
}

//
//     let frame_usage = mapper.allocator().frame_usage();
//     log::info!(
//         "Physical memory after mapping: Used {}MiB of {}MiB, available {}MiB",
//         frame_usage.used * 4 / 1024,
//         frame_usage.total * 4 / 1024,
//         (frame_usage.total - frame_usage.used) * 4 / 1024
//     );

//     log::debug!("testing heap mapping...");
//     unsafe {
//         let slice = core::slice::from_raw_parts_mut(HEAP_BASE.as_raw() as *mut u8, HEAP_SIZE);
//         slice.fill(0);
//     }
//     log::debug!("testing heap success...");
//
//     Ok(())
// }
