// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::indices::VMSharedTypeIndex;
use core::fmt;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;

/// A pointer that is used by compiled code, or in other words is accessed
/// outside of Rust.
///
/// This type is pointer-sized and typed-like-a-pointer. This is additionally
/// like a `NonNull<T>` in that it's never a null pointer (and
/// `Option<VmPtr<T>>` is pointer-sized). This pointer auto-infers
/// `Send` and `Sync` based on `T`. Note the lack of `T: ?Sized` bounds in this
/// type additionally, meaning that it only works with sized types. That's
/// intentional as compiled code should not be interacting with dynamically
/// sized types in Rust.
///
/// This type serves two major purposes with respect to provenance and safety:
///
/// * Primarily this type is the only pointer type that implements `VmSafe`, the
///   marker trait below. That forces all pointers shared with compiled code to
///   use this type.
///
/// * This type represents a pointer with "exposed provenance". Once a value of
///   this type is created the original pointer's provenance will be marked as
///   exposed. This operation may hinder optimizations around the use of said
///   pointer in that case.
///
/// You should only use this type when sending pointers to compiled code.
#[repr(transparent)]
pub struct VmPtr<T> {
    ptr: NonZeroUsize,
    _marker: PhantomData<NonNull<T>>,
}

impl<T> VmPtr<T> {
    /// View this pointer as a [`NonNull<T>`].
    pub fn as_non_null(&self) -> NonNull<T> {
        let ptr = core::ptr::with_exposed_provenance_mut(self.ptr.get());
        unsafe { NonNull::new_unchecked(ptr) }
    }

    /// Similar to `as_send_sync`, but returns a `*mut T`.
    pub fn as_ptr(&self) -> *mut T {
        self.as_non_null().as_ptr()
    }
}

// `VmPtr<T>`, like raw pointers, is trivially `Clone`/`Copy`.
impl<T> Clone for VmPtr<T> {
    fn clone(&self) -> VmPtr<T> {
        *self
    }
}

impl<T> Copy for VmPtr<T> {}

// Forward debugging to `SendSyncPtr<T>` which renders the address.
impl<T> fmt::Debug for VmPtr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_non_null().fmt(f)
    }
}

impl<T> fmt::Pointer for VmPtr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.as_non_null(), f)
    }
}

// Constructor from `NonNull<T>`
impl<T> From<NonNull<T>> for VmPtr<T> {
    fn from(ptr: NonNull<T>) -> VmPtr<T> {
        VmPtr {
            ptr: unsafe { NonZeroUsize::new_unchecked(ptr.as_ptr().expose_provenance()) },
            _marker: PhantomData,
        }
    }
}

/// A custom "marker trait" used to tag types that are safe to share with
/// compiled wasm code.
///
/// The intention of this trait is to be used as a bound in a few core locations
/// in Wasmtime, such as `Instance::vmctx_plus_offset_mut`, and otherwise not
/// present very often. The purpose of this trait is to ensure that all types
/// stored to be shared with compiled code have a known layout and are
/// guaranteed to be "safe" to share with compiled wasm code.
///
/// This is an `unsafe` trait as it's generally not safe to share anything with
/// compiled code and it is used to invite extra scrutiny to manual `impl`s of
/// this trait. Types which implement this marker trait must satisfy at least
/// the following requirements.
///
/// * The ABI of `Self` must be well-known and defined. This means that the type
///   can interoperate with compiled code. For example `u8` is well defined as
///   is a `#[repr(C)]` structure. Types lacking `#[repr(C)]` or other types
///   like Rust tuples do not satisfy this requirement.
///
/// * For types which contain pointers the pointer's provenance is guaranteed to
///   have been exposed when the type is constructed. This is satisfied where
///   the only pointer that implements this trait is `VmPtr<T>` above which is
///   explicitly used to indicate exposed provenance. Notably `*mut T` and
///   `NonNull<T>` do not implement this trait, and intentionally so.
///
/// * For composite structures (e.g. `struct`s in Rust) all member fields must
///   satisfy the above criteria. All fields must have defined layouts and
///   pointers must be `VmPtr<T>`.
///
/// * Newtype or wrapper types around primitives that are used by value must be
///   `#[repr(transparent)]` to ensure they aren't considered aggregates by the
///   compile to match the ABI of the primitive type.
///
/// In this module a number of impls are provided for the primitives of Rust,
/// for example integers. Additionally some basic pointer-related impls are
/// provided for `VmPtr<T>` above. More impls can be found in `vmcontext.rs`
/// where there are manual impls for all `VM*` data structures which are shared
/// with compiled code.
pub unsafe trait VmSafe {}

// Implementations for primitive types. Note that atomics are included here as
// some atomic values are shared with compiled code. Rust's atomics are
// guaranteed to have the same memory representation as their primitive.
unsafe impl VmSafe for u8 {}
unsafe impl VmSafe for u16 {}
unsafe impl VmSafe for u32 {}
unsafe impl VmSafe for u64 {}
unsafe impl VmSafe for u128 {}
unsafe impl VmSafe for usize {}
unsafe impl VmSafe for i8 {}
unsafe impl VmSafe for i16 {}
unsafe impl VmSafe for i32 {}
unsafe impl VmSafe for i64 {}
unsafe impl VmSafe for i128 {}
unsafe impl VmSafe for isize {}
unsafe impl VmSafe for AtomicUsize {}
#[cfg(target_has_atomic = "64")]
unsafe impl VmSafe for core::sync::atomic::AtomicU64 {}

unsafe impl VmSafe for VMSharedTypeIndex {}

// Core implementations for `VmPtr`. Notably `VMPtr<T>` requires that `T` also
// implements `VmSafe`. Additionally an `Option` wrapper is allowed as that's
// just a nullable pointer.
unsafe impl<T: VmSafe> VmSafe for VmPtr<T> {}
unsafe impl<T: VmSafe> VmSafe for Option<VmPtr<T>> {}
