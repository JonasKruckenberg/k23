// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod arena;
mod frame;

use crate::arch;
use crate::hart_local::HartLocal;
use crate::vm::address::VirtualAddress;
use crate::vm::bootstrap_alloc::BootstrapAllocator;
use crate::vm::frame_list::FrameList;
use crate::vm::PhysicalAddress;
use alloc::vec::Vec;
use arena::{select_arenas, Arena};
use core::alloc::Layout;
use core::cell::RefCell;
use core::fmt::Formatter;
use core::ptr::NonNull;
use core::range::Range;
use core::sync::atomic::AtomicUsize;
use core::{cmp, fmt, iter, slice};
use fallible_iterator::FallibleIterator;
pub use frame::{Frame, FrameInfo};
use sync::{Mutex, OnceLock};

static FRAME_ALLOC: OnceLock<FrameAllocator> = OnceLock::new();

#[cold]
pub fn init(boot_alloc: BootstrapAllocator, fdt_region: Range<PhysicalAddress>) {
    #[expect(tail_expr_drop_order, reason = "")]
    FRAME_ALLOC.get_or_init(|| {
        let mut max_alignment = arch::PAGE_SIZE;
        let mut arenas = Vec::new();

        let phys_regions = boot_alloc.free_regions().chain(iter::once(fdt_region));
        for selection_result in select_arenas(phys_regions).iterator() {
            match selection_result {
                Ok(selection) => {
                    log::trace!("selection {selection:?}");
                    let arena = Arena::from_selection(selection);
                    max_alignment = cmp::max(max_alignment, arena.max_alignment());
                    arenas.push(arena);
                }
                Err(err) => {
                    log::error!("unable to include RAM region {:?}", err.range);
                }
            }
        }

        FrameAllocator {
            global: Mutex::new(GlobalFrameAllocator {
                arenas,
                max_alignment,
            }),
            frames_in_caches_hint: AtomicUsize::new(0),
            hart_local_cache: HartLocal::new(),
        }
    });
}

pub struct FrameAllocator {
    /// Global list of arenas that can be allocated from.
    global: Mutex<GlobalFrameAllocator>,
    /// Per-hart cache of frames to speed up allocation.
    hart_local_cache: HartLocal<RefCell<HartLocalFrameCache>>,
    /// Number of frames - across all harts - that are in hart-local caches.
    /// This value must only ever be treated as a hint and should only be used to
    /// produce more accurate frame usage statistics.
    frames_in_caches_hint: AtomicUsize,
}

/// Allocation failure that may be due to resource exhaustion or invalid combination of arguments
/// such as a too-large alignment. Importantly this error is *not-permanent*, a caller choosing to
/// retry allocation at a later point in time or with different arguments and might receive a successful
/// result.
#[derive(Debug)]
pub struct AllocError;

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("AllocError")
    }
}

impl core::error::Error for AllocError {}

/// Allocate a single [`Frame`].
pub fn alloc_one() -> Result<Frame, AllocError> {
    let alloc = FRAME_ALLOC
        .get()
        .expect("cannot access FRAME_ALLOC before it is initialized");

    let mut hart_local_cache = alloc.hart_local_cache.get_or_default().borrow_mut();
    let frame = hart_local_cache
        .allocate_one()
        .or_else(|| {
            let mut global_alloc = alloc.global.lock();

            global_alloc.allocate_one()
        })
        .ok_or(AllocError)?;

    // Safety: we just allocated the frame
    let frame = unsafe { Frame::from_free_info(frame) };

    #[cfg(debug_assertions)]
    frame.assert_valid();
    Ok(frame)
}

/// Allocate a single [`Frame`] and ensure the backing physical memory is zero initialized.
pub fn alloc_one_zeroed() -> Result<Frame, AllocError> {
    let frame = alloc_one()?;

    // Translate the physical address into a virtual one through the physmap
    let virt = VirtualAddress::from_phys(frame.addr()).unwrap();

    // memset'ing the slice to zero
    // Safety: the slice has just been allocated
    unsafe {
        slice::from_raw_parts_mut(virt.as_mut_ptr(), arch::PAGE_SIZE).fill(0);
    }

    Ok(frame)
}

