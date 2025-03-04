// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::bootstrap_alloc::BootstrapAllocator;
use crate::{INITIAL_HEAP_SIZE_PAGES, arch};
use core::alloc::Layout;
use core::range::Range;
use loader_api::BootInfo;
use talc::{ErrOnOom, Span, Talc, Talck};

#[global_allocator]
static KERNEL_ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();

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
    tracing::debug!("Kernel Heap: {virt:#x?}");

    let mut alloc = KERNEL_ALLOCATOR.lock();
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
