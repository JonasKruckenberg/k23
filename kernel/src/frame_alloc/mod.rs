use crate::error::Error;
use arena::Arena;
use arrayvec::ArrayVec;
use core::alloc::Layout;
use core::cell::RefCell;
use core::cmp;
use frame::Frame;
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::BootstrapAllocator;
use mmu::VirtualAddress;
use sync::{Mutex, OnceLock};
use thread_local::thread_local;

mod arena;
mod frame;

static FRAME_ALLOCATOR: OnceLock<Mutex<FrameAllocator>> = OnceLock::new();
#[cold]
pub fn init(boot_alloc: BootstrapAllocator, phys_off: VirtualAddress) {
    FRAME_ALLOCATOR.get_or_init(|| {
        let mut arenas: ArrayVec<_, 16> = ArrayVec::new();

        for selection_result in arena::select_arenas(boot_alloc.free_regions()) {
            match selection_result {
                Ok(selection) => {
                    log::trace!("selection {selection:?}");
                    arenas.push(Arena::from_selection(selection, phys_off));
                }
                Err(err) => {
                    log::error!("unable to include RAM region {:?}", err.range)
                }
            }
        }

        // arenas.sort_unstable_by(|a, b| -> Ordering {
        //     if a.range_phys().end <= b.range_phys().start {
        //         Ordering::Less
        //     } else if b.range_phys().end <= a.range_phys().start {
        //         Ordering::Greater
        //     } else {
        //         // This should never happen if the `exclude_region` code about is correct
        //         unreachable!("Memory region {a:?} and {b:?} are overlapping");
        //     }
        // });

        Mutex::new(FrameAllocator { arenas })
    });

    let out = allocate_frames(Layout::from_size_align(6 * PAGE_SIZE, PAGE_SIZE).unwrap()).unwrap();
    log::trace!("allocated {out:?}");
    assert_eq!(out.len(), 6);
    assert!(out
        .cursor_front()
        .get()
        .unwrap()
        .phys
        .is_aligned_to(PAGE_SIZE));

    let out = allocate_frames(Layout::from_size_align(PAGE_SIZE, 4 * PAGE_SIZE).unwrap()).unwrap();
    log::trace!("allocated {out:?}");
    assert_eq!(out.len(), 1);
    assert!(out
        .cursor_front()
        .get()
        .unwrap()
        .phys
        .is_aligned_to(4 * PAGE_SIZE));
}

thread_local!(static PER_HART_FRAME_CACHE: RefCell<linked_list::List<Frame>> = RefCell::new(linked_list::List::new()));

pub fn allocate_frames(layout: Layout) -> crate::Result<linked_list::List<Frame>> {
    PER_HART_FRAME_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();

        // try to allocate from the per-hart cache first
        if let Some(frames) = allocate_hart_local(&mut cache, layout) {
            Ok(frames)
        } else {
            let mut alloc = FRAME_ALLOCATOR.get().unwrap().lock();

            const AMORTIZATION_FACTOR: usize = 2;

            let refill_size =
                cmp::max(layout.size().next_power_of_two(), layout.align()) * AMORTIZATION_FACTOR;
            let refill_layout = Layout::from_size_align(refill_size, layout.align()).unwrap();

            log::trace!(
                "Hart-local cache exhausted, refilling {} frames...",
                layout.size() / PAGE_SIZE
            );
            alloc.allocate_contiguous(refill_layout, &mut cache)?;

            log::trace!("retrying allocation...");
            allocate_hart_local(&mut cache, layout).ok_or(Error::NoResources)
        }
    })
}

fn allocate_hart_local(
    cache: &mut linked_list::List<Frame>,
    layout: Layout,
) -> Option<linked_list::List<Frame>> {
    let frames = layout.size() / PAGE_SIZE;

    // short-circuit if the cache doesn't even have enough pages
    if cache.len() < frames {
        return None;
    }

    let mut index = 0;
    let mut base = cache.cursor_front();
    'outer: while let Some(base_frame) = base.get() {
        if base_frame.phys.alignment() >= layout.align() {
            let cursor = base.clone();
            let mut prev_addr = base_frame.phys;

            let mut c = 0;
            while let Some(frame) = cursor.get() {
                // we found a contiguous block
                if c == frames {
                    break 'outer;
                }

                if frame.phys.checked_sub_addr(prev_addr).unwrap() > PAGE_SIZE {
                    // frames aren't contiguous, so let's try the next one
                    log::trace!("frames not contiguous, trying next");
                    continue 'outer;
                }

                c += 1;
                prev_addr = frame.phys;
            }
        }

        log::trace!("base frame not aligned, trying next");
        // the base wasn't aligned, try the next one
        index += 1;
        base.move_next();
    }

    log::trace!("found contiguous block at index {index}");

    // split the cache first at the start of the contiguous block. This will return the contiguous block
    // plus everything after it
    let mut split = cache.split_off(index)?;
    // the split the contiguous block after the number of frames we need
    // and return the rest back to the cache
    let mut rest = split.split_off(frames).unwrap();
    cache.append(&mut rest);

    Some(split)
}

pub struct FrameAllocator {
    arenas: ArrayVec<Arena, 16>,
}

impl FrameAllocator {
    pub fn allocate_contiguous(
        &mut self,
        layout: Layout,
        list: &mut linked_list::List<Frame>,
    ) -> crate::Result<()> {
        for arena in &mut self.arenas {
            if arena.allocate_contiguous(layout, list).is_ok() {
                return Ok(());
            }
        }

        Err(Error::NoResources)
    }
}
