// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::string::String;
use alloc::sync::Arc;
use core::alloc::Layout;
use core::ops::Range;
use core::{ptr, slice};

use kmem_core::{
    AddressRangeExt, Arch, Flush, MemoryAttributes, PhysicalAddress, VirtualAddress, WriteOrExecute,
};
use spin::Mutex;

use crate::arch;
use crate::mem::{AddressSpace, AddressSpaceRegion, Batch};

/// A memory mapping.
///
/// This is essentially a handle to an [`AddressSpaceRegion`] with convenience methods for userspace
/// specific needs such as copying from and to memory.
#[derive(Debug)]
pub struct Mmap {
    aspace: Option<Arc<Mutex<AddressSpace<arch::KmemArch>>>>,
    range: Range<VirtualAddress>,
}

// Safety: All mutations of the `*mut AddressSpaceRegion` are happening through a `&mut AddressSpace`
unsafe impl Send for Mmap {}
// Safety: All mutations of the `*mut AddressSpaceRegion` are happening through a `&mut AddressSpace`
unsafe impl Sync for Mmap {}

impl Mmap {
    /// Creates a new empty `Mmap`.
    ///
    /// Note that the size of this cannot be changed after the fact, all accessors will return empty
    /// slices and permission changing methods will always fail.
    pub const fn new_empty() -> Self {
        Self {
            aspace: None,
            range: Range {
                start: VirtualAddress::MIN,
                end: VirtualAddress::MIN,
            },
        }
    }

    /// Creates a new read-write (`RW`) memory mapping in the given address space.
    pub fn new_zeroed(
        aspace: Arc<Mutex<AddressSpace<arch::KmemArch>>>,
        len: usize,
        align: usize,
        name: Option<String>,
    ) -> crate::Result<Self> {
        debug_assert!(
            align >= arch::PAGE_SIZE,
            "alignment must be at least a page"
        );

        let layout = Layout::from_size_align(len, align).unwrap();

        let mut aspace_ = aspace.lock();
        let range = aspace_
            .map(
                layout,
                MemoryAttributes::new()
                    .with(MemoryAttributes::READ, true)
                    .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
                |range, attributes, batch| {
                    Ok(AddressSpaceRegion::new_zeroed(
                        batch.frame_alloc,
                        range,
                        attributes,
                        name,
                    ))
                },
            )?
            .range
            .clone();
        drop(aspace_);

        tracing::trace!("new_zeroed: {len} {range:?}");

        Ok(Self {
            aspace: Some(aspace),
            range,
        })
    }

    pub fn new_phys(
        aspace: Arc<Mutex<AddressSpace<arch::KmemArch>>>,
        range_phys: Range<PhysicalAddress>,
        len: usize,
        align: usize,
        name: Option<String>,
    ) -> crate::Result<Self> {
        // debug_assert!(
        //     matches!(aspace.kind(), AddressSpaceKind::User),
        //     "cannot create UserMmap in kernel address space"
        // );
        debug_assert!(
            align >= arch::PAGE_SIZE,
            "alignment must be at least a page"
        );
        debug_assert!(len >= arch::PAGE_SIZE, "len must be at least a page");
        debug_assert_eq!(
            len % arch::PAGE_SIZE,
            0,
            "len must be a multiple of page size"
        );

        let layout = Layout::from_size_align(len, align).unwrap();

        let mut aspace_ = aspace.lock();
        let range = aspace_
            .map(
                layout,
                MemoryAttributes::new()
                    .with(MemoryAttributes::READ, true)
                    .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
                |range_virt, attributes, _batch| {
                    Ok(AddressSpaceRegion::new_phys(
                        range_virt,
                        attributes,
                        range_phys.clone(),
                        name,
                    ))
                },
            )?
            .range
            .clone();
        drop(aspace_);

        tracing::trace!("new_phys: {len} {range:?} => {range_phys:?}");

        Ok(Self {
            aspace: Some(aspace),
            range,
        })
    }

    pub fn range(&self) -> Range<VirtualAddress> {
        self.range.clone()
    }

    pub fn copy_from_userspace(
        &self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        src_range: Range<usize>,
        dst: &mut [u8],
    ) -> crate::Result<()> {
        self.with_user_slice(aspace, src_range, |src| dst.clone_from_slice(src))
    }

    pub fn copy_to_userspace(
        &mut self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        src: &[u8],
        dst_range: Range<usize>,
    ) -> crate::Result<()> {
        self.with_user_slice_mut(aspace, dst_range, |dst| {
            dst.copy_from_slice(src);
        })
    }

