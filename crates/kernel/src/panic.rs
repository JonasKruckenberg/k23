// use crate::{arch, backtrace};
use crate::backtrace;
use core::arch::asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("KERNEL PANIC {}", info);

    // if we panic in the backtrace, prevent us from spinning into an infinite panic loop
    if !PANICKING.swap(true, Ordering::AcqRel) {
        log::error!("un-symbolized stack trace:");
        let mut count = 0;
        backtrace::trace(|frame| {
            count += 1;
            log::debug!("{:<2}- {:#x?}", count, frame.symbol_address());
        });
    }

    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
