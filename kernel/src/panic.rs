use crate::arch;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // if we panic in the logger, this will prevent us from spinning into an infinite panic loop
    if !PANICKING.swap(true, Ordering::AcqRel) {
        log::error!("KERNEL PANIC {info}");
    }

    loop {
        arch::halt()
    }
}
