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
    _uwe: crate::unwinding::UnwindException,
    canary: *const u8,
    cause: Box<dyn Any + Send>,
}

pub fn panic_begin(data: Box<dyn Any + Send>) -> i32 {
    extern "C" fn exception_cleanup(
        _unwind_code: crate::unwinding::UnwindReasonCode,
        exception: *mut crate::unwinding::UnwindException,
    ) {
        unsafe {
            let _: Box<Exception> = Box::from_raw(exception as *mut Exception);
            heprintln!("Rust panics must be rethrown");
            arch::abort_internal(1);
        }
    }

    let exception = Box::into_raw(Box::new(Exception {
        _uwe: crate::unwinding::UnwindException::new(
            rust_exception_class(),
            Some(exception_cleanup),
        ),
        canary: &CANARY,
        cause: data,
    })) as *mut crate::unwinding::UnwindException;

    unsafe { crate::unwinding::_Unwind_RaiseException(exception).0 }
}

pub unsafe fn panic_cleanup(ptr: *mut u8) -> Box<dyn Any + Send> {
    let exception = ptr as *mut UnwindException;
    if (*exception).exception_class != rust_exception_class() {
        crate::unwinding::_Unwind_DeleteException(exception);
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

    let exception = Box::from_raw(exception as *mut Exception);
    exception.cause
}

// Rust's exception class identifier.  This is used by personality routines to
// determine whether the exception was thrown by their own runtime.
fn rust_exception_class() -> u64 {
    // M O Z \0  R U S T -- vendor, language
    0x4d4f5a_00_52555354
}
