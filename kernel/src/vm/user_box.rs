// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::string::String;
use core::alloc::Layout;
use core::{cmp, ptr, slice};
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::NonNull;
use core::range::Range;
use crate::arch;
use crate::util::send_sync_ptr::SendSyncPtr;
use crate::vm::{AddressSpace, Error, UserMmap};

#[derive(Debug)]
pub struct UserBox<T: ?Sized> {
    ptr: SendSyncPtr<T>,
    mmap: UserMmap,
}

impl<T> UserBox<T> {
    pub fn new(aspace: &mut AddressSpace, data: T, name: Option<String>) -> Result<Self, Error> {
        let layout = Layout::new::<T>();

        let mut mmap = UserMmap::new_zeroed(aspace, layout.size(), cmp::max(layout.align(), arch::PAGE_SIZE), name)?;

        // Safety: yikes
        unsafe {
            let src: &[u8] = slice::from_raw_parts(ptr::from_ref(&data).cast(), size_of_val(&data));
            mmap.copy_to_userspace(aspace, src, Range::from(0..src.len()))?;

            let ptr = SendSyncPtr::new(NonNull::new_unchecked(mmap.as_mut_ptr().cast()));

            Ok(Self { ptr, mmap })
        }
    }

    pub const fn into_pin(boxed: Self) -> Pin<Self> {
        // It's not possible to move or replace the insides of a `Pin<Box<T>>`
        // when `T: !Unpin`, so it's safe to pin it directly without any
        // additional requirements.
        unsafe { Pin::new_unchecked(boxed) }
    }
}

impl<T: ?Sized> AsRef<T> for UserBox<T> {
    fn as_ref(&self) -> &T {
        // Safety: constructor ensures ptr is valid and immutable reference can always be taken out
        unsafe { self.ptr.as_ref() }
    }
}
impl<T: ?Sized> AsMut<T> for UserBox<T> {
    fn as_mut(&mut self) -> &mut T {
        // Safety: constructor ensures ptr is valid, and we're taking a mutable reference to the box
        // which means access is already synchronized.
        unsafe { self.ptr.as_mut() }
    }
}
impl<T: ?Sized> Deref for UserBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}
impl<T: ?Sized> DerefMut for UserBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}