#![no_std]
#![allow(internal_features)]
#![feature(std_internals, core_intrinsics, thread_local, rustc_attrs)]

extern crate alloc;

mod panic_count;

use crate::panic_count::MustAbort;
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::panic::{PanicPayload, UnwindSafe};
use core::{fmt, mem};

pub use panic_common::PanicHookInfo;

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
    unwind2::catch_unwind(f).inspect_err(|_| {
        panic_count::decrease() // decrease the panic count, since we caught it
    })
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
    backtrace::__rust_end_short_backtrace(|| {
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
    })
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
fn rust_panic(payload: &mut dyn PanicPayload) -> ! {
    match unwind2::begin_panic(unsafe { Box::from_raw(payload.take_box()) }) {
        Ok(_) => panic_common::exit(0),
        Err(_) => panic_common::abort(),
    }
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

pub fn set_hook(hook: Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send>) {
    assert!(
        !panicking(),
        "cannot set a panic hook from a panicking thread"
    );
    unsafe { panic_common::hook::set_hook(hook) }
}

pub fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
    assert!(
        !panicking(),
        "cannot set a panic hook from a panicking thread"
    );
    unsafe { panic_common::hook::take_hook() }
}

pub fn update_hook<F>(hook_fn: F)
where
    F: Fn(&(dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static), &PanicHookInfo<'_>)
        + Sync
        + Send
        + 'static,
{
    assert!(
        !panicking(),
        "cannot set a panic hook from a panicking thread"
    );
    unsafe { panic_common::hook::update_hook(hook_fn) }
}
