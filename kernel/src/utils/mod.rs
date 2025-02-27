// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::NonNull;

pub mod cache_padded;
pub mod maybe_uninit;

/// Helper to construct a `NonNull<T>` from a raw pointer to `T`, with null
/// checks elided in release mode.
#[cfg(debug_assertions)]
#[track_caller]
#[inline(always)]
pub(crate) unsafe fn non_null<T>(ptr: *mut T) -> NonNull<T> {
    NonNull::new(ptr).expect(
        "/!\\ constructed a `NonNull` from a null pointer! /!\\ \n\
        in release mode, this would have called `NonNull::new_unchecked`, \
        violating the `NonNull` invariant!",
    )
}

/// Helper to construct a `NonNull<T>` from a raw pointer to `T`, with null
/// checks elided in release mode.
///
/// This is the release mode version.
#[cfg(not(debug_assertions))]
#[inline(always)]
pub(crate) unsafe fn non_null<T>(ptr: *mut T) -> NonNull<T> {
    // Safety: ensured by caller
    unsafe { NonNull::new_unchecked(ptr) }
}
