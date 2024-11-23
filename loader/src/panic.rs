use cfg_if::cfg_if;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let location = info.location();

    log::error!("hart panicked at {location:?}:\n{}", info.message());

    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}