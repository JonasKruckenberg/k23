// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ops::Range;

use kmem_core::{AddressRangeExt, Arch, FrameAllocator, VirtualAddress};
use loader_api::BootInfo;
use talc::{ErrOnOom, Span, Talc, Talck};
use kmem_core::bootstrap::BootstrapAllocator;
use crate::{INITIAL_HEAP_SIZE_PAGES, arch};

#[global_allocator]
static KERNEL_ALLOCATOR: Talck<spin::RawMutex, ErrOnOom> = Talc::new(ErrOnOom).lock();

pub fn init<A>(boot_alloc: &mut BootstrapAllocator<spin::RawMutex>, boot_info: &BootInfo<arch::KmemArch>) where A: Arch {
    let layout =
        Layout::from_size_align(INITIAL_HEAP_SIZE_PAGES * arch::PAGE_SIZE, arch::PAGE_SIZE)
            .unwrap();

    let phys = boot_alloc.allocate_contiguous(layout).unwrap();

    let virt: Range<VirtualAddress> = {
        let start = boot_info.address_space.arch().phys_to_virt(phys);

        Range::from_start_len(start, layout.size())
    };
    tracing::debug!("Kernel Heap: {virt:#x?}");

    let mut alloc = KERNEL_ALLOCATOR.lock();
    let span = Span::from_base_size(virt.start.as_mut_ptr(), virt.len());

    // Safety: just allocated the memory region
    unsafe {
        let old_heap = alloc.claim(span).unwrap();
        alloc.extend(old_heap, span);
    }
}
