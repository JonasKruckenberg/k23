use crate::arch;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

#[thread_local]
static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // if we panic in the logger, this will prevent us from spinning into an infinite panic loop
    if !PANICKING.swap(true, Ordering::AcqRel) {
        // let _ = riscv::semihosting::HostStream::new_stdout()
        //     .write_fmt(format_args!("KERNEL PANIC {info}"));

        log::error!("KERNEL PANIC {info}");
    }

    loop {
        arch::halt()
    }
}