    pub fn with_user_slice<F>(
        &self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        range: Range<usize>,
        f: F,
    ) -> crate::Result<()>
    where
        F: FnOnce(&[u8]),
    {
        self.commit(aspace, range.clone(), false)?;

        aspace.raw.arch().fence(Range {
            start: self.range.start.add(range.start),
            end: self.range.start.add(range.end),
        });

        // Safety: checked by caller
        unsafe {
            let slice = slice::from_raw_parts(self.range.start.as_ptr(), self.range().len());

            f(&slice[range]);
        }

        Ok(())
    }

    pub fn with_user_slice_mut<F>(
        &mut self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        range: Range<usize>,
        f: F,
    ) -> crate::Result<()>
    where
        F: FnOnce(&mut [u8]),
    {
        self.commit(aspace, range.clone(), true)?;

        aspace.raw.arch().fence(Range {
            start: self.range.start.add(range.start),
            end: self.range.start.add(range.end),
        });

        // Safety: checked by caller
        unsafe {
            let slice =
                slice::from_raw_parts_mut(self.range.start.as_mut_ptr(), self.range().len());
            f(&mut slice[range]);
        }

        Ok(())
    }

    /// Returns a pointer to the start of the memory mapped by this `Mmap`.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        if self.range.is_empty() {
            return ptr::null();
        }

        let ptr = self.range.start.as_ptr();
        debug_assert!(!ptr.is_null());
        ptr
    }

    /// Returns a mutable pointer to the start of the memory mapped by this `Mmap`.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        if self.range.is_empty() {
            return ptr::null_mut();
        }

        let ptr = self.range.start.as_mut_ptr();
        debug_assert!(!ptr.is_null());
        ptr
    }

    /// Returns the size in bytes of this memory mapping.
    #[inline]
    pub fn len(&self) -> usize {
        // Safety: the constructor ensures that the NonNull is valid.
        self.range.len()
    }

    /// Whether this is a mapping of zero bytes
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Mark this memory mapping as executable (`RX`) this will by-design make it not-writable too.
    pub fn make_executable(
        &mut self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        _branch_protection: bool,
    ) -> crate::Result<()> {
        tracing::trace!("UserMmap::make_executable: {:?}", self.range);
        self.protect(
            aspace,
            MemoryAttributes::new()
                .with(MemoryAttributes::READ, true)
                .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute),
        )
    }

    /// Mark this memory mapping as read-only (`R`) essentially removing the write permission.
    pub fn make_readonly(
        &mut self,
        aspace: &mut AddressSpace<arch::KmemArch>,
    ) -> crate::Result<()> {
        tracing::trace!("UserMmap::make_readonly: {:?}", self.range);
        self.protect(
            aspace,
            MemoryAttributes::new().with(MemoryAttributes::READ, true),
        )
    }

    fn protect(
        &mut self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        new_attributes: MemoryAttributes,
    ) -> crate::Result<()> {
        if !self.range.is_empty() {
            let mut cursor = aspace.regions.find_mut(&self.range.start);
            let mut region = cursor.get_mut().unwrap();

            region.attributes = new_attributes;

            let mut flush = Flush::new();
            // Safety: constructors ensure invariants are maintained
            unsafe {
                aspace
                    .raw
                    .set_attributes(self.range, new_attributes.into(), &mut flush);
            };
            flush.flush(aspace.raw.arch());
        }

        Ok(())
    }

    pub fn commit(
        &self,
        aspace: &mut AddressSpace<arch::KmemArch>,
        range: Range<usize>,
        will_write: bool,
    ) -> crate::Result<()> {
        if !self.range.is_empty() {
            let mut cursor = aspace.regions.find_mut(&self.range.start);

            let src_range = Range {
                start: self.range.start.add(range.start),
                end: self.range.end.add(range.start),
            };

            let mut batch = Batch::new(&mut aspace.raw, aspace.frame_alloc);
            cursor
                .get_mut()
                .unwrap()
                .commit(&mut batch, src_range, will_write)?;
            batch.flush()?;
        }

        Ok(())
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        // A `None` means the Mmap got created through `Mmap::new_empty` so there is nothing to unmap
        if let Some(aspace) = &self.aspace {
            let mut aspace = aspace.lock();
            aspace.unmap(self.range.clone()).unwrap();
        }
    }
}
