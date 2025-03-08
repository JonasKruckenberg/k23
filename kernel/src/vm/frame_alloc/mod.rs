// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod arena;
mod frame;
pub mod frame_list;

use crate::arch;
use crate::cpu_local::CpuLocal;
use crate::vm::bootstrap_alloc::BootstrapAllocator;
use crate::vm::{PhysicalAddress, VirtualAddress};
use alloc::vec::Vec;
use arena::Arena;
use arena::select_arenas;
use core::alloc::Layout;
use core::cell::RefCell;
use core::ptr::NonNull;
use core::range::Range;
use core::sync::atomic::AtomicUsize;
use core::{cmp, fmt, iter, slice};
use fallible_iterator::FallibleIterator;
use spin::{Mutex, OnceLock};

use crate::vm::frame_alloc::frame_list::FrameList;
pub use frame::{Frame, FrameInfo};

pub static FRAME_ALLOC: OnceLock<FrameAllocator> = OnceLock::new();
pub fn init(
    boot_alloc: BootstrapAllocator,
    fdt_region: Range<PhysicalAddress>,
) -> &'static FrameAllocator {
    FRAME_ALLOC.get_or_init(|| FrameAllocator::new(boot_alloc, fdt_region))
}

#[derive(Debug)]
pub struct FrameAllocator {
    /// Global list of arenas that can be allocated from.
    global: Mutex<GlobalFrameAllocator>,
    max_alignment: usize,
    /// Per-cpu cache of frames to speed up allocation.
    cpu_local_cache: CpuLocal<RefCell<CpuLocalFrameCache>>,
    /// Number of frames - across all cpus - that are in cpu-local caches.
    /// This value must only ever be treated as a hint and should only be used to
    /// produce more accurate frame usage statistics.
    frames_in_caches_hint: AtomicUsize,
}

#[derive(Debug)]
struct GlobalFrameAllocator {
    arenas: Vec<Arena>,
}

#[derive(Debug, Default)]
struct CpuLocalFrameCache {
    free_list: linked_list::List<FrameInfo>,
}

/// Allocation failure that may be due to resource exhaustion or invalid combination of arguments
/// such as a too-large alignment. Importantly this error is *not-permanent*, a caller choosing to
/// retry allocation at a later point in time or with different arguments and might receive a successful
/// result.
#[derive(Debug)]
pub struct AllocError;

// === impl FrameAllocator ===

impl FrameAllocator {
    pub fn new(boot_alloc: BootstrapAllocator, fdt_region: Range<PhysicalAddress>) -> Self {
        let mut max_alignment = arch::PAGE_SIZE;
        let mut arenas = Vec::new();

        let phys_regions: Vec<_> = boot_alloc.free_regions().chain(iter::once(fdt_region)).collect();
        for selection_result in select_arenas(phys_regions.into_iter()).iterator() {
            match selection_result {
                Ok(selection) => {
                    tracing::trace!("selection {selection:?}");
                    let arena = Arena::from_selection(selection);
                    max_alignment = cmp::max(max_alignment, arena.max_alignment());
                    arenas.push(arena);
                }
                Err(err) => {
                    tracing::error!("unable to include RAM region {:?}", err.range);
                }
            }
        }

        FrameAllocator {
            global: Mutex::new(GlobalFrameAllocator { arenas }),
            max_alignment,
            frames_in_caches_hint: AtomicUsize::new(0),
            cpu_local_cache: CpuLocal::new(),
        }
    }

    /// Allocate a single [`Frame`].
    pub fn alloc_one(&self) -> Result<Frame, AllocError> {
        let mut cpu_local_cache = self.cpu_local_cache.get_or_default().borrow_mut();
        let frame = cpu_local_cache
            .allocate_one()
            .or_else(|| {
                let mut global_alloc = self.global.lock();

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
    pub fn alloc_one_zeroed(&self) -> Result<Frame, AllocError> {
        let frame = self.alloc_one()?;

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
    pub fn alloc_contiguous(&self, layout: Layout) -> Result<FrameList, AllocError> {
        // try to allocate from the per-cpu cache first
        let mut cpu_local_cache = self.cpu_local_cache.get_or_default().borrow_mut();
        let frames = cpu_local_cache
            .allocate_contiguous(layout)
            .or_else(|| {
                let mut global_alloc = self.global.lock();

                tracing::trace!(
                    "CPU-local cache exhausted, refilling {} frames...",
                    layout.size() / arch::PAGE_SIZE
                );
                let mut frames = global_alloc.allocate_contiguous(layout)?;
                cpu_local_cache.free_list.append(&mut frames);

                tracing::trace!("retrying allocation...");
                // If this fails then we failed to pull enough frames from the global allocator
                // which means we're fully out of frames
                cpu_local_cache.allocate_contiguous(layout)
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
    pub fn alloc_contiguous_zeroed(&self, layout: Layout) -> Result<FrameList, AllocError> {
        let frames = self.alloc_contiguous(layout)?;

        // Translate the physical address into a virtual one through the physmap
        let virt = VirtualAddress::from_phys(frames.first().unwrap().addr()).unwrap();

        // memset'ing the slice to zero
        // Safety: the slice has just been allocated
        unsafe {
            slice::from_raw_parts_mut(virt.as_mut_ptr(), frames.size()).fill(0);
        }

        Ok(frames)
    }

    pub fn max_alignment(&self) -> usize {
        self.max_alignment
    }
}

// === impl GlobalFrameAllocator ===

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

// === impl CpuLocalFrameCache ===

impl CpuLocalFrameCache {
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
                        tracing::trace!("frames not contiguous, trying next");
                        continue 'outer;
                    }

                    c += 1;
                    prev_addr = frame.addr();
                }
            }

            tracing::trace!("base frame not aligned, trying next");
            // the base wasn't aligned, try the next one
            index += 1;
            base.move_next();
        }

        tracing::trace!("found contiguous block at index {index}");

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

// === impl AllocError ===

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AllocError")
    }
}

impl core::error::Error for AllocError {}
