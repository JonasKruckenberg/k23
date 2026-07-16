// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ffi::c_int;

use abort::abort;

/// The header the unwinder threads through a landing pad. Its layout is fixed by
/// the ABI the compiler-generated personality and landing-pad code expect: the
/// pointer arrives in `UNWIND_DATA_REG` and round-trips through the cleanup pad
/// back to `_Unwind_Resume` unchanged.
///
/// What the pointer refers to is opaque to this crate — callers own it and
/// recover whatever sits at it on the catch side.
#[repr(C, align(16))]
pub struct UnwindException {
    exception_class: u64,
    cleanup: extern "C" fn(c_int, *mut UnwindException),
    _unused: [u64; 2],
}

impl UnwindException {
    /// Rust's exception class identifier. Personality routines use it to
    /// determine whether an exception was thrown by their own runtime.
    #[expect(clippy::unusual_byte_groupings, reason = "its a bit pattern")]
    pub(crate) const CLASS: u64 = 0x4d4f5a_00_52555354; // M O Z \0  R U S T -- vendor, language

    /// Creates a header tagged with Rust's exception class.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            exception_class: Self::CLASS,
            cleanup: exception_cleanup,
            _unused: [0; 2],
        }
    }

    /// Whether this header carries Rust's exception class.
    pub(crate) fn is_rust(&self) -> bool {
        self.exception_class == Self::CLASS
    }
}

impl Default for UnwindException {
    fn default() -> Self {
        Self::new()
    }
}

/// `_Unwind_DeleteException` invokes this through the header when a foreign
/// runtime tears down an in-flight Rust exception. In-kernel there is no
/// foreign runtime, so reaching it means an exception escaped where it must
/// not: abort rather than let it be silently dropped.
extern "C" fn exception_cleanup(_unwind_code: c_int, _exception: *mut UnwindException) {
    log::error!("Rust exceptions must not be deleted by a foreign runtime");
    abort();
}
