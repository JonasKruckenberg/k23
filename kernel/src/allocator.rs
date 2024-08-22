use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use core::alloc::Layout;
use core::ops::Range;
use kmm::EntryFlags;
use kmm::{Flush, FrameAllocator, Mapper, VirtualAddress};
use loader_api::BootInfo;
use talc::{Span, Talc, Talck};

#[global_allocator]
static KERNEL_ALLOCATOR: Talck<sync::RawMutex, OomHandler> = Talc::new(OomHandler {
    heap: Span::empty(),
    max: 0,
})
.lock();

pub fn init(
    frame_alloc: &mut dyn FrameAllocator<kconfig::MEMORY_MODE>,
    boot_info: &BootInfo,
) -> Result<(), kmm::Error> {
    let heap = boot_info
        .free_virt
        .end
        .sub(kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)..boot_info.free_virt.end;

    log::debug!("Kernel heap: {heap:?}");

    let mut alloc = KERNEL_ALLOCATOR.lock();
    alloc.oom_handler.heap =
        Span::from_base_size(heap.start.as_raw() as *mut u8, kconfig::PAGE_SIZE);
    alloc.oom_handler.max = heap.end.as_raw();

    OomHandler::ensure_mapped(frame_alloc, None, alloc.oom_handler.heap)?;

    unsafe {
        let heap = alloc.oom_handler.heap;
        alloc.claim(heap).unwrap()
    };

    Ok(())
}

struct OomHandler {
    heap: Span,
    max: usize,
}

impl OomHandler {
    fn ensure_mapped(
        frame_alloc: &mut dyn FrameAllocator<kconfig::MEMORY_MODE>,
        old_heap: Option<Span>,
        new_heap: Span,
    ) -> Result<(), kmm::Error> {
        let span_to_map = if let Some(old_heap) = old_heap {
            let (empty, span_to_map) = new_heap.except(old_heap);
            assert!(empty.is_empty());
            span_to_map
        } else {
            new_heap
        };

        let mut mapper: Mapper<'_, kmm::Riscv64Sv39> = Mapper::from_active(0, frame_alloc);
        let mut flush = Flush::empty(0);

        let heap_phys = {
            assert_eq!(span_to_map.size() % kconfig::PAGE_SIZE, 0);
            let base = mapper
                .allocator_mut()
                .allocate_frames(span_to_map.size() / kconfig::PAGE_SIZE)?;
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
                0,
                old_heap
                    .size()
                    .max(layout.size())
                    .div_ceil(kconfig::PAGE_SIZE)
                    * kconfig::PAGE_SIZE,
            )
            .below(talc.oom_handler.max as *mut u8);

        log::trace!("Extending heap. Old {old_heap:?} => {new_heap:?}");

        if new_heap == old_heap {
            // we won't be extending the heap, so we should return Err
            return Err(());
        }

        with_frame_alloc(|alloc| OomHandler::ensure_mapped(alloc, Some(old_heap), new_heap))
            .inspect_err(|err| log::error!("failed to map extended heap! old heap {old_heap:?} new heap {new_heap:?} err {err:?}"))
            .map_err(|_| ())?;

        unsafe {
            // we're assuming the new memory up to HEAP_TOP_LIMIT is unused and allocatable
            talc.oom_handler.heap = talc.extend(old_heap, new_heap);
        }

        log::trace!("successfully extended heap {:?}", talc.oom_handler.heap);

        Ok(())
    }
}
