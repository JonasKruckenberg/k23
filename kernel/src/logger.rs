use crate::boot_info::BootInfo;
use core::fmt::Write;
use core::mem::MaybeUninit;
use log::{Metadata, Record};
use spin::mutex::Mutex;
use uart_16550::SerialPort;
use vmm::VirtualAddress;
use crate::kconfig;

static LOGGER: Logger = Logger(Mutex::new(MaybeUninit::uninit()));

struct Logger(Mutex<MaybeUninit<SerialPort>>);

pub fn init(base: VirtualAddress, clock_freq: u32) {
    let serial_port = unsafe {
        SerialPort::new(
            base.as_raw(),
            clock_freq,
            38400,
        )
    };

    LOGGER.0.lock().write(serial_port);

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(kconfig::LOG_LEVEL.to_level_filter());
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut uart = self.0.lock();
            let uart = unsafe { uart.assume_init_mut() };

            // disable interrupts while we hold the uart lock
            // otherwise we might deadlock if we try to log from the trap handler
            // arch::without_interrupts(|| {
            let _ = writeln!(
                uart,
                "[{:<5} {}] {}",
                record.level(),
                record.module_path_static().unwrap_or_default(),
                record.args()
            );
            // });
        }
    }

    fn flush(&self) {}
}
