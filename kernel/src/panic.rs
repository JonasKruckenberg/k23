use crate::arch;
use crate::machine_info::MINFO;
use core::panic::PanicInfo;
use qemu_exit::QEMUExit;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("KERNEL PANIC {}", info);

    if let Some(minfo) = MINFO.get() {
        if let Some(exit_handle) = &minfo.qemu_exit_handle {
            exit_handle.exit_failure()
        }
    }

    arch::halt()
}
