use cfg_if::cfg_if;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
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
    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}
