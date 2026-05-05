// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod arena;
mod frame;

use alloc::vec::Vec;
use core::alloc::Layout;
use core::cell::RefCell;
use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;
use core::{cmp, fmt, slice};

use arena::{Arena, select_arenas};
use cordyceps::list::List;
use cpu_local::cpu_local;
use fallible_iterator::FallibleIterator;
pub use frame::{Frame, FrameInfo};
use mem_core::{PhysMap, PhysicalAddress};
use spin::{Mutex, OnceLock};

use crate::arch;
use crate::mem::bootstrap_alloc::BootstrapAllocator;

cpu_local! {
    /// Per-cpu cache of frames to speed up allocation.
    static CPU_LOCAL_CACHE: RefCell<CpuLocalFrameCache> = const { RefCell::new(CpuLocalFrameCache { free_list: List::new() }) };
}

pub static FRAME_ALLOC: OnceLock<FrameAllocator> = OnceLock::new();

pub fn init(
    boot_alloc: BootstrapAllocator,
    physical_memory_regions: loader_api::MemoryRegions,
    physmap: &'static PhysMap,
) -> &'static FrameAllocator {
    FRAME_ALLOC.get_or_init(|| FrameAllocator::new(boot_alloc, physical_memory_regions, physmap))
}

#[derive(Debug)]
pub struct FrameAllocator {
    /// Global list of arenas that can be allocated from.
    global: Mutex<GlobalFrameAllocator>,
    max_alignment: usize,
    /// Number of frames - across all cpus - that are in cpu-local caches.
    /// This value must only ever be treated as a hint and should only be used to
    /// produce more accurate frame usage statistics.
    frames_in_caches_hint: AtomicUsize,
    pub physmap: &'static PhysMap,
}

#[derive(Debug)]
struct GlobalFrameAllocator {
    arenas: Vec<Arena>,
}

#[derive(Debug, Default)]
struct CpuLocalFrameCache {
    free_list: List<FrameInfo>,
}

/// Allocation failure that may be due to resource exhaustion or invalid combination of arguments
/// such as a too-large alignment. Importantly this error is *not-permanent*, a caller choosing to
/// retry allocation at a later point in time or with different arguments and might receive a successful
/// result.
#[derive(Debug)]
pub struct AllocError;

// === impl FrameAllocator ===

impl FrameAllocator {
    pub fn new(
        _boot_alloc: BootstrapAllocator,
        physical_memory_regions: loader_api::MemoryRegions,
        physmap: &'static PhysMap,
    ) -> Self {
        let mut max_alignment = arch::PAGE_SIZE;
        let mut arenas = Vec::new();

        for selection_result in select_arenas(physical_memory_regions).iterator() {
            match selection_result {
                Ok(selection) => {
                    tracing::trace!("selection {selection:?}");
                    let arena = Arena::from_selection(selection, physmap);
                    tracing::trace!("max arena alignment {}", arena.max_alignment());
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
            physmap,
        }
    }

    /// Allocate a single [`Frame`].
    pub fn alloc_one(&self) -> Result<Frame, AllocError> {
        let mut cpu_local_cache = CPU_LOCAL_CACHE.borrow_mut();
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
        frame.assert_valid("FrameAllocator::alloc_one after allocation");

        Ok(frame)
    }

    /// Allocate a single [`Frame`] and ensure the backing physical memory is zero initialized.
    pub fn alloc_one_zeroed(&self, physmap: &PhysMap) -> Result<Frame, AllocError> {
        let frame = self.alloc_one()?;

        // Translate the physical address into a virtual one through the physmap
        let virt = physmap.phys_to_virt(frame.addr());

        // memset'ing the slice to zero
        // Safety: the slice has just been allocated
        unsafe {
            slice::from_raw_parts_mut(virt.as_mut_ptr(), arch::PAGE_SIZE).fill(0);
        }

        Ok(frame)
    }

    /// Allocate a contiguous runs of [`Frame`] meeting the size and alignment requirements of `layout`.
    pub fn alloc_contiguous(&self, layout: Layout) -> Result<List<FrameInfo>, AllocError> {
        // Fast path: try to satisfy from the per-cpu cache. The borrow must be
        // released before the slow path runs, otherwise the slow path's re-borrow
        // panics with "RefCell already borrowed" — non-reentrant by design.
        if let Some(frames) = CPU_LOCAL_CACHE.borrow_mut().allocate_contiguous(layout) {
            return Ok(frames);
        }

        tracing::trace!(
            "CPU-local cache exhausted, refilling {} frames...",
            layout.size() / arch::PAGE_SIZE
        );
        let mut refill = self
            .global
            .lock()
            .allocate_contiguous(layout)
            .ok_or(AllocError)?;

        let mut cpu_local_cache = CPU_LOCAL_CACHE.borrow_mut();
        cpu_local_cache.free_list.append(&mut refill);

        tracing::trace!("retrying allocation...");
        // If this fails then we failed to pull enough frames from the global allocator
        // which means we're fully out of frames.
        cpu_local_cache
            .allocate_contiguous(layout)
            .ok_or(AllocError)
    }

    /// Allocate a contiguous runs of [`Frame`] meeting the size and alignment requirements of `layout`
    /// and ensuring the backing physical memory is zero initialized.
    pub fn alloc_contiguous_zeroed(
        &self,
        layout: Layout,
        physmap: &PhysMap,
    ) -> Result<List<FrameInfo>, AllocError> {
        let frames = self.alloc_contiguous(layout)?;

        // Translate the physical address into a virtual one through the physmap
        let virt = physmap.phys_to_virt(frames.iter().next().unwrap().addr());

        // memset'ing the slice to zero
        // Safety: the slice has just been allocated
        unsafe {
            slice::from_raw_parts_mut(virt.as_mut_ptr(), frames.len()).fill(0);
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

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<List<FrameInfo>> {
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

    fn allocate_contiguous(&mut self, layout: Layout) -> Option<List<FrameInfo>> {
        let frames_needed = layout.size() / arch::PAGE_SIZE;
        if frames_needed == 0 || self.free_list.len() < frames_needed {
            return None;
        }
        let start = self.find_contiguous_run(frames_needed, layout.align())?;

        let mut split = self.free_list.split_off(start);
        let mut rest = split.split_off(frames_needed);
        self.free_list.append(&mut rest);
        Some(split)
    }

    /// Locate the index of a window of `frames_needed` cache entries that is
    /// contiguous in physical memory and starts on an `align`-aligned address.
    ///
    /// The cache is in insertion order, not address order, so we walk it once and
    /// grow a candidate run whenever the next entry sits exactly one page above the
    /// previous one. `wrapping_sub` is required for the contiguity test:
    /// `offset_from_unsigned` panics on out-of-order pairs, and historically that
    /// panic fired under the talc lock and deadlocked the kernel.
    fn find_contiguous_run(&self, frames_needed: usize, align: usize) -> Option<usize> {
        let mut start = 0;
        let mut run = 0;
        let mut prev: Option<PhysicalAddress> = None;
        for (i, frame) in self.free_list.iter().enumerate() {
            let addr = frame.addr();
            let extends_run =
                prev.is_some_and(|p| addr.get().wrapping_sub(p.get()) == arch::PAGE_SIZE);
            prev = Some(addr);

            if run > 0 && extends_run {
                run += 1;
            } else if addr.is_aligned_to(align) {
                (start, run) = (i, 1);
            } else {
                run = 0;
            }

            if run == frames_needed {
                return Some(start);
            }
        }
        None
    }
}

// === impl AllocError ===

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AllocError")
    }
}

impl core::error::Error for AllocError {}
