use crate::arch;
use crate::arch::MINFO;
use core::panic::PanicInfo;
use qemu_exit::QEMUExit;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("LOADER PANIC {}", info);

    if let Some(minfo) = MINFO.get() {
        if let Some(exit_handle) = &minfo.qemu_exit_handle {
            exit_handle.exit_failure()
        }
    }

    // In case there is no QEMU exit handle we just wait forever here
    arch::halt()
}
