#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("LOADER PANIC {info}");
    riscv::semihosting::exit(1);
}
