// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::Trap;
use alloc::boxed::Box;
use core::num::NonZeroU32;
use core::panic::AssertUnwindSafe;
use core::ptr::NonNull;

pub enum TrapReason {
    /// A user-raised trap through `raise_user_trap`.
    User(anyhow::Error),

    /// A trap raised from Cranelift-generated code.
    Jit {
        /// The program counter where this trap originated.
        ///
        /// This is later used with side tables from compilation to translate
        /// the trapping address to a trap code.
        pc: usize,

        /// If the trap was a memory-related trap such as SIGSEGV then this
        /// field will contain the address of the inaccessible data.
        ///
        /// Note that wasm loads/stores are not guaranteed to fill in this
        /// information. Dynamically-bounds-checked memories, for example, will
        /// not access an invalid address but may instead load from NULL or may
        /// explicitly jump to a `ud2` instruction. This is only available for
        /// fault-based traps which are one of the main ways, but not the only
        /// way, to run wasm.
        faulting_addr: Option<usize>,

        /// The trap code associated with this trap.
        trap: Trap,
    },

    /// A trap raised from a wasm builtin
    Wasm(Trap),
}

impl From<anyhow::Error> for TrapReason {
    fn from(err: anyhow::Error) -> Self {
        TrapReason::User(err)
    }
}

impl From<Trap> for TrapReason {
    fn from(code: Trap) -> Self {
        TrapReason::Wasm(code)
    }
}

pub enum UnwindReason {
    Panic(Box<dyn core::any::Any + Send>),
    Trap(TrapReason),
}

/// A trait used in conjunction with `catch_unwind_and_record_trap` to convert a
/// Rust-based type to a specific ABI while handling traps/unwinds.
pub trait HostResult {
    /// The type of the value that's returned to Cranelift-compiled code. Needs
    /// to be ABI-safe to pass through an `extern "C"` return value.
    type Abi: Copy;
    /// This type is implemented for return values from host function calls and
    /// builtins. The `Abi` value of this trait represents either a successful
    /// execution with some payload state or that a failed execution happened.
    /// Cranelift-compiled code is expected to test for this failure sentinel
    /// and process it accordingly.
    fn maybe_catch_unwind(f: impl FnOnce() -> Self) -> (Self::Abi, Option<UnwindReason>);
}

pub fn catch_unwind_and_record_trap<R>(f: impl FnOnce() -> R) -> R::Abi
where
    R: HostResult,
{
    let (ret, _unwind) = R::maybe_catch_unwind(f);
    ret
}

// Base case implementations that do not catch unwinds. These are for libcalls
// that neither trap nor execute user code. The raw value is the ABI itself.
//
// Panics in these libcalls will result in a process abort as unwinding is not
// allowed via Rust through `extern "C"` function boundaries.
macro_rules! host_result_no_catch {
    ($($t:ty,)*) => {
        $(
            impl HostResult for $t {
                type Abi = $t;
                fn maybe_catch_unwind(f: impl FnOnce() -> $t) -> ($t, Option<UnwindReason>) {
                    (f(), None)
                }
            }
        )*
    }
}

host_result_no_catch! {
    (),
    bool,
    u32,
    *mut u8,
    u64,
}

impl HostResult for NonNull<u8> {
    type Abi = *mut u8;
    fn maybe_catch_unwind(f: impl FnOnce() -> Self) -> (*mut u8, Option<UnwindReason>) {
        (f().as_ptr(), None)
    }
}

impl<T, E> HostResult for Result<T, E>
where
    T: HostResultHasUnwindSentinel,
    E: Into<TrapReason>,
{
    type Abi = T::Abi;

    fn maybe_catch_unwind(f: impl FnOnce() -> Self) -> (Self::Abi, Option<UnwindReason>) {
        let f = move || match f() {
            Ok(ret) => (ret.into_abi(), None),
            Err(reason) => (T::SENTINEL, Some(UnwindReason::Trap(reason.into()))),
        };

        crate::panic::catch_unwind(AssertUnwindSafe(f))
            .unwrap_or_else(|payload| (T::SENTINEL, Some(UnwindReason::Panic(payload))))
    }
}

/// Trait used in conjunction with `HostResult for Result<T, E>` where this is
/// the trait bound on `T`.
///
/// This is for values in the "ok" position of a `Result` return value. Each
/// value can have a separate ABI from itself (e.g. `type Abi`) and must be
/// convertible to the ABI. Additionally all implementations of this trait have
/// a "sentinel value" which indicates that an unwind happened. This means that
/// no valid instance of `Self` should generate the `SENTINEL` via the
/// `into_abi` function.
pub unsafe trait HostResultHasUnwindSentinel {
    /// The Cranelift-understood ABI of this value (should not be `Self`).
    type Abi: Copy;

    /// A value that indicates that an unwind should happen and is tested for in
    /// Cranelift-generated code.
    const SENTINEL: Self::Abi;

    /// Converts this value into the ABI representation. Should never returned
    /// the `SENTINEL` value.
    fn into_abi(self) -> Self::Abi;
}

/// No return value from the host is represented as a `bool` in the ABI. Here
/// `true` means that execution succeeded while `false` is the sentinel used to
/// indicate an unwind.
unsafe impl HostResultHasUnwindSentinel for () {
    type Abi = bool;
    const SENTINEL: bool = false;
    fn into_abi(self) -> bool {
        true
    }
}

unsafe impl HostResultHasUnwindSentinel for NonZeroU32 {
    type Abi = u32;
    const SENTINEL: Self::Abi = 0;
    fn into_abi(self) -> Self::Abi {
        self.get()
    }
}

/// A 32-bit return value can be inflated to a 64-bit return value in the ABI.
/// In this manner a successful result is a zero-extended 32-bit value and the
/// failure sentinel is `u64::MAX` or -1 as a signed integer.
unsafe impl HostResultHasUnwindSentinel for u32 {
    type Abi = u64;
    const SENTINEL: u64 = u64::MAX;
    fn into_abi(self) -> u64 {
        self.into()
    }
}

/// If there is not actual successful result (e.g. an empty enum) then the ABI
/// can be `()`, or nothing, because there's no successful result and it's
/// always a failure.
unsafe impl HostResultHasUnwindSentinel for core::convert::Infallible {
    type Abi = ();
    const SENTINEL: () = ();
    fn into_abi(self) {
        match self {}
    }
}
