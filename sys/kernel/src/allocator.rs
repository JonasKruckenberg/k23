// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Kernel heap allocator.
//!
//! The heap is backed by `talc` and lives inside the higher-half direct map (HHDM): every
//! claimed span is just a contiguous physical run reinterpreted via `arch::phys_to_virt`. We
//! therefore never need to touch the kernel [`AddressSpace`](crate::mem::AddressSpace) when
//! growing the heap, which sidesteps the deadlock between `KERNEL_ALLOCATOR` and
//! `KERNEL_ASPACE` (mapping into the kernel address space itself allocates from the heap).

use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::range::Range;

use arrayvec::ArrayVec;
use loader_api::BootInfo;
use mem_core::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use static_assertions::const_assert;
use talc::base::Talc;
use talc::base::binning::{Binning, DefaultBinning};
use talc::source::Source;
use talc::{TalcLock, min_first_heap_size};

use crate::mem::bootstrap_alloc::BootstrapAllocator;
use crate::mem::frame_alloc::FrameAllocator;
use crate::{INITIAL_HEAP_SIZE_PAGES, arch};

/// Fixed-size growth chunk: 2 MiB on 4 KiB pages.
const HEAP_GROW_CHUNK_PAGES: usize = 512;

/// Default cap on total heap size when no `--heap-max` bootarg is provided. 1 GiB on 4 KiB pages.
const HEAP_DEFAULT_MAX_PAGES: usize = 256 * 1024;

/// Hard upper bound on the number of distinct talc heaps we'll ever claim.
const MAX_HEAP_CHUNKS: usize = 1024;

// The first claim must fit talc's gap-list metadata; subsequent claims have a much
// smaller minimum. See `Talc::claim` and `min_first_heap_size`.
const_assert!(INITIAL_HEAP_SIZE_PAGES * arch::PAGE_SIZE >= min_first_heap_size::<DefaultBinning>());

#[global_allocator]
static KERNEL_ALLOCATOR: TalcLock<spin::RawMutex, KernelHeapSource> =
    TalcLock::new(KernelHeapSource::new());

/// Auto-resizing memory source for the kernel heap. See module docs.
#[derive(Debug)]
pub struct KernelHeapSource {
    /// Wired up by [`late_init`] once the frame allocator exists. While `None` the source
    /// behaves like [`talc::source::Manual`], which is the correct fallback during early boot.
    frame_alloc: Option<&'static FrameAllocator>,
    /// Soft cap on total claimed pages, including the initial heap. `0` means unlimited.
    /// Soft because the underlying buddy allocator rounds requests up to a power of two, so
    /// a single grow may overshoot the cap by up to one chunk; subsequent grows are denied.
    max_total_pages: usize,
    /// Pages currently claimed (initial heap + every successful grow).
    total_pages: usize,
    /// Per-chunk records for diagnostics and the future shrink path.
    chunks: ArrayVec<HeapChunk, MAX_HEAP_CHUNKS>,
}

#[derive(Debug, Clone, Copy)]
struct HeapChunk {
    phys_start: PhysicalAddress,
    virt_start: VirtualAddress,
    pages: usize,
}

impl KernelHeapSource {
    const fn new() -> Self {
        Self {
            frame_alloc: None,
            max_total_pages: 0,
            total_pages: 0,
            chunks: ArrayVec::new(),
        }
    }
}

// SAFETY: `acquire` only mutates `KernelHeapSource` state and calls `talc.claim` on a
// freshly-allocated, exclusively-owned physical run via the stable HHDM mapping. It
// never re-enters the parent `TalcLock` directly or indirectly.
unsafe impl Source for KernelHeapSource {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, layout: Layout) -> Result<(), ()> {
        // TODO(metrics): increment a "kernel_heap.oom.invocations" counter here.

        // Snapshot what we need from the source up front so we don't have to juggle reborrows
        // around the call to `talc.claim`.
        let fa = talc.source.frame_alloc.ok_or(())?;
        if talc.source.chunks.is_full() {
            // TODO(metrics): increment "kernel_heap.oom.chunk_table_full".
            return Err(());
        }
        let max_total = talc.source.max_total_pages;
        let total = talc.source.total_pages;

        // Round to whole pages with worst-case alignment slack, plus one page for talc's
        // per-claim metadata. Without that slack, a layout sized within a few words of a page
        // boundary would loop: the chunk fits raw bytes but not talc-managed bytes, so talc
        // retries with the same layout until we hit `max_total_pages`.
        let needed_pages = layout
            .size()
            .saturating_add(layout.align() - 1)
            .div_ceil(arch::PAGE_SIZE)
            .saturating_add(1);
        if max_total != 0 && max_total.saturating_sub(total) < needed_pages {
            // TODO(metrics): increment "kernel_heap.oom.cap_exceeded".
            return Err(());
        }
        let mut want_pages = needed_pages.max(HEAP_GROW_CHUNK_PAGES);
        // Trim to the remaining cap budget so one grow can't overshoot by up to a chunk.
        // The cap check above guarantees `max_total > total` here, so the sub is exact.
        if max_total != 0 {
            want_pages = want_pages.min(max_total - total);
        }

