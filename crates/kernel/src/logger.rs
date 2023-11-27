use crate::board_info::Serial;
use crate::sync::Mutex;
use log::{Metadata, Record};
use uart_16550::SerialPort;

static LOGGER: Logger = Logger::empty();

struct Logger(Mutex<Option<SerialPort>>);

pub fn init(serial_info: &Serial, baud_rate: u32) {
    let uart = unsafe {
        SerialPort::new(
            serial_info.mmio_regs.start.as_raw(),
            serial_info.clock_frequency,
            baud_rate,
        )
    };
    LOGGER.0.lock().replace(uart);

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
}

impl Logger {
    pub const fn empty() -> Self {
        Self(Mutex::new(None))
    }
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            use core::fmt::Write;

            // disable interrupts while we hold the uart lock
            // otherwise we might deadlock if we try to log from the trap handler
            // TODO maybe replace this with a reentrant mutex
            crate::interrupt::without(|| {
                let mut uart = self.0.lock();
                // don't panic if we accidentally log before the logger is initialized
                // logs are not that important anyway
                let Some(uart) = uart.as_mut() else { return };

                let _ = writeln!(
                    uart,
                    "[{:<5} {}] {}",
                    record.level(),
                    record.module_path_static().unwrap_or_default(),
                    record.args()
                );
            })
        }
    }

    fn flush(&self) {}
}
