mod arena;
mod frame;

use arena::{select_arenas, Arena};
use arrayvec::ArrayVec;
use core::alloc::Layout;
use core::cell::RefCell;
use core::ptr::NonNull;
pub use frame::Frame;
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::BootstrapAllocator;
use mmu::VirtualAddress;
use sync::{Mutex, OnceLock};
use thread_local::thread_local;

static GLOBAL_FRAME_ALLOCATOR: OnceLock<Mutex<GlobalFrameAllocator>> = OnceLock::new();

thread_local! {
    static HART_LOCAL_FRAME_CACHE: RefCell<PerHartFrameCache> = RefCell::new(PerHartFrameCache::default());
}

pub fn allocate_one() -> Option<NonNull<Frame>> {
    HART_LOCAL_FRAME_CACHE.with_borrow_mut(|hart_local_cache| {
        hart_local_cache.allocate_one().or_else(|| {
            let mut global_alloc = GLOBAL_FRAME_ALLOCATOR.get().unwrap().lock();

            global_alloc.allocate_one()
        })
    })
}

pub fn allocate_contiguous(layout: Layout) -> Option<linked_list::List<Frame>> {
    HART_LOCAL_FRAME_CACHE.with_borrow_mut(|hart_local_cache| {
        // try to allocate from the per-hart cache first
        hart_local_cache.allocate_contiguous(layout).or_else(|| {
            let mut global_alloc = GLOBAL_FRAME_ALLOCATOR.get().unwrap().lock();

            log::trace!(
                "Hart-local cache exhausted, refilling {} frames...",
                layout.size() / PAGE_SIZE
            );
            let mut frames = global_alloc.allocate_contiguous(layout)?;
            hart_local_cache.free_list.append(&mut frames);

            log::trace!("retrying allocation...");
            // If this fails then we failed to pull enough frames from the global allocator
            // which means we're fully out of frames
            hart_local_cache.allocate_contiguous(layout)
        })
    })
}

pub fn deallocate(mut frames: linked_list::List<Frame>) {
    HART_LOCAL_FRAME_CACHE.with_borrow_mut(|hart_local_cache| {
        hart_local_cache.free_list.append(&mut frames);
    });
}

#[cold]
pub fn init(boot_alloc: BootstrapAllocator, phys_off: VirtualAddress) {
    GLOBAL_FRAME_ALLOCATOR.get_or_init(|| {
        let mut arenas: ArrayVec<_, 16> = ArrayVec::new();

        for selection_result in select_arenas(boot_alloc.free_regions()) {
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

        Mutex::new(GlobalFrameAllocator { arenas })
    });
}

struct GlobalFrameAllocator {
    arenas: ArrayVec<Arena, 16>,
}

impl GlobalFrameAllocator {
    fn allocate_one(&mut self) -> Option<NonNull<Frame>> {
        for arena in &mut self.arenas {
            if let Some(frame) = arena.allocate_one() {
                return Some(frame);
            }
        }

        None
    }

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<linked_list::List<Frame>> {
        for arena in &mut self.arenas {
            if let Some(frames) = arena.allocate_contiguous(layout) {
                return Some(frames);
            }
        }

        None
    }
}

#[derive(Default)]
struct PerHartFrameCache {
    free_list: linked_list::List<Frame>,
}

impl PerHartFrameCache {
    fn allocate_one(&mut self) -> Option<NonNull<Frame>> {
        self.free_list.pop_front()
    }

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<linked_list::List<Frame>> {
        let frames = layout.size() / PAGE_SIZE;

        // short-circuit if the cache doesn't even have enough pages
        if self.free_list.len() < frames {
            return None;
        }

        let mut index = 0;
        let mut base = self.free_list.cursor_front();
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
        let mut split = self.free_list.split_off(index)?;
        // the split the contiguous block after the number of frames we need
        // and return the rest back to the cache
        let mut rest = split.split_off(frames).unwrap();
        self.free_list.append(&mut rest);

        Some(split)
    }
}
