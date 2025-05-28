// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
};
use cpu_local::cpu_local;

/// A reason for forcing an immediate abort on panic.
#[derive(Debug)]
pub enum MustAbort {
    // AlwaysAbort,
    PanicInHook,
}

// Panic count for the current thread and whether a panic hook is currently
// being executed.
cpu_local! {
    static LOCAL_PANIC_COUNT: Cell<(usize, bool)> = Cell::new((0, false));
}

static GLOBAL_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn increase(run_panic_hook: bool) -> Option<MustAbort> {
    let (count, in_panic_hook) = LOCAL_PANIC_COUNT.get();
    if in_panic_hook {
        return Some(MustAbort::PanicInHook);
    }
    LOCAL_PANIC_COUNT.set((count + 1, run_panic_hook));
    None
}

pub fn finished_panic_hook() {
    let (count, _) = LOCAL_PANIC_COUNT.get();
    LOCAL_PANIC_COUNT.set((count, false));
}

pub fn decrease() {
    GLOBAL_PANIC_COUNT.fetch_sub(1, Ordering::Relaxed);
    let (count, _) = LOCAL_PANIC_COUNT.get();
    LOCAL_PANIC_COUNT.set((count - 1, false));
}

// Disregards ALWAYS_ABORT_FLAG
#[must_use]
#[inline]
pub fn count_is_zero() -> bool {
    if GLOBAL_PANIC_COUNT.load(Ordering::Relaxed) == 0 {
        // Fast path: if `GLOBAL_PANIC_COUNT` is zero, all threads
        // (including the current one) will have `LOCAL_PANIC_COUNT`
        // equal to zero, so TLS access can be avoided.
        //
        // In terms of performance, a relaxed atomic load is similar to a normal
        // aligned memory read (e.g., a mov instruction in x86), but with some
        // compiler optimization restrictions. On the other hand, a TLS access
        // might require calling a non-inlinable function (such as `__tls_get_addr`
        // when using the GD TLS model).
        true
    } else {
        is_zero_slow_path()
    }
}

// Slow path is in a separate function to reduce the amount of code
// inlined from `count_is_zero`.
#[inline(never)]
#[cold]
fn is_zero_slow_path() -> bool {
    LOCAL_PANIC_COUNT.get().0 == 0
}