/// Allocate a contiguous runs of [`Frame`] meeting the size and alignment requirements of `layout`.
pub fn alloc_contiguous(layout: Layout) -> Result<FrameList, AllocError> {
    let alloc = FRAME_ALLOC
        .get()
        .expect("cannot access FRAME_ALLOC before it is initialized");

    // try to allocate from the per-hart cache first
    let mut hart_local_cache = alloc.hart_local_cache.get_or_default().borrow_mut();
    let frames = hart_local_cache
        .allocate_contiguous(layout)
        .or_else(|| {
            let mut global_alloc = alloc.global.lock();

            log::trace!(
                "Hart-local cache exhausted, refilling {} frames...",
                layout.size() / arch::PAGE_SIZE
            );
            let mut frames = global_alloc.allocate_contiguous(layout)?;
            hart_local_cache.free_list.append(&mut frames);

            log::trace!("retrying allocation...");
            // If this fails then we failed to pull enough frames from the global allocator
            // which means we're fully out of frames
            hart_local_cache.allocate_contiguous(layout)
        })
        .ok_or(AllocError)?;

    let frames = FrameList::from_iter(frames.into_iter().map(|info| {
        // Safety: we just allocated the frame
        unsafe { Frame::from_free_info(info) }
    }));
    #[cfg(debug_assertions)]
    frames.assert_valid();
    Ok(frames)
}

/// Allocate a contiguous runs of [`Frame`] meeting the size and alignment requirements of `layout`
/// and ensuring the backing physical memory is zero initialized.
pub fn alloc_contiguous_zeroed(layout: Layout) -> Result<FrameList, AllocError> {
    let frames = alloc_contiguous(layout)?;

    // Translate the physical address into a virtual one through the physmap
    let virt = VirtualAddress::from_phys(frames.first().unwrap().addr()).unwrap();

    // memset'ing the slice to zero
    // Safety: the slice has just been allocated
    unsafe {
        slice::from_raw_parts_mut(virt.as_mut_ptr(), frames.size()).fill(0);
    }

    Ok(frames)
}

pub fn max_alignment() -> usize {
    let alloc = FRAME_ALLOC
        .get()
        .expect("cannot access FRAME_ALLOC before it is initialized");

    alloc.global.lock().max_alignment
}

struct GlobalFrameAllocator {
    arenas: Vec<Arena>,
    max_alignment: usize,
}

impl GlobalFrameAllocator {
    fn allocate_one(&mut self) -> Option<NonNull<FrameInfo>> {
        for arena in &mut self.arenas {
            if let Some(frame) = arena.allocate_one() {
                return Some(frame);
            }
        }

        None
    }

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<linked_list::List<FrameInfo>> {
        for arena in &mut self.arenas {
            if let Some(frames) = arena.allocate_contiguous(layout) {
                return Some(frames);
            }
        }

        None
    }
}

#[derive(Default)]
struct HartLocalFrameCache {
    free_list: linked_list::List<FrameInfo>,
}

impl HartLocalFrameCache {
    fn allocate_one(&mut self) -> Option<NonNull<FrameInfo>> {
        self.free_list.pop_front()
    }

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<linked_list::List<FrameInfo>> {
        let frames = layout.size() / arch::PAGE_SIZE;

        // short-circuit if the cache doesn't even have enough pages
        if self.free_list.len() < frames {
            return None;
        }

        let mut index = 0;
        let mut base = self.free_list.cursor_front();
        'outer: while let Some(base_frame) = base.get() {
            if base_frame.addr().alignment() >= layout.align() {
                let cursor = base.clone();
                let mut prev_addr = base_frame.addr();

                let mut c = 0;
                while let Some(frame) = cursor.get() {
                    // we found a contiguous block
                    if c == frames {
                        break 'outer;
                    }

                    if frame.addr().checked_sub_addr(prev_addr).unwrap() > arch::PAGE_SIZE {
                        // frames aren't contiguous, so let's try the next one
                        log::trace!("frames not contiguous, trying next");
                        continue 'outer;
                    }

                    c += 1;
                    prev_addr = frame.addr();
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
