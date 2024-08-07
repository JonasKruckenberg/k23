//! DWARF based stack unwinding.
//!
//! # libunwind compatibility
//!
//! The functions mirror the [`libunwind`][libunwind] ABI and therefore this module is littered
//! with ugly externs, C ABI types, pointers and the like.
//! *In theory* there is no requirement for this ABI compatibility (aside from the `_Unwind_Resume` function)
//! and much of the baggage *could* be dropped from this module since we don't do any FFI in the kernel,
//! but stack unwinding is complicated and intricate enough that sticking with established patterns.
//!
//! Rewriting this to have a nice, sane API remains a TODO for later.
//!
//! # Acknowledgement
//!
//! Much of the unwinding code is taken from the great [`unwinding`][unwinding] crate, with some
//! inspiration from stdlib too, both licensed under MIT/Apache 2.0.
//!
//! [libunwind]: https://www.ucw.cz/~hubicka/papers/abi/node25.html#SECTION00923100000000000000
//! [unwinding]: https://github.com/nbdd0121/unwinding


mod eh_info;
mod frame;
mod personality;
mod unwinder;
mod utils;

use core::{any::Any, ptr};

use crate::{arch, heprintln};
use alloc::boxed::Box;
pub use unwinder::*;

static CANARY: u8 = 0;

#[repr(C)]
struct Exception {
    _uwe: UnwindException,
    canary: *const u8,
    cause: Box<dyn Any + Send>,
}

pub fn panic_begin(data: Box<dyn Any + Send>) -> i32 {
    extern "C" fn exception_cleanup(
        _unwind_code: UnwindReasonCode,
        exception: *mut UnwindException,
    ) {
        unsafe {
            let _: Box<Exception> = Box::from_raw(exception.cast::<Exception>());
            heprintln!("Rust panics must be rethrown");
            arch::abort_internal(1);
        }
    }

    let exception = Box::into_raw(Box::new(Exception {
        _uwe: UnwindException::new(rust_exception_class(), Some(exception_cleanup)),
        canary: &CANARY,
        cause: data,
    }))
    .cast::<UnwindException>();

    unsafe { _Unwind_RaiseException(exception).0 }
}

/// # Safety
///
/// The caller has to ensure the given `ptr` points to a valid and correctly aligned `Exception`
#[allow(clippy::cast_ptr_alignment)]
pub unsafe fn panic_cleanup(ptr: *mut u8) -> Box<dyn Any + Send> {
    let exception = ptr.cast::<UnwindException>();
    if (*exception).exception_class != rust_exception_class() {
        _Unwind_DeleteException(exception);
        heprintln!("Rust cannot catch foreign exceptions");
        arch::abort_internal(1);
    }

    let exception = exception.cast::<Exception>();
    // Just access the canary field, avoid accessing the entire `Exception` as
    // it can be a foreign Rust exception.
    let canary = ptr::addr_of!((*exception).canary).read();
    if !ptr::eq(canary, &CANARY) {
        heprintln!("Rust cannot catch foreign exceptions");
        arch::abort_internal(1);
    }

    let exception = Box::from_raw(exception);
    exception.cause
}

// Rust's exception class identifier.  This is used by personality routines to
// determine whether the exception was thrown by their own runtime.
#[allow(clippy::unusual_byte_groupings)]
fn rust_exception_class() -> u64 {
    // M O Z \0  R U S T -- vendor, language
    0x4d4f5a_00_52555354
}
