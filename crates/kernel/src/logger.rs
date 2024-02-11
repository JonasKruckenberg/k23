use crate::board_info::BoardInfo;
use crate::sync::{Mutex, Once};
use crate::{arch, Error};
use core::fmt::Write;
use kmem::VirtualAddress;
use log::{Metadata, Record};
use uart_16550::SerialPort;

const BAUD_RATE: u32 = 38400;

static LOGGER: Once<Logger> = Once::empty();

struct Logger {
    port: Mutex<SerialPort>,
    freq: u32,
}

/// Perform [*early initialization*](../../../ARCHITECTURE.md) of the logger.
pub fn init_early(board_info: &BoardInfo) -> crate::Result<()> {
    let port = unsafe {
        SerialPort::new(
            board_info.serial.mmio_regs.start.as_raw(),
            board_info.serial.clock_frequency,
            BAUD_RATE,
        )
    };

    LOGGER.get_or_init(move || Logger {
        port: Mutex::new(port),
        freq: board_info.serial.clock_frequency,
    });

    log::set_logger(&LOGGER).map_err(Error::InitLogger)?;
    log::set_max_level(log::LevelFilter::Trace);

    Ok(())
}

pub fn init_late(uart_base: VirtualAddress) {
    let logger = LOGGER.wait();

    *logger.port.lock() = unsafe { SerialPort::new(uart_base.as_raw(), logger.freq, BAUD_RATE) };
}

impl log::Log for Once<Logger> {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        arch::interrupt::without(|| {
            // don't deadlock if we accidentally log before the logger is initialized
            // logs are not that important anyway
            if let Some(logger) = self.get() {
                let mut uart = logger.port.lock();

                let _ = writeln!(
                    uart,
                    "[{:<5} {}] {}",
                    record.level(),
                    record.module_path_static().unwrap_or_default(),
                    record.args()
                );
            }
        })
    }

    fn flush(&self) {}
}
