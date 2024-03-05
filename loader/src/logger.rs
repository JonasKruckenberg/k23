//! `log` compatible frontend to emit messages through the SBI DBCN interface.
//!
//! Since the loader assumes the presence of a previous loader stage that also provides a
//! Risc-V *Supervisor Binary Interface* Environment, we can save ourselves the hassle and complexity of
//! having to parse the DTB and initialize a UART serial port by using the provided DBCN logging facilities.

use core::fmt;
use core::fmt::Write;
use log::{Metadata, Record};
use spin::Mutex;

static LOGGER: Logger = Logger(Mutex::new(LoggerInner));
struct Logger(Mutex<LoggerInner>);
pub struct LoggerInner;

pub fn init() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut inner = self.0.lock();
            let _ = writeln!(
                inner,
                "[{:<5} {}] {}",
                record.level(),
                record.module_path_static().unwrap_or_default(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

impl Write for LoggerInner {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let ptr = s.as_ptr();
        sbicall::dbcn::debug_console_write(s.len(), ptr as usize, 0).unwrap();
        Ok(())
    }
}
