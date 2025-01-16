// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::address::AddressRangeExt;
use crate::vm::address_space_region::AddressSpaceRegion;
use crate::vm::{AddressSpace, ArchAddressSpace, Error, Permissions, VirtualAddress};
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::range::Range;
use core::slice;

/// A memory mapping, essentially handle to an `AddressSpaceRegion`
pub struct MmapSlice {
    ptr: *mut AddressSpaceRegion,
    range: Range<VirtualAddress>,
}

impl MmapSlice {
    pub unsafe fn from_raw(ptr: *mut AddressSpaceRegion, range: Range<VirtualAddress>) -> Self {
        Self { ptr, range }
    }

    // /// Creates a new empty `Mmap`.
    // ///
    // /// Note that the size of this cannot be changed after the fact, all accessors will return empty
    // /// slices and permission changing methods will always fail.
    // pub fn new_empty() -> Self {
    //     Self {
    //         ptr: ptr::null_mut(),
    //         range: Range::default(),
    //     }
    // }
    // /// Creates a new read-write (`RW`) memory mapping in the given address space.
    // pub fn new(aspace: &mut AddressSpace, len: usize) -> Result<Self, Error> {
    //     let layout = Layout::from_size_align(len, arch::PAGE_SIZE).unwrap();
    //     let vmo = Vmo::new_paged(iter::repeat_n(
    //         THE_ZERO_FRAME.clone(),
    //         layout.size().div_ceil(arch::PAGE_SIZE),
    //     ));
    //
    //     let region = aspace.map(layout, vmo, 0, Permissions::READ | Permissions::WRITE, None)?;
    //
    //     #[allow(tail_expr_drop_order)]
    //     Ok(Self {
    //         range: region.range,
    //         ptr: ptr::from_mut(unsafe { Pin::into_inner_unchecked(region) }),
    //     })
    // }

    /// Returns a slice to the memory mapped by this `Mmap`.
    #[inline]
    pub unsafe fn slice(&self, range: Range<usize>) -> &[u8] {
        assert!(range.end <= self.len());
        let len = range.end.checked_sub(range.start).unwrap();
        unsafe { slice::from_raw_parts(self.as_ptr().add(range.start), len) }
    }

    /// Returns a mutable slice to the memory mapped by this `Mmap`.
    #[inline]
    pub unsafe fn slice_mut(&mut self, range: Range<usize>) -> &mut [u8] {
        assert!(range.end <= self.len());
        let len = range.end.checked_sub(range.start).unwrap();
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr().add(range.start), len) }
    }

    /// Returns a pointer to the start of the memory mapped by this `Mmap`.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.range.start.as_ptr()
    }

    /// Returns a mutable pointer to the start of the memory mapped by this `Mmap`.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.range.start.as_mut_ptr()
    }

    /// Returns the size in bytes of this memory mapping.
    #[inline]
    pub fn len(&self) -> usize {
        // Safety: the constructor ensures that the NonNull is valid.
        self.range.size()
    }

    /// Whether this is a mapping of zero bytes
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Mark this memory mapping as executable (`RX`) this will by-design make it not-writable too.
    pub fn make_executable(&mut self, aspace: &mut AddressSpace) -> Result<(), Error> {
        self.protect(aspace, Permissions::READ | Permissions::EXECUTE)
    }

    /// Mark this memory mapping as read-only (`R`) essentially removing the write permission.
    pub fn make_readonly(&mut self, aspace: &mut AddressSpace) -> Result<(), Error> {
        self.protect(aspace, Permissions::READ)
    }

    fn protect(
        &mut self,
        aspace: &mut AddressSpace,
        new_permissions: Permissions,
    ) -> Result<(), Error> {
        if let Some(ptr) = NonNull::new(self.ptr) {
            let mut c = unsafe { aspace.regions.cursor_mut_from_ptr(ptr) };
            let mut region = c.get_mut().unwrap();

            region.permissions = new_permissions;

            let mut flush = aspace.arch.new_flush();
            unsafe {
                aspace.arch.protect(
                    self.range.start,
                    NonZeroUsize::new(self.range.size()).unwrap(),
                    new_permissions.into(),
                    &mut flush,
                )?
            };
            flush.flush()?;
        }

        Ok(())
    }
}
