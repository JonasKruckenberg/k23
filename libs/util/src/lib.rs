#![no_std]

mod cache_padded;
mod checked_maybe_uninit;

pub use cache_padded::CachePadded;
pub use checked_maybe_uninit::{CheckedMaybeUninit, MaybeUninitExt};
use core::ptr::NonNull;

/// Helper to construct a `NonNull<T>` from a raw pointer to `T`, with null
/// checks elided in release mode.
#[cfg(debug_assertions)]
#[track_caller]
#[inline(always)]
pub unsafe fn non_null<T>(ptr: *mut T) -> NonNull<T> {
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
pub unsafe fn non_null<T>(ptr: *mut T) -> NonNull<T> {
    // Safety: ensured by caller
    unsafe { NonNull::new_unchecked(ptr) }
}

/// Wraps a `const fn` stripping the "constness" when compiled under loom.
///
/// `loom` works by tracking additional state alongside each type. This has the annoying limitation that
/// many methods that are `const` in `core` cannot be `const` in `loom` because of this additional tracking.
///
/// As you can imagine this makes writing `const` functions that use `loom` types difficult.
///
/// # Example
///
/// ```rust
/// # use util::loom_const_fn;
///
/// struct Something { str: &'static str }
///
/// impl Something {
///     // `Something::new` will be const in regular use and non-const when running in loom
///     loom_const_fn! {
///         pub fn new() -> Self {
///             Self { str: "Hello World" }
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! loom_const_fn {
    (
        $(#[$meta:meta])*
        $vis:vis unsafe const fn $name:ident($($arg:ident: $T:ty),*) -> $Ret:ty $body:block
    ) => {
        $(#[$meta])*
        #[cfg(not(loom))]
        $vis const unsafe fn $name($($arg: $T),*) -> $Ret $body

        $(#[$meta])*
        #[cfg(loom)]
        $vis unsafe fn $name($($arg: $T),*) -> $Ret $body
    };
    (
        $(#[$meta:meta])*
        $vis:vis const fn $name:ident($($arg:ident: $T:ty),*) -> $Ret:ty $body:block
    ) => {
        $(#[$meta])*
        #[cfg(not(loom))]
        $vis const fn $name($($arg: $T),*) -> $Ret $body

        $(#[$meta])*
        #[cfg(loom)]
        $vis fn $name($($arg: $T),*) -> $Ret $body
    }
}
