use crate::allocator::locked::LockedHeap;
use crate::kconfig;
use vmm::VirtualAddress;

mod heap;
mod locked;
mod slab;
#[cfg(feature = "track-allocations")]
mod tracking;

#[global_allocator]
pub static KERNEL_ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init(heap_start: VirtualAddress) -> Result<(), vmm::Error> {
    unsafe {
        KERNEL_ALLOCATOR
            .init::<kconfig::MEMORY_MODE>(heap_start, kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)
    }

    #[cfg(feature = "track-allocations")]
    tracking::init();

    Ok(())
}

pub fn print_heap_statistics() {
    log::debug!("Allocator Usage {:#?}", KERNEL_ALLOCATOR.usage());

    #[cfg(feature = "track-allocations")]
    tracking::print_histograms();
}
