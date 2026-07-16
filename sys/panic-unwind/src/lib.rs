// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std] // this crate is fully incompatible with `std` due to clashing lang item definitions
#![cfg(target_os = "none")]
#![feature(panic_can_unwind)]
#![feature(thread_local)]

use core::cell::Cell;
use core::ptr;

use abort::abort;
use cpu_local::cpu_local;
use unwind::UnwindException;

// Single exception header shared by every in-flight panic.
// We need _some_ pointer to pass through the system (that is expected by the landing pad code)
// so we use this static so we can check against a known address AND the class field.
// This is safe since we only every read from this.
static PANIC_EXCEPTION: UnwindException = UnwindException::new();

cpu_local! {
    // In-flight panic count, and whether a handler is currently reporting
    // one (logging + backtrace).
    static PANIC_STATE: Cell<(usize, bool)> = Cell::new((0, false));
}

/// Whether the current CPU is unwinding because of a panic.
#[inline]
#[must_use]
pub fn panicking() -> bool {
    PANIC_STATE.get().0 > 0
}

fn increase() {
    let (count, reporting) = PANIC_STATE.get();
    // A panic raised while a handler is reporting can only have come from the
    // reporting path itself and would recurse back through here forever; abort
    // silently, since logging would recurse again.
    if reporting {
        abort();
    }
    PANIC_STATE.set((count + 1, false));
}

fn decrease() {
    let (count, _) = PANIC_STATE.get();
    PANIC_STATE.set((count - 1, false));
}

fn set_reporting(reporting: bool) {
    let (count, _) = PANIC_STATE.get();
    PANIC_STATE.set((count, reporting));
}

/// Invokes a closure, catching an unwinding panic if one occurs.
///
/// # Errors
///
/// Returns `Err(())` if the closure panicked.
pub fn catch_unwind<F, R>(f: F) -> Result<R, ()>
where
    F: FnOnce() -> R + core::panic::UnwindSafe,
{
    unwind::catch_unwind(f).map_err(|_| decrease())
}

/// Resume an unwind previously caught with [`catch_unwind`].
pub fn resume_unwind() -> ! {
    increase();
    unwind::with_context(|regs, pc| rust_panic(regs.clone(), pc))
}

/// Begin unwinding from an externally captured register context (such as a trap
/// handler).
///
/// # Safety
///
/// This walks the stack and runs `Drop` implementations starting at `pc` with
/// the given `regs`. Be VERY careful that they are actually correctly captured.
pub unsafe fn begin_unwind(regs: unwind::Registers, pc: usize) -> ! {
    increase();
    rust_panic(regs, pc)
}

#[panic_handler]
fn panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    increase();

    // A panic thrown from here (a `Display` impl, the backtrace walk) recurses
    // straight back into this handler; `increase` aborts on it while set.
    set_reporting(true);

    log::error!("CPU {info}");

    // FIXME 32 seems adequate for unoptimized builds where the callstack can get quite deep
    //  but (at least at the moment) is absolute overkill for optimized builds. Sadly there
    //  is no good way to do conditional compilation based on the opt-level.
    const MAX_BACKTRACE_FRAMES: usize = 32;

    match backtrace::__rust_end_short_backtrace(
        backtrace::Backtrace::<MAX_BACKTRACE_FRAMES>::capture,
    ) {
        Ok(bt) => {
            log::error!("{bt}");
            if bt.frames_omitted {
                log::warn!("Stack trace was larger than backtrace buffer, omitted some frames.");
            }
        }
        Err(err) => log::error!("backtrace unavailable: {err}"),
    }

    set_reporting(false);

    if !info.can_unwind() {
        // Panicking while running destructors or through a nounwind function
        // (e.g. `extern "C"`) cannot continue unwinding; abort immediately.
        log::error!("cpu caused non-unwinding panic. aborting.");
        abort();
    }

    unwind::with_context(|regs, pc| rust_panic(regs.clone(), pc))
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[unsafe(no_mangle)]
fn rust_panic(regs: unwind::Registers, pc: usize) -> ! {
    // NB: `PANIC_EXCEPTION` is actually treated as immutable, nothing ever writes through this
    // but the rustc `intrinsics::catch_unwind` require a mut ptr.
    let exception = ptr::from_ref(&PANIC_EXCEPTION).cast_mut();

    // Safety: `PANIC_EXCEPTION` is a static, it trivially outlives the unwind.
    let Err(err) = unsafe { unwind::begin_unwind_with(exception, regs, pc) };
    match err {
        unwind::Error::EndOfStack => {
            log::error!(
                "unwinding completed without finding a `catch_unwind` make sure there is at least a root level catch unwind wrapping the main function. aborting."
            );
            abort();
        }
        err => {
            log::error!("unwinding failed with error {err}. aborting.");
            abort()
        }
    }
}
