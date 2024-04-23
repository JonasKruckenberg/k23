#![allow(unused)]

use crate::allocator::locked::LockedHeap;
use crate::kconfig;
use crate::kernel_mapper::with_kernel_mapper;
use vmm::{EntryFlags, FrameAllocator, VirtualAddress};

mod heap;
mod locked;
mod slab;

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init(offset: VirtualAddress) -> Result<(), vmm::Error> {
    const HEAP_PAGES: usize = 8192; // 32 MiB

    let heap_start = with_kernel_mapper(|mapper, flush| {
        let heap_phys = {
            let base = mapper.allocator_mut().allocate_frames(HEAP_PAGES)?;
            base..base.add(HEAP_PAGES * kconfig::PAGE_SIZE)
        };

        let heap_virt = offset.sub(HEAP_PAGES * kconfig::PAGE_SIZE)..offset;

        mapper.map_range_with_flush(
            heap_virt.clone(),
            heap_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            flush,
        )?;

        Ok(heap_virt.start)
    })?;

    unsafe { ALLOCATOR.init::<kconfig::MEMORY_MODE>(heap_start, HEAP_PAGES * kconfig::PAGE_SIZE) }

    Ok(())
}
