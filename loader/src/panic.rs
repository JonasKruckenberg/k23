use crate::arch;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("LOADER PANIC {}", info);

    arch::halt()
}
