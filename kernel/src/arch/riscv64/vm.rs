use crate::arch;
use core::alloc::Layout;
use core::num::NonZeroUsize;
use loader_api::BootInfo;
use pmm::frame_alloc::{BuddyAllocator, FrameAllocator, FrameUsage};
use pmm::{AddressRangeExt, Flush, PhysicalAddress};

const KERNEL_ASID: usize = 0;

pub fn init(
    boot_info: &BootInfo,
    frame_alloc: &mut BuddyAllocator,
) -> crate::Result<pmm::AddressSpace> {
    let (mut arch, mut flush) =
        pmm::AddressSpace::from_active(KERNEL_ASID, boot_info.physical_memory_offset);

    unmap_loader(boot_info, frame_alloc, &mut arch, &mut flush);

    flush.flush()?;

    Ok(arch)
}

fn unmap_loader(
    boot_info: &BootInfo,
    frame_alloc: &mut BuddyAllocator,
    arch: &mut pmm::AddressSpace,
    flush: &mut Flush,
) {
    log::debug!("unmapping loader {:?}...", boot_info.loader_region);

    // unmap the identity mapped loader, but - since the physical memory is unmanaged - we
    // use a special "IgnoreAlloc" allocator that doesn't actually deallocate the frames
    // and instead just ignores the deallocation request
    let loader_region_len = boot_info.loader_region.size();
    arch.unmap(
        &mut IgnoreAlloc,
        boot_info.loader_region.start,
        NonZeroUsize::new(loader_region_len).unwrap(),
        flush,
    )
    .unwrap();

    // The kernel ELF is inlined by the loader, but we don't want it to be managed by the frame allocator
    // so instead we form the "pre-range" of all physical memory before the kernel ELF and the "post-range"
    // of all physical memory after the kernel ELF and add them to the frame allocator
    let pre_range =
        PhysicalAddress::new(boot_info.loader_region.start.as_raw())..boot_info.kernel_elf.start;
    let post_range = boot_info.kernel_elf.end.align_up(arch::PAGE_SIZE)
        ..PhysicalAddress::new(boot_info.loader_region.end.as_raw());

    unsafe {
        frame_alloc.add_range(pre_range);
        frame_alloc.add_range(post_range);
    }
}

struct IgnoreAlloc;
impl FrameAllocator for IgnoreAlloc {
    fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        unimplemented!()
    }

    fn deallocate_contiguous(&mut self, addr: PhysicalAddress, layout: Layout) {}

    fn allocate_contiguous_zeroed(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        unimplemented!()
    }

    fn allocate_partial(&mut self, layout: Layout) -> Option<(PhysicalAddress, usize)> {
        unimplemented!()
    }

    fn frame_usage(&self) -> FrameUsage {
        unimplemented!()
    }
}
