// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod arena;
mod frame;
pub mod frame_list;

use alloc::vec::Vec;
use core::alloc::Layout;
use core::cell::RefCell;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;
use core::{cmp, iter, slice};

use arena::{select_arenas, Arena};
use cordyceps::list::List;
use cpu_local::collection::CpuLocal;
use fallible_iterator::FallibleIterator;
pub use frame::{Frame, FrameInfo};
use kmem_core::bootstrap::BootstrapAllocator;
use kmem_core::{AllocError, PhysicalAddress};
use spin::{Mutex, OnceLock};

use crate::arch;
use crate::mem::frame_alloc::frame_list::FrameList;

pub static FRAME_ALLOC: OnceLock<FrameAllocator> = OnceLock::new();
pub fn init<A: kmem_core::Arch>(
    boot_alloc: BootstrapAllocator<spin::RawMutex>,
    fdt_region: Range<PhysicalAddress>,
    arch: &A
) -> &'static FrameAllocator {
    FRAME_ALLOC.get_or_init(|| FrameAllocator::new(boot_alloc, fdt_region, arch))
}

#[derive(Debug)]
pub struct FrameAllocator {
    /// Global list of arenas that can be allocated from.
    global: Mutex<GlobalFrameAllocator>,
    max_block_size: NonZeroUsize,
    min_block_size: NonZeroUsize,
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
    free_list: List<FrameInfo>,
}

// === impl FrameAllocator ===

impl FrameAllocator {
    pub fn new<A: kmem_core::Arch>(
        boot_alloc: BootstrapAllocator<spin::RawMutex>,
        fdt_region: Range<PhysicalAddress>,
        arch: &A
    ) -> Self {
        let mut max_block_size = NonZeroUsize::new(arch::PAGE_SIZE).unwrap();
        let mut arenas = Vec::new();

        let phys_regions = boot_alloc
            .free_regions()
            .chain(iter::once(fdt_region))
            .collect();
        for selection_result in select_arenas(phys_regions).iterator() {
            match selection_result {
                Ok(selection) => {
                    tracing::trace!("selection {selection:?}");
                    let arena = Arena::from_selection(selection, arch);
                    tracing::trace!("max arena alignment {}", arena.max_block_size());
                    max_block_size = cmp::max(max_block_size, arena.max_block_size());
                    arenas.push(arena);
                }
                Err(err) => {
                    tracing::error!("unable to include RAM region {:?}", err.range);
                }
            }
        }

        FrameAllocator {
            global: Mutex::new(GlobalFrameAllocator { arenas }),
            max_block_size,
            min_block_size: NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
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
        frame.assert_valid("FrameAllocator::alloc_one after allocation");

        Ok(frame)
    }

    /// Allocate a single [`Frame`] and ensure the backing physical memory is zero initialized.
    pub fn alloc_one_zeroed<A: kmem_core::Arch>(&self, arch: &A) -> Result<Frame, AllocError> {
        let frame = self.alloc_one()?;

        // Translate the physical address into a virtual one through the physmap
        let virt = arch.phys_to_virt(frame.addr());

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
        frames.assert_valid("FrameAllocator::allocate_contiguous after allocation");

        Ok(frames)
    }

    /// Allocate a contiguous runs of [`Frame`] meeting the size and alignment requirements of `layout`
    /// and ensuring the backing physical memory is zero initialized.
    pub fn alloc_contiguous_zeroed<A: kmem_core::Arch>(
        &self,
        layout: Layout,
        arch: &A,
    ) -> Result<FrameList, AllocError> {
        let frames = self.alloc_contiguous(layout)?;

        // Translate the physical address into a virtual one through the physmap
        let virt = arch.phys_to_virt(frames.first().unwrap().addr());

        // memset'ing the slice to zero
        // Safety: the slice has just been allocated
        unsafe {
            slice::from_raw_parts_mut(virt.as_mut_ptr(), frames.size()).fill(0);
        }

        Ok(frames)
    }
}

unsafe impl kmem_core::FrameAllocator for FrameAllocator {
    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let _frames = self.alloc_contiguous(layout)?;

        todo!()
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {
        todo!()
    }

    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (self.min_block_size, Some(self.max_block_size))
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
        let frames = layout.size() / arch::PAGE_SIZE;

        // short-circuit if the cache doesn't even have enough pages
        if self.free_list.len() < frames {
            return None;
        }

        let mut index = 0;
        let mut base = self.free_list.iter();
        'outer: while let Some(base_frame) = base.next() {
            let address_alignment = base_frame.addr().get() & (!base_frame.addr().get() + 1);

            if address_alignment >= layout.align() {
                let mut prev_addr = base_frame.addr();

                let mut c = 0;
                for frame in base.by_ref() {
                    // we found a contiguous block
                    if c == frames {
                        break 'outer;
                    }

                    if frame.addr().offset_from_unsigned(prev_addr) > arch::PAGE_SIZE {
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
        }

        tracing::trace!("found contiguous block at index {index}");

        // split the cache first at the start of the contiguous block. This will return the contiguous block
        // plus everything after it
        let mut split = self.free_list.split_off(index);
        // the split the contiguous block after the number of frames we need
        // and return the rest back to the cache
        let mut rest = split.split_off(frames);
        self.free_list.append(&mut rest);

        Some(split)
    }
}
