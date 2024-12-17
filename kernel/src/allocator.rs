use loader_api::BootInfo;
use talc::{ErrOnOom, Span, Talc, Talck};
use pmm::AddressRangeExt;

#[global_allocator]
static KERNEL_ALLOCATOR: Talck<sync::RawMutex, ErrOnOom> = Talc::new(ErrOnOom).lock();

pub fn init(boot_info: &BootInfo) {
    let heap = boot_info
        .heap_region
        .as_ref()
        .expect("missing heap region, this is a bug!");

    log::debug!("Kernel heap: {heap:?}");

    let mut alloc = KERNEL_ALLOCATOR.lock();
    let span = Span::from_base_size(
        heap.start.as_raw() as *mut u8,
        heap.size()
    );
    unsafe {
        let old_heap = alloc.claim(span).unwrap();
        alloc.extend(old_heap, span);
    }
}
