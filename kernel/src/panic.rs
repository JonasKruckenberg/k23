use crate::arch;
use crate::boot_info::BOOT_INFO;
use core::arch::asm;
use core::panic::PanicInfo;
use qemu_exit::QEMUExit;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("KERNEL PANIC {}", info);

    unsafe {
        let ra: usize;
        asm!("mv {}, ra", out(reg) ra);
        log::info!("return address was {ra:#x?}");
    }

    // if let Some(minfo) = BOOT_INFO.get() {
    //     if let Some(exit_handle) = &minfo.qemu_exit_handle {
    //         exit_handle.exit_failure()
    //     }
    // }

    arch::halt()
}
