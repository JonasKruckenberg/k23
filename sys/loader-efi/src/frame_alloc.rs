// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::iter;
use core::num::NonZero;
use core::ptr::NonNull;
use core::range::Range;

use mem_core::{AddressRangeExt, AllocError, FrameAllocator, PhysicalAddress};
use uefi::boot::{AllocateType, MemoryType};

/// A [`FrameAllocator`] backed by UEFI boot services `AllocatePages`.
///
/// Frames are allocated as `MemoryType::RESERVED` so the kernel does not later
/// classify them as reclaimable on handoff.
pub struct UefiFrameAlloc;

// SAFETY: `allocate_pages` hands out exclusively-owned, page-aligned physical
// frames; we never report a size we did not allocate and never alias frames.
unsafe impl FrameAllocator for UefiFrameAlloc {
    fn allocate(
        &self,
        layout: Layout,
    ) -> core::result::Result<impl ExactSizeIterator<Item = Range<PhysicalAddress>>, AllocError>
    {
        let block = self.allocate_contiguous(layout)?;
        Ok(iter::once(Range::from_start_len(block, layout.size())))
    }

    fn allocate_contiguous(
        &self,
        layout: Layout,
    ) -> core::result::Result<PhysicalAddress, AllocError> {
        if !crate::are_boot_services_active() {
            log::warn!("cannot use UefiFrameAllocator after exiting boot services");
            return Err(AllocError);
        }

        if layout.align() > uefi::boot::PAGE_SIZE {
            return Err(AllocError);
        }

        let pages = layout.pad_to_align().size().div_ceil(uefi::boot::PAGE_SIZE);
        let ptr = uefi::boot::allocate_pages(AllocateType::AnyPages, MemoryType::RESERVED, pages)
            .map_err(|_| AllocError)?;

        // SAFETY: `allocate_pages` returned `pages` fresh, exclusively-owned pages,
        // which is at least `layout.size()` writable bytes.
        unsafe {
            ptr.write_bytes(0, layout.size());
        }
        Ok(PhysicalAddress::new(ptr.addr().get()))
    }

    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout) {
        if !crate::are_boot_services_active() {
            log::warn!("cannot use UefiFrameAllocator after exiting boot services");
            return;
        }

        debug_assert!(layout.align() <= uefi::boot::PAGE_SIZE);

        let pages = layout.pad_to_align().size().div_ceil(uefi::boot::PAGE_SIZE);
        let ptr = NonNull::dangling().with_addr(NonZero::new(block.get()).unwrap());

        // SAFETY: the caller guarantees `block`/`layout` denote a live allocation
        // previously handed out by this allocator.
        if let Err(err) = unsafe { uefi::boot::free_pages(ptr, pages) } {
            log::warn!("failed to free {pages} pages at {block}: {err}");
        }
    }
}
