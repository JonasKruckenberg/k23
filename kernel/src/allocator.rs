// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::bootstrap_alloc::BootstrapAllocator;
use crate::{arch, INITIAL_HEAP_SIZE_PAGES};
use core::alloc::{GlobalAlloc, Layout};
use core::range::Range;
use loader_api::BootInfo;
use talc::{ErrOnOom, Span, Talc, Talck};

#[global_allocator]
static KERNEL_ALLOCATOR: Alloc = Alloc(Talc::new(ErrOnOom).lock());

pub fn init(boot_alloc: &mut BootstrapAllocator, boot_info: &BootInfo) {
    let layout =
        Layout::from_size_align(INITIAL_HEAP_SIZE_PAGES * arch::PAGE_SIZE, arch::PAGE_SIZE)
            .unwrap();

    let phys = boot_alloc.allocate_contiguous(layout).unwrap();

    let virt = {
        let start = boot_info
            .physical_address_offset
            .checked_add(phys.get())
            .unwrap();

        Range::from(start..start.checked_add(layout.size()).unwrap())
    };

    let mut alloc = KERNEL_ALLOCATOR.0.lock();
    let span = Span::from_base_size(
        virt.start as *mut u8,
        virt.end.checked_sub(virt.start).unwrap(),
    );

    // Safety: just allocated the memory region
    unsafe {
        let old_heap = alloc.claim(span).unwrap();
        alloc.extend(old_heap, span);
    }
}

struct Alloc(Talck<sync::RawMutex, ErrOnOom>);

unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            // log::trace!("Alloc::alloc({layout:?}) ...");
            let ptr = self.0.alloc(layout);
            // log::trace!("-> {ptr:?}");
            ptr
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            // log::trace!("Alloc::dealloc({ptr:?}, {layout:?}) ...");
            self.0.dealloc(ptr, layout);
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe {
            // log::trace!("Alloc::alloc_zeroed({layout:?}) ...");
            let ptr = self.0.alloc_zeroed(layout);
            // log::trace!("-> {ptr:?}");
            ptr
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe {
            // log::trace!("Alloc::realloc({ptr:?}, {layout:?}, {new_size:?}) ...");
            let ptr = self.0.realloc(ptr, layout, new_size);
            // log::trace!("-> {ptr:?}");
            ptr
        }
    }
}