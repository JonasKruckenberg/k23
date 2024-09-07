#![no_std]
#![allow(internal_features)]
#![feature(std_internals, core_intrinsics, thread_local, rustc_attrs)]
extern crate alloc;

use crate::panic_count::MustAbort;
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::panic::{PanicPayload, UnwindSafe};
use core::{fmt, intrinsics, mem, mem::ManuallyDrop, ptr::addr_of_mut};
use panic_common::PanicHookInfo;

mod panic_count;

/// Determines whether the current thread is unwinding because of panic.
#[inline]
pub fn panicking() -> bool {
    !panic_count::count_is_zero()
}

/// Invokes a closure, capturing the cause of an unwinding panic if one occurs.
///
/// # Errors
///
/// If the given closure panics, the panic cause will be returned in the Err variant.
pub fn catch_unwind<F, R>(f: F) -> Result<R, Box<dyn Any + Send + 'static>>
where
    F: FnOnce() -> R + UnwindSafe,
{
    union Data<F, R> {
        // when we start, this field holds the closure
        f: ManuallyDrop<F>,
        // when the closure completed successfully, this will hold the return
        r: ManuallyDrop<R>,
        // when the closure panicked this will hold the panic payload
        p: ManuallyDrop<Box<dyn Any + Send>>,
    }

    #[inline]
    fn do_call<F: FnOnce() -> R, R>(data: *mut u8) {
        // SAFETY: this is the responsibility of the caller, see above.
        unsafe {
            let data = data.cast::<Data<F, R>>();
            let data = &mut (*data);
            let f = ManuallyDrop::take(&mut data.f);
            data.r = ManuallyDrop::new(f());
        }
    }

    #[inline]
    #[rustc_nounwind] // `intrinsic::r#try` requires catch fn to be nounwind
    fn do_catch<F: FnOnce() -> R, R>(data: *mut u8, payload: *mut u8) {
        // SAFETY: this is the responsibility of the caller, see above.
        //
        // When `__rustc_panic_cleaner` is correctly implemented we can rely
        // on `obj` being the correct thing to pass to `data.p` (after wrapping
        // in `ManuallyDrop`).
        unsafe {
            let data = data.cast::<Data<F, R>>();
            let data = &mut (*data);
            let obj = cleanup(payload);
            data.p = ManuallyDrop::new(obj);
        }
    }

    #[cold]
    unsafe fn cleanup(payload: *mut u8) -> Box<dyn Any + Send + 'static> {
        // SAFETY: The whole unsafe block hinges on a correct implementation of
        // the panic handler `__rust_panic_cleanup`. As such we can only
        // assume it returns the correct thing for `Box::from_raw` to work
        // without undefined behavior.
        let obj = unsafe { unwind::panic_cleanup(payload) };
        panic_count::decrease();
        obj
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    let data_ptr = addr_of_mut!(data).cast::<u8>();

    unsafe {
        if intrinsics::catch_unwind(do_call::<F, R>, data_ptr, do_catch::<F, R>) == 0 {
            Ok(ManuallyDrop::into_inner(data.r))
        } else {
            Err(ManuallyDrop::into_inner(data.p))
        }
    }
}

/// Triggers a panic, bypassing the panic hook.
pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    struct RewrapBox(Box<dyn Any + Send>);

    unsafe impl PanicPayload for RewrapBox {
        fn take_box(&mut self) -> *mut (dyn Any + Send) {
            Box::into_raw(mem::replace(&mut self.0, Box::new(())))
        }

        fn get(&mut self) -> &(dyn Any + Send) {
            &*self.0
        }
    }

    impl fmt::Display for RewrapBox {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(payload_as_str(&self.0))
        }
    }

    panic_count::increase(false);
    rust_panic(&mut RewrapBox(payload))
}

/// Entry point for panics from the `core` crate.
#[panic_handler]
fn begin_panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    panic_common::with_panic_info(info, |payload, location, can_unwind| {
        if let Some(must_abort) = panic_count::increase(true) {
            match must_abort {
                MustAbort::PanicInHook => {
                    let msg = payload_as_str(payload.get());
                    log::error!(
                        "panicked at {location}:\n{msg}\nhart panicked while processing panic. aborting.\n"
                    );
                }
            }

            panic_common::abort();
        }

        panic_common::hook::call(&PanicHookInfo::new(location, payload.get(), can_unwind));
        panic_count::finished_panic_hook();

        if !can_unwind {
            // If a thread panics while running destructors or tries to unwind
            // through a nounwind function (e.g. extern "C") then we cannot continue
            // unwinding and have to abort immediately.
            log::error!("hart caused non-unwinding panic. aborting.\n");

            panic_common::abort();
        }

        rust_panic(payload)
    })
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
pub fn rust_panic(payload: &mut dyn PanicPayload) -> ! {
    let code = unwind::panic_begin(unsafe { Box::from_raw(payload.take_box()) });

    panic_common::exit(code);
}

fn payload_as_str(payload: &dyn Any) -> &str {
    if let Some(&s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "Box<dyn Any>"
    }
}
