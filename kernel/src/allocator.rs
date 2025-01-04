use crate::INITIAL_HEAP_SIZE_PAGES;
use core::alloc::Layout;
use core::range::Range;
use loader_api::BootInfo;
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::{BootstrapAllocator, FrameAllocator};
use mmu::AddressRangeExt;
use talc::{ErrOnOom, Span, Talc, Talck};

#[global_allocator]
static KERNEL_ALLOCATOR: Talck<sync::RawMutex, ErrOnOom> = Talc::new(ErrOnOom).lock();

pub fn init(boot_alloc: &mut BootstrapAllocator, boot_info: &BootInfo) {
    let layout = Layout::from_size_align(INITIAL_HEAP_SIZE_PAGES * PAGE_SIZE, PAGE_SIZE).unwrap();

    let phys = boot_alloc.allocate_contiguous(layout).unwrap();

    let virt = {
        let start = boot_info
            .physical_address_offset
            .checked_add(phys.get())
            .unwrap();

        Range::from(start..start.checked_add(layout.size()).unwrap())
    };

    log::debug!("Kernel heap: {virt:?}");

    let mut alloc = KERNEL_ALLOCATOR.lock();
    let span = Span::from_base_size(virt.start.as_mut_ptr(), virt.size());
    unsafe {
        let old_heap = alloc.claim(span).unwrap();
        alloc.extend(old_heap, span);
    }
}