        let chunk_bytes = want_pages.checked_mul(arch::PAGE_SIZE).ok_or(())?;
        let chunk_layout = Layout::from_size_align(chunk_bytes, arch::PAGE_SIZE).map_err(|_| ())?;
        let frames = fa.alloc_contiguous(chunk_layout).map_err(|_| {
            // TODO(metrics): increment "kernel_heap.oom.frame_alloc_failed".
        })?;

        let phys_start = frames.iter().next().unwrap().addr();

        let virt_start = arch::phys_to_virt(phys_start);

        // Safety: we just exclusively allocated this physical run from the frame allocator and
        // the HHDM mapping is wired and stable for the rest of the kernel's lifetime.
        //
        // `claim` returns `None` only for regions below `CHUNK_UNIT`; we always pass at least
        // one page, so this is unreachable. If it ever does fail the physical run stays pinned
        // (`alloc_contiguous_pages_global` `mem::forget`s the frames) until a matching free
        // path exists — see TODO on that function.
        unsafe {
            talc.claim(virt_start.as_mut_ptr(), frames.len() * arch::PAGE_SIZE)
                .ok_or(())?;
        };

        // Capacity was checked before we did anything observable.
        // Safety: `chunks.is_full()` returned false above and we hold the talc lock, so no
        // other task can have pushed in between.
        unsafe {
            talc.source.chunks.push_unchecked(HeapChunk {
                phys_start,
                virt_start,
                pages: frames.len(),
            });
        }
        talc.source.total_pages = total.saturating_add(frames.len());

        // TODO(metrics): increment "kernel_heap.oom.grew" and update a "kernel_heap.total_pages"
        // gauge.

        Ok(())
    }
}

/// Set up the initial heap from the bootstrap allocator.
///
/// Runs before `frame_alloc::init`; the heap source stays in fallback mode (returning `Err`)
/// until [`late_init`] wires up the frame allocator.
pub fn init(boot_alloc: &mut BootstrapAllocator, boot_info: &BootInfo) {
    let layout =
        Layout::from_size_align(INITIAL_HEAP_SIZE_PAGES * arch::PAGE_SIZE, arch::PAGE_SIZE)
            .unwrap();

    let phys = boot_alloc.allocate_contiguous(layout).unwrap();

    let virt: Range<VirtualAddress> = {
        let start = boot_info.physmap.phys_to_virt(phys);
        Range::from_start_len(start, layout.size())
    };
    log::debug!("Kernel Heap: {virt:#x?} {phys:?}");

    let mut alloc = KERNEL_ALLOCATOR.lock();

    // Safety: just allocated the memory region. The compile-time assertion at the top of
    // this module guarantees the initial heap is large enough for talc's first-claim metadata.
    unsafe { alloc.claim(virt.start.as_mut_ptr(), virt.len()).unwrap() };

    // Safety: `chunks` is empty and `MAX_HEAP_CHUNKS >= 1`.
    unsafe {
        alloc.source.chunks.push_unchecked(HeapChunk {
            phys_start: phys,
            virt_start: virt.start,
            pages: INITIAL_HEAP_SIZE_PAGES,
        });
    }
    alloc.source.total_pages = INITIAL_HEAP_SIZE_PAGES;
}

/// Wire up the frame allocator and bootargs-driven cap, enabling automatic heap growth.
///
/// Must be called after `frame_alloc::init`. `heap_max_bytes` is the value of the `--heap-max`
/// bootarg in bytes, or `None` to use the default cap.
pub fn late_init(fa: &'static FrameAllocator, heap_max_bytes: Option<NonZeroUsize>) {
    let max_total_pages = match heap_max_bytes {
        Some(bytes) => bytes.get() / arch::PAGE_SIZE,
        None => HEAP_DEFAULT_MAX_PAGES,
    };

    {
        let mut alloc = KERNEL_ALLOCATOR.lock();
        alloc.source.frame_alloc = Some(fa);
        alloc.source.max_total_pages = max_total_pages;
    }

    // Trace outside the lock: tracing macros may allocate, and the kernel allocator's spinlock
    // is non-reentrant.
    if max_total_pages == 0 {
        tracing::debug!(
            "Kernel heap auto-grow enabled: chunk={} pages, cap=unlimited",
            HEAP_GROW_CHUNK_PAGES,
        );
    } else {
        tracing::debug!(
            "Kernel heap auto-grow enabled: chunk={} pages, cap={} pages",
            HEAP_GROW_CHUNK_PAGES,
            max_total_pages,
        );
    }
}
