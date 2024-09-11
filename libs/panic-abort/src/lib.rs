#![no_std]
#![allow(internal_features)]
#![feature(std_internals)]

extern crate alloc;

use alloc::boxed::Box;
use core::panic::PanicPayload;
use panic_common::PanicHookInfo;

/// Entry point for panics from the `core` crate.
#[panic_handler]
pub fn begin_panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    panic_common::with_panic_info(info, |payload, location, can_unwind| {
        panic_common::hook::call(&PanicHookInfo::new(location, payload.get(), can_unwind));

        if !can_unwind {
            // If a thread panics while running destructors or tries to unwind
            // through a nounwind function (e.g. extern "C") then we cannot continue
            // unwinding and have to abort immediately.
            log::error!("hart caused non-unwinding panic. aborting.\n");
        }

        panic_common::abort();
    })
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
fn rust_panic(_: &mut dyn PanicPayload) -> ! {
    panic_common::abort();
}

pub fn set_hook(hook: Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send>) {
    // Safety: A panicking thread will not be able to call this function because it has already aborted.
    unsafe { panic_common::hook::set_hook(hook) }
}

pub fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
    // Safety: A panicking thread will not be able to call this function because it has already aborted.
    unsafe { panic_common::hook::take_hook() }
}

pub fn update_hook<F>(hook_fn: F)
where
    F: Fn(&(dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static), &PanicHookInfo<'_>)
        + Sync
        + Send
        + 'static,
{
    // Safety: A panicking thread will not be able to call this function because it has already aborted.
    unsafe { panic_common::hook::update_hook(hook_fn) }
}
