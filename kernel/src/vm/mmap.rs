// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::{AddressSpace, Permissions, Vmo, THE_ZERO_FRAME};
use core::alloc::Layout;
use core::range::Range;
use core::{iter, slice};
use mmu::arch::PAGE_SIZE;
use mmu::{AddressRangeExt, VirtualAddress};

pub struct Mmap {
    range: Range<VirtualAddress>,
}

impl Drop for Mmap {
    fn drop(&mut self) {
        log::debug!("TODO unmap range {:?}", self.range);
    }
}

impl Mmap {
    pub fn new_empty() -> Self {
        Self {
            range: Range::default(),
        }
    }

    /// Creates a new memory mapping in the given address space. `len` bytes will be immediately
    /// accessible for `READ` & `WRITE`.
    pub fn new_eager(aspace: &mut AddressSpace, len: usize) -> crate::Result<Self> {
        let layout = Layout::from_size_align(len, PAGE_SIZE).unwrap();
        let vmo = Vmo::new_paged(iter::repeat_n(
            THE_ZERO_FRAME.clone(),
            layout.size().div_ceil(PAGE_SIZE),
        ));

        let region = aspace.map(layout, vmo, 0, Permissions::READ | Permissions::WRITE, None)?;

        Ok(Self {
            range: region.range,
        })
    }

    /// Reserve a region of memory without actually allocating it.
    pub fn new_lazy(aspace: &mut AddressSpace, len: usize) -> crate::Result<Self> {
        let layout = Layout::from_size_align(len, PAGE_SIZE).unwrap();
        let vmo = Vmo::new_paged(iter::repeat_n(
            THE_ZERO_FRAME.clone(),
            layout.size().div_ceil(PAGE_SIZE),
        ));

        let region = aspace.map(layout, vmo, 0, Permissions::empty(), None)?;

        Ok(Self {
            range: region.range,
        })
    }

    #[inline]
    pub unsafe fn slice(&self, range: Range<usize>) -> &[u8] {
        assert!(range.end <= self.len());
        let len = range.end.checked_sub(range.start).unwrap();
        slice::from_raw_parts(self.as_ptr().add(range.start), len)
    }

    #[inline]
    pub unsafe fn slice_mut(&mut self, range: Range<usize>) -> &mut [u8] {
        assert!(range.end <= self.len());
        let len = range.end.checked_sub(range.start).unwrap();
        slice::from_raw_parts_mut(self.as_mut_ptr().add(range.start), len)
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.range.start.as_ptr()
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.range.start.as_mut_ptr()
    }

    #[inline]
    pub fn len(&self) -> usize {
        // Safety: the constructor ensures that the NonNull is valid.
        self.range.size()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn make_accessible(
        &mut self,
        aspace: &mut AddressSpace,
        start: usize,
        len: usize,
    ) -> crate::Result<()> {
        assert!(start + len <= self.range.size());

        let start = self.range.start.checked_add(start).unwrap();

        assert!(start <= self.range.end);
        assert!(
            start.is_aligned_to(PAGE_SIZE),
            "changing of protections isn't page-aligned",
        );

        aspace.protect(
            Range::from(start..start.checked_add(len).unwrap()),
            Permissions::READ | Permissions::WRITE,
        )?;

        Ok(())
    }

    pub fn make_executable(
        &mut self,
        aspace: &mut AddressSpace,
        start: usize,
        len: usize,
    ) -> crate::Result<()> {
        assert!(start + len <= self.range.size());

        let start = self.range.start.checked_add(start).unwrap();

        assert!(start <= self.range.end);
        assert!(
            start.is_aligned_to(PAGE_SIZE),
            "changing of protections isn't page-aligned",
        );

        aspace.protect(
            Range::from(start..start.checked_add(len).unwrap()),
            Permissions::READ | Permissions::EXECUTE,
        )?;

        Ok(())
    }

    pub fn make_readonly(
        &mut self,
        aspace: &mut AddressSpace,
        start: usize,
        len: usize,
    ) -> crate::Result<()> {
        assert!(start + len <= self.range.size());

        let start = self.range.start.checked_add(start).unwrap();

        assert!(start <= self.range.end);
        assert!(
            start.is_aligned_to(PAGE_SIZE),
            "changing of protections isn't page-aligned",
        );

        aspace.protect(
            Range::from(start..start.checked_add(len).unwrap()),
            Permissions::READ,
        )?;

        Ok(())
    }
}
