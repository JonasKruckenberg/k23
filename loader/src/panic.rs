// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use kabort::abort;

use crate::arch;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // disable interrupts as soon as we enter the panic subsystem
    // no need to bother with those now as we're about to shut down anyway
    arch::interrupt::disable();

    let loc = info.location().unwrap(); // The current implementation always returns Some
    let msg = info.message();

    log::error!("cpu panicked at {loc}:\n{msg}");

    rust_panic()
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[unsafe(no_mangle)]
fn rust_panic() -> ! {
    abort()
}
