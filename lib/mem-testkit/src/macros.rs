// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

/// Emit a copy of `$body` for each architecture in the list, aliasing `$arch` to
/// that architecture's type inside a module named after it.
///
/// The architectures are listed explicitly at the call site rather than baked into
/// the macro, so the matrix — including the `#[cfg(not(miri))]` gates that drop the
/// extra paging modes under Miri — is visible while reading the test. (The page-walk
/// logic is identical across paging modes; running all three only triples Miri's
/// interpreter time without adding coverage.)
///
/// ```ignore
/// for_arch!(A in [Riscv64Sv39, #[cfg(not(miri))] Riscv64Sv48] {
///     #[test]
///     fn it_works() { /* `A` is the concrete arch type here */ }
/// });
/// ```
#[macro_export]
macro_rules! for_arch {
    ($arch:ident in [ $( $(#[$meta:meta])* $archty:ident ),+ $(,)? ] $body:tt) => {
        $(
            $(#[$meta])*
            #[expect(non_snake_case, reason = "test module named after the arch it instantiates")]
            mod $archty {
                use super::*;
                type $arch = mem_core::arch::riscv64::$archty;

                // The body is re-emitted verbatim per arch; capturing it as one `tt`
                // and unwrapping it here avoids `macro_rules!` zipping the body items
                // against the (independent) arch list.
                $crate::for_arch!(@items $body);
            }
        )+
    };
    (@items { $($body:item)* }) => { $($body)* };
}

/// Like [`for_arch!`], but for generic test functions: each `fn name<A: Arch>()`
/// is instantiated once per architecture in the list. The arch list (and its Miri
/// `#[cfg]` gates) is spelled out at the call site for the same reason — see
/// [`for_arch!`].
///
/// The type parameter may carry additional bounds beyond the leading one (e.g.
/// `fn map<A: Arch + MapsAt<Size4KiB>>()` for tests that map at a fixed page size);
/// the leading bound is a plain trait name and each further `+ Bound` a path. A bare
/// trait-name leading bound (rather than an arbitrary path) is what lets the macro
/// find the closing `>` unambiguously — a `tt`-greedy bound list cannot.
///
/// ```ignore
/// archtest!([Riscv64Sv39, #[cfg(not(miri))] Riscv64Sv48] {
///     #[test]
///     fn it_works<A: Arch>() { /* instantiated once per arch */ }
/// });
/// ```
#[macro_export]
macro_rules! archtest {
    ([ $( $(#[$meta:meta])* $archty:ident ),+ $(,)? ] $body:tt) => {
        $(
            $(#[$meta])*
            #[expect(non_snake_case, reason = "test module named after the arch it instantiates")]
            mod $archty {
                use super::*;

                // See [`for_arch!`]: the body is captured as one `tt` and
                // unwrapped here so it isn't zipped against the arch list.
                $crate::archtest!(@fns $archty $body);
            }
        )+
    };
    (@fns $archty:ident {
        $( $(#[$tmeta:meta])* fn $test_name:ident<$ge:ident: $bound:ident $(+ $extra:path)*>() $body:block )*
    }) => {
        $(
            $(#[$tmeta])*
            fn $test_name() {
                fn $test_name<$ge: $bound $(+ $extra)*>() $body
                $test_name::<mem_core::arch::riscv64::$archty>()
            }
        )*
    };
}
