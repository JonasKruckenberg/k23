use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("LOADER PANIC {}", info);

    loop {}
}
