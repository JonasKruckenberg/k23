use crate::arch;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("KERNEL PANIC {}", info);

    loop {
        arch::halt()
    }
}
