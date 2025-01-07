mod arena;
mod frame;

use crate::thread_local::ThreadLocal;
use crate::BOOT_INFO;
use alloc::vec::Vec;
use arena::select_arenas;
use arena::Arena;
use core::alloc::Layout;
use core::cell::RefCell;
use core::ptr;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use fallible_iterator::FallibleIterator;
pub use frame::Frame;
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::{BootstrapAllocator, FrameUsage};
use mmu::{PhysicalAddress, VirtualAddress};
use sync::{Mutex, OnceLock};

pub static FRAME_ALLOC: OnceLock<FrameAllocator> = OnceLock::new();

#[cold]
pub fn init(boot_alloc: BootstrapAllocator, phys_offset: VirtualAddress) -> &'static FrameAllocator {
    FRAME_ALLOC.get_or_init(|| {
        let mut arenas = Vec::new();

        for selection_result in select_arenas(boot_alloc.free_regions()).iterator() {
            match selection_result {
                Ok(selection) => {
                    log::trace!("selection {selection:?}");
                    arenas.push(Arena::from_selection(selection, phys_offset));
                }
                Err(err) => {
                    log::error!("unable to include RAM region {:?}", err.range)
                }
            }
        }

        FrameAllocator {
            arenas: Mutex::new(arenas),
            frames_in_caches_hint: AtomicUsize::new(0),
            hart_local_cache: ThreadLocal::new(),
        }
    })
}

pub struct FrameAllocator {
    /// Global list of arenas that can be allocated from.
    arenas: Mutex<Vec<Arena>>,
    /// Per-hart cache of frames to speed up allocation.
    hart_local_cache: ThreadLocal<RefCell<linked_list::List<Frame>>>,
    /// Number of frames - across all harts - that are in hart-local caches.
    /// This value must only ever be treated as a hint and should only be used to
    /// produce more accurate frame usage statistics.
    frames_in_caches_hint: AtomicUsize,
}

impl FrameAllocator {
    pub fn allocate_one(&self) -> Option<NonNull<Frame>> {
        self.hart_local_allocate_one()
            .or_else(|| self.global_allocate_one())
    }

    pub fn allocate_contiguous(&self, layout: Layout) -> Option<linked_list::List<Frame>> {
        // try to allocate from the per-hart cache first
        self.hart_local_allocate_contiguous(layout).or_else(|| {
            log::trace!(
                "Hart-local cache exhausted, refilling {} frames...",
                layout.size() / PAGE_SIZE
            );

            self.refill_hart_local_cache(layout)?;

            log::trace!("retrying allocation...");
            // If this fails then we failed to pull enough frames from the global allocator
            // which means we're fully out of frames
            self.hart_local_allocate_contiguous(layout)
        })
    }

    fn refill_hart_local_cache(&self, layout: Layout) -> Option<()> {
        let mut frames = self.global_allocate_contiguous(layout)?;

        self.frames_in_caches_hint
            .fetch_add(frames.len(), Ordering::Relaxed);

        let mut hart_local_cache = self.hart_local_cache.get_or_default().borrow_mut();
        hart_local_cache.append(&mut frames);

        Some(())
    }

    fn hart_local_allocate_one(&self) -> Option<NonNull<Frame>> {
        let mut free_list = self.hart_local_cache.get_or_default().borrow_mut();
        let frame = free_list.pop_front()?;

        self.frames_in_caches_hint.fetch_sub(1, Ordering::Relaxed);

        Some(frame)
    }

    fn hart_local_allocate_contiguous(&self, layout: Layout) -> Option<linked_list::List<Frame>> {
        let mut free_list = self.hart_local_cache.get_or_default().borrow_mut();
        let frames = layout.size() / PAGE_SIZE;

        // short-circuit if the cache doesn't even have enough pages
        if free_list.len() < frames {
            return None;
        }

        let mut index = 0;
        let mut base = free_list.cursor_front();
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
        let mut split = free_list.split_off(index)?;
        // the split the contiguous block after the number of frames we need
        // and return the rest back to the cache
        let mut rest = split.split_off(frames).unwrap();
        free_list.append(&mut rest);

        self.frames_in_caches_hint
            .fetch_sub(split.len(), Ordering::Relaxed);

        Some(split)
    }

    fn global_allocate_one(&self) -> Option<NonNull<Frame>> {
        let mut arenas = self.arenas.lock();
        for arena in arenas.iter_mut() {
            if let Some(frame) = arena.allocate_one() {
                return Some(frame);
            }
        }

        None
    }

    fn global_allocate_contiguous(&self, layout: Layout) -> Option<linked_list::List<Frame>> {
        let mut arenas = self.arenas.lock();
        for arena in arenas.iter_mut() {
            if let Some(frames) = arena.allocate_contiguous(layout) {
                return Some(frames);
            }
        }

        None
    }
}

impl mmu::frame_alloc::FrameAllocator for &'_ FrameAllocator {
    fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let frames = FrameAllocator::allocate_contiguous(self, layout)?;
        Some(frames.front().unwrap().phys)
    }

    fn deallocate_contiguous(&mut self, _addr: PhysicalAddress, _layout: Layout) {
        todo!("FrameAllocator::deallocate_contiguous")
    }

    fn allocate_contiguous_zeroed(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let requested_size = layout.pad_to_align().size();
        let addr = self.allocate_contiguous(layout)?;

        let phys_offset = BOOT_INFO.get().unwrap().physical_address_offset;

        unsafe {
            ptr::write_bytes::<u8>(
                phys_offset.checked_add(addr.get()).unwrap().as_mut_ptr(),
                0,
                requested_size,
            )
        }
        Some(addr)
    }

    fn allocate_partial(&mut self, _layout: Layout) -> Option<(PhysicalAddress, usize)> {
        todo!("FrameAllocator::allocate_partial")
    }

    fn frame_usage(&self) -> FrameUsage {
        let mut frame_usage = FrameUsage::default();

        let arenas = self.arenas.lock();
        for arena in arenas.iter() {
            let FrameUsage { used, total } = arena.frame_usage();
            frame_usage.used += used;
            frame_usage.total += total;
        }

        // frames that are in hart-local caches are counted by the global arenas as "used"
        // so to get the accurate usage counts we need to subtract the cache freelist lengths
        frame_usage.used -= self.frames_in_caches_hint.load(Ordering::Relaxed);

        frame_usage
    }
}
