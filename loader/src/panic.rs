/// Entry point for panics from the `core` crate.
#[panic_handler]
pub fn begin_panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    let loc = info.location().unwrap(); // The current implementation always returns Some
    let msg = info.message();
    log::error!("hart panicked at {loc}:\n{msg}");

    if !info.can_unwind() {
        // If a thread panics while running destructors or tries to unwind
        // through a nounwind function (e.g. extern "C") then we cannot continue
        // unwinding and have to abort immediately.
        log::error!("hart caused non-unwinding panic. aborting.\n");
    }

    rust_panic()
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
fn rust_panic() -> ! {
    riscv::abort();
}
