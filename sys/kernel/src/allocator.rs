// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::{GlobalAlloc, Layout};
use core::ops::Range;

use loader_api::BootInfo;
use mem_core::{AddressRangeExt, VirtualAddress};
use talc::{ErrOnOom, Span, Talc, Talck};

use crate::mem::bootstrap_alloc::BootstrapAllocator;
use crate::{INITIAL_HEAP_SIZE_PAGES, alloc_trace, arch};

/// Throwaway shim around the real allocator: records every successful
/// allocation into [`alloc_trace`]. The shim is unconditional but the trace
/// itself is gated by [`alloc_trace::enable`], so the overhead until that
/// switch flips is one acquire load.
pub struct TracingAlloc<A>(pub A);

// Safety: forwards every operation to the wrapped allocator unchanged. The
// trace recording happens after the inner call returns and cannot affect
// the returned pointer's validity.
unsafe impl<A: GlobalAlloc> GlobalAlloc for TracingAlloc<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Safety: forwarded to wrapped allocator
        let p = unsafe { self.0.alloc(layout) };
        if !p.is_null() {
            alloc_trace::record(layout);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // Safety: forwarded to wrapped allocator
        let p = unsafe { self.0.alloc_zeroed(layout) };
        if !p.is_null() {
            alloc_trace::record(layout);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Safety: forwarded to wrapped allocator
        unsafe { self.0.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Safety: forwarded to wrapped allocator
        let p = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            // Record the new layout as a fresh allocation; offline analysis
            // de-duplicates by callstack.
            if let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) {
                alloc_trace::record(new_layout);
            }
        }
        p
    }
}

#[global_allocator]
static KERNEL_ALLOCATOR: TracingAlloc<Talck<spin::RawMutex, ErrOnOom>> =
    TracingAlloc(Talc::new(ErrOnOom).lock());

pub fn init(boot_alloc: &mut BootstrapAllocator, boot_info: &BootInfo) {
    let layout =
        Layout::from_size_align(INITIAL_HEAP_SIZE_PAGES * arch::PAGE_SIZE, arch::PAGE_SIZE)
            .unwrap();

    let phys = boot_alloc.allocate_contiguous(layout).unwrap();

    let virt: Range<VirtualAddress> = {
        let start = boot_info.physical_address_offset.add(phys.get());

        Range::from_start_len(start, layout.size())
    };
    tracing::debug!("Kernel Heap: {virt:#x?}");

    let mut alloc = KERNEL_ALLOCATOR.0.lock();
    let span = Span::from_base_size(virt.start.as_mut_ptr(), virt.len());

    // Safety: just allocated the memory region
    unsafe {
        let old_heap = alloc.claim(span).unwrap();
        alloc.extend(old_heap, span);
    }
}
