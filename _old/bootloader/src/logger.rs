use crate::machine_info::MachineInfo;
use crate::KCONFIG;
use core::fmt::Write;
use core::mem::MaybeUninit;
use log::{Metadata, Record};
use spin::mutex::Mutex;
use uart_16550::SerialPort;

static LOGGER: Logger = Logger(Mutex::new(MaybeUninit::uninit()));

struct Logger(Mutex<MaybeUninit<SerialPort>>);

pub fn init(machine_info: &MachineInfo) {
    let serial_port = unsafe {
        SerialPort::new(
            machine_info.serial.mmio_regs.start.as_raw(),
            machine_info.serial.clock_frequency,
            KCONFIG.uart_baud_rate,
        )
    };

    LOGGER.0.lock().write(serial_port);

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(KCONFIG.log_level.to_level_filter());
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
