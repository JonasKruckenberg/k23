mod heap;
mod locked;
mod slab;

use crate::allocator::locked::LockedHeap;
use crate::arch::paging::FRAME_ALLOC;
use crate::arch::VMM;
use crate::{GIB, MIB};
use core::ops::DerefMut;
use kmem::{Arch, EntryFlags, Mapper, VirtualAddress};

pub const HEAP_SIZE: usize = 64 * MIB;

pub const HEAP_BASE: VirtualAddress =
    unsafe { VirtualAddress::new(usize::MAX & !VMM::ADDR_OFFSET_MASK).sub(2 * GIB) };

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() -> crate::Result<()> {
    let mut frame_alloc = FRAME_ALLOC.wait().lock();
    let mut mapper = Mapper::from_active(0, frame_alloc.deref_mut(), VMM::PHYS_OFFSET);

    log::debug!("here");

    let heap_phys = {
        let base = mapper
            .allocator_mut()
            .allocate_frames(HEAP_SIZE / VMM::PAGE_SIZE)?;
        base..base.add(HEAP_SIZE)
    };
    let heap_virt = HEAP_BASE..HEAP_BASE.add(HEAP_SIZE);

    log::trace!(
        "Mapping kernel heap {:?}..{:?} => {:?}..{:?}",
        heap_virt.start,
        heap_virt.end,
        heap_phys.start,
        heap_phys.end
    );

    let flush = mapper.map_range(heap_virt, heap_phys, EntryFlags::READ | EntryFlags::WRITE)?;
    flush.flush()?;

    unsafe {
        ALLOCATOR.init(HEAP_BASE, HEAP_SIZE);
    }

    Ok(())
}
