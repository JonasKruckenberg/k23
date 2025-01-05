use crate::arch;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // disable interrupts as soon as we enter the panic subsystem
    // no need to bother with those now as we're about to shut down anyway
    arch::interrupt::disable();

    let loc = info.location().unwrap(); // The current implementation always returns Some
    let msg = info.message();

    log::error!("hart panicked at {loc}:\n{msg}");

    rust_panic()
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
fn rust_panic() -> ! {
    arch::abort()
}
