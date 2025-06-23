// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

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
///         pub const fn new() -> Self {
///             Self { str: "Hello World" }
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! loom_const_fn {
    (
        $(#[$meta:meta])*
        $vis:vis const unsafe fn $name:ident($($arg:ident: $T:ty),*) -> $Ret:ty $body:block
    ) => {
        $(#[$meta])*
        #[cfg(not(loom))]
        $vis const unsafe fn $name($($arg: $T),*) -> $Ret $body

        $(#[$meta])*
        #[cfg(loom)]
        #[inline]
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
        #[inline]
        $vis fn $name($($arg: $T),*) -> $Ret $body
    }
}
