use crate::arch::EntryFlags;
use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use core::alloc::Layout;
use core::ops::Range;
use talc::{Span, Talc, Talck};
use vmm::{Flush, FrameAllocator, Mapper, VirtualAddress};

#[global_allocator]
static KERNEL_ALLOCATOR: Talck<kstd::sync::RawMutex, OomHandler> = Talc::new(OomHandler {
    heap: Span::empty(),
    min: 0,
})
.lock();

pub fn init(
    frame_alloc: &mut dyn FrameAllocator<kconfig::MEMORY_MODE>,
    heap: Range<VirtualAddress>,
) -> Result<(), vmm::Error> {
    let mut alloc = KERNEL_ALLOCATOR.lock();
    alloc.oom_handler.heap = Span::from_base_size(
        heap.end.sub(kconfig::PAGE_SIZE).as_raw() as *mut u8,
        kconfig::PAGE_SIZE,
    );
    alloc.oom_handler.min = heap.start.as_raw();
    alloc.oom_handler.ensure_mapped(
        frame_alloc,
        Span::from_base_size(heap.end.as_raw() as *mut u8, 0),
    )?;

    unsafe {
        let heap = alloc.oom_handler.heap;
        alloc.claim(heap).unwrap()
    };

    Ok(())
}

struct OomHandler {
    heap: Span,
    min: usize,
}

impl OomHandler {
    fn ensure_mapped(
        &self,
        frame_alloc: &mut dyn FrameAllocator<kconfig::MEMORY_MODE>,
        old_heap: Span,
    ) -> Result<(), vmm::Error> {
        let (span_to_map, empty) = self.heap.except(old_heap);
        assert!(empty.is_empty());

        let mut mapper: Mapper<'_, vmm::Riscv64Sv39> = Mapper::from_active(0, frame_alloc);
        let mut flush = Flush::empty(0);

        let heap_phys = {
            let base = mapper.allocator_mut().allocate_frames(span_to_map.size())?;
            base..base.add(span_to_map.size())
        };

        let heap_virt = {
            let Range { start, end } = span_to_map.to_ptr_range().unwrap();

            VirtualAddress::new(start as usize)..VirtualAddress::new(end as usize)
        };

        log::debug!("mapping kernel heap region {heap_virt:?} => {heap_phys:?}");
        mapper.map_range(
            heap_virt,
            heap_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            &mut flush,
        )?;

        flush.flush()
    }
}

impl talc::OomHandler for OomHandler {
    fn handle_oom(talc: &mut Talc<Self>, layout: Layout) -> Result<(), ()> {
        let old_heap = talc.oom_handler.heap;

        // we're going to extend the heap downward, doubling its size,
        // but we'll be sure not to extend past the limit
        let new_heap: Span = old_heap
            .extend(
                old_heap
                    .size()
                    .max(layout.size())
                    .div_ceil(kconfig::PAGE_SIZE)
                    * kconfig::PAGE_SIZE,
                0,
            )
            .above(talc.oom_handler.min as *mut u8);

        if new_heap == old_heap {
            // we won't be extending the heap, so we should return Err
            return Err(());
        }

        unsafe {
            // we're assuming the new memory up to HEAP_TOP_LIMIT is unused and allocatable
            talc.oom_handler.heap = talc.extend(old_heap, new_heap);
        }

        with_frame_alloc(|alloc| talc.oom_handler.ensure_mapped(alloc, old_heap))
            .map_err(|_| ())?;

        Ok(())
    }
}
