use crate::arch;
use crate::machine_info::MachineInfo;
use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::ops::DerefMut;
use loader_api::BootInfo;
use mmu::frame_alloc::{BuddyAllocator, FrameAllocator, FrameUsage};
use mmu::{AddressRangeExt, Flush, PhysicalAddress, VirtualAddress};

const KERNEL_ASID: usize = 0;

pub fn init(boot_info: &BootInfo, minfo: &MachineInfo) -> crate::Result<mmu::AddressSpace> {
    let mut frame_alloc = crate::vm::FRAME_ALLOC.get().unwrap().lock();

    let (mut arch, mut flush) =
        mmu::AddressSpace::from_active(KERNEL_ASID, boot_info.physical_address_offset);

    unmap_loader(boot_info, &mut arch, &mut flush);

    flush.flush()?;

    Ok(arch)
}

fn unmap_loader(boot_info: &BootInfo, arch: &mut mmu::AddressSpace, flush: &mut Flush) {
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
