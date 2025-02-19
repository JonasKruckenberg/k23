// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::{arch, Error};
use alloc::boxed::Box;
use core::any::Any;
use core::ffi::c_int;
use core::ptr;

/// The C ABI UnwindReasonCode passed to the exception cleanup function when a foreign
/// (i.e. not originating from this crate) exception is caught. Since that exception might have
/// come from any libunwind compatible language and unwinder we have to play by the rules and
/// provide the correct A ABI compatible code.
const URC_FOREIGN_EXCEPTION_CAUGHT: c_int = 1;

/// This static's address is used to verify that exceptions we handle
/// are actually generated by us and not a different unwind implementation.
static CANARY: u8 = 0;

#[repr(C)]
pub struct Exception {
    _uwe: UnwindException,
    canary: *const u8,
    payload: Box<dyn Any + Send>,
}

/// The C ABI compliant part of the exception. This is used to differentiate between
/// libunwind exceptions and ours on a type-level and to make sure we have the right
/// structure in place so no other unwinders mess with our data.
#[repr(C, align(16))]
pub struct UnwindException {
    exception_class: u64,
    cleanup: extern "C" fn(c_int, *mut UnwindException),
    pub(crate) _unused: [u64; 2],
}

impl Exception {
    /// Rust's exception class identifier.  This is used by personality routines to
    /// determine whether the exception was thrown by their own runtime.
    #[expect(clippy::unusual_byte_groupings, reason = "its a bit pattern")]
    const CLASS: u64 = 0x4d4f5a_00_52555354; // M O Z \0  R U S T -- vendor, language

    /// Wraps the given payload in an exception.
    pub fn wrap(payload: Box<dyn Any + Send>) -> *mut Self {
        // This function is just a bad, unergonomic wrapper around the exceptions drop handler.
        // It is necessary, since other unwinding implementations *might* call this function (it
        // is part of the required C ABI) through the _Unwind_DeleteException function.
        //
        // We don't really care about that too much since we don't use it at all, but we still
        // provide it just to be safe from nasty, hard to debug crashes.
        extern "C" fn exception_cleanup(_unwind_code: c_int, exception: *mut UnwindException) {
            // Safety: Caller ensures `exception` is a valid exception
            unsafe {
                drop(Box::from_raw(exception.cast::<Exception>()));
                arch::abort("Rust panics must be rethrown");
            }
        }

        let exception = Box::new(Self {
            _uwe: UnwindException {
                exception_class: Self::CLASS,
                cleanup: exception_cleanup,
                _unused: [0; 2],
            },
            canary: &CANARY,
            payload,
        });

        Box::into_raw(exception)
    }

    /// Unwraps the exception and returns the stored payload.
    pub unsafe fn unwrap(exception: *mut Self) -> Result<Box<dyn Any + Send>, Error> {
        // First check whether this is a Rust exception
        let exception = exception.cast::<UnwindException>();
        // Safety: caller has to ensure `exception` is a valid pointer
        let _exception = unsafe { &*exception };
        if _exception.exception_class != Self::CLASS {
            (_exception.cleanup)(URC_FOREIGN_EXCEPTION_CAUGHT, exception);
            return Err(Error::ForeignException);
        }

        // Now let's check whether it has been created by *us* and not some other
        // rust unwinder. Make sure to only access the canary field, as the rest of the
        // structure is unknown.
        let exception = exception.cast::<Exception>();
        // Safety: caller has to ensure `exception` is a valid pointer
        let canary = unsafe { ptr::addr_of!((*exception).canary).read() };
        if !ptr::eq(canary, &CANARY) {
            return Err(Error::ForeignException);
        }

        // We can be certain it's our exception, so we can safely unwrap it.
        // Safety: we checked this exhaustively above
        let exception = unsafe { Box::from_raw(exception) };
        Ok(exception.payload)
    }
}
