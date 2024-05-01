use crate::allocator::locked::LockedHeap;
use crate::kconfig;
use crate::kernel_mapper::with_mapper;
use vmm::{EntryFlags, VirtualAddress};

mod heap;
mod locked;
mod slab;
#[cfg(feature = "track-allocations")]
mod tracking;

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init(offset: VirtualAddress) -> Result<(), vmm::Error> {
    const HEAP_PAGES: usize = 8192; // 32 MiB

    let heap_start = with_mapper(0, |mut mapper, flush| {
        let heap_phys = {
            let base = mapper.allocator_mut().allocate_frames(HEAP_PAGES)?;
            base..base.add(HEAP_PAGES * kconfig::PAGE_SIZE)
        };

        let heap_virt = offset.sub(HEAP_PAGES * kconfig::PAGE_SIZE)..offset;

        log::trace!("Mapping kernel heap {heap_virt:?} => {heap_phys:?}...");

        mapper.map_range_with_flush(
            heap_virt.clone(),
            heap_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            flush,
        )?;

        Ok(heap_virt.start)
    })?;

    unsafe { ALLOCATOR.init::<kconfig::MEMORY_MODE>(heap_start, HEAP_PAGES * kconfig::PAGE_SIZE) }

    #[cfg(feature = "track-allocations")]
    tracking::init();

    Ok(())
}

pub fn print_heap_statistics() {
    log::debug!("Allocator Usage {:#?}", ALLOCATOR.usage());

    #[cfg(feature = "track-allocations")]
    tracking::print_histograms();
}
