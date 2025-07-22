// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use core::fmt::Formatter;
use core::ops::{Deref, DerefMut};
use core::{fmt, ptr, slice};

use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Permissions: u8 {
        /// Allow reads from the memory region
        const READ = 1 << 0;
        /// Allow writes to the memory region
        const WRITE = 1 << 1;
        /// Allow code execution from the memory region
        const EXECUTE = 1 << 2;
        /// TODO
        const BRANCH_PREDICTION = 1 << 3;
    }
}

#[derive(Debug)]
pub struct RawMmapVTable {
    pub protect: unsafe fn(data: *mut (), permissions: Permissions) -> anyhow::Result<()>,
    pub as_ptr: unsafe fn(data: *mut ()) -> *mut u8,
    pub len: unsafe fn(data: *mut ()) -> usize,
}

#[derive(Debug)]
pub struct RawMmap {
    data: *mut (),
    vtable: &'static RawMmapVTable,
}

pub struct Mmap {
    name: Cow<'static, str>,
    permissions: Permissions,
    mmap: RawMmap,
}

// ===== impl Mmap =====

impl Unpin for Mmap {}

unsafe impl Send for Mmap {}

impl Mmap {
    pub const unsafe fn from_raw(mmap: RawMmap) -> Self {
        Self {
            name: Cow::Borrowed("<unnamed mystery Mmap>"),
            permissions: Permissions::empty(),
            mmap,
        }
    }

    pub const fn new_empty() -> Mmap {
        unsafe {
            Self::from_raw(RawMmap {
                data: ptr::null_mut(),
                vtable: &RawMmap::EMPTY_VTABLE,
            })
        }
    }

    #[must_use]
    pub fn named(mut self, name: String) -> Self {
        self.name = Cow::Owned(name);
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn make_executable(&mut self, branch_protection: bool) -> anyhow::Result<()> {
        let mut flags = Permissions::READ | Permissions::EXECUTE;
        flags.set(Permissions::BRANCH_PREDICTION, branch_protection);
        self.mmap.protect(flags)
    }

    pub fn make_read_only(&mut self) -> anyhow::Result<()> {
        self.mmap.protect(Permissions::READ)
    }

    pub fn make_mut(&mut self) -> anyhow::Result<()> {
        self.mmap.protect(Permissions::READ | Permissions::WRITE)
    }

    /// Gets the `data` pointer used to create this `Mmap`.
    #[inline]
    #[must_use]
    pub fn data(&self) -> *mut () {
        self.mmap.data
    }

    /// Gets the `vtable` pointer used to create this `Mmap`.
    #[inline]
    #[must_use]
    pub fn vtable(&self) -> &'static RawMmapVTable {
        self.mmap.vtable
    }
}

impl fmt::Debug for Mmap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mmap")
            .field("name", &self.name)
            .field("permissions", &self.permissions)
            .field("raw", &self.mmap)
            .field("<mmap range>", &self.as_ptr_range())
            .finish()
    }
}

impl fmt::Display for Mmap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}, {:?}, ", self.name, self.as_ptr_range(),)?;
        bitflags::parser::to_writer(&self.permissions, f)
    }
}

impl Deref for Mmap {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        let ptr = unsafe { (self.mmap.vtable.as_ptr)(self.mmap.data) };
        let len = unsafe { (self.mmap.vtable.len)(self.mmap.data) };
        unsafe { slice::from_raw_parts(ptr, len) }
    }
}

impl DerefMut for Mmap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let ptr = unsafe { (self.mmap.vtable.as_ptr)(self.mmap.data) };
        let len = unsafe { (self.mmap.vtable.len)(self.mmap.data) };
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }
}

// === impl RawMmap ===

impl RawMmap {
    pub fn from_parts(data: *mut (), vtable: &'static RawMmapVTable) -> Self {
        Self { data, vtable }
    }

    pub fn protect(&mut self, permissions: Permissions) -> anyhow::Result<()> {
        unsafe { (self.vtable.protect)(self.data, permissions) }
    }

    pub fn as_ptr(&self) -> *const u8 {
        unsafe { (self.vtable.as_ptr)(self.data) }
    }

    pub fn len(&self) -> usize {
        unsafe { (self.vtable.len)(self.data) }
    }

    // ===== Empty VTable methods =====

    const EMPTY_VTABLE: RawMmapVTable = RawMmapVTable {
        protect: Self::empty_protect,
        as_ptr: Self::empty_as_ptr,
        len: Self::empty_len,
    };

    unsafe fn empty_new_zeroed() -> anyhow::Result<RawMmap> {
        Ok(Self::from_parts(ptr::null_mut(), &Self::EMPTY_VTABLE))
    }

    unsafe fn empty_protect(data: *mut (), _permissions: Permissions) -> anyhow::Result<()> {
        debug_assert!(data.is_null());
        Ok(())
    }

    unsafe fn empty_as_ptr(data: *mut ()) -> *mut u8 {
        debug_assert!(data.is_null());
        ptr::null_mut()
    }

    unsafe fn empty_len(data: *mut ()) -> usize {
        debug_assert!(data.is_null());
        0
    }
}
