use crate::kconfig;
use core::fmt;
use core::fmt::Write;
use log::{Metadata, Record};
use spin::Mutex;

pub fn init() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(kconfig::LOG_LEVEL.to_level_filter());
}

static LOGGER: Logger = Logger(Mutex::new(LoggerInner));

struct Logger(Mutex<LoggerInner>);

struct LoggerInner;

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
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "riscv64")] {
                let ptr = s.as_ptr();
                let _ = sbicall::dbcn::debug_console_write(s.len(), ptr as usize, 0);
            }
        }

        Ok(())
    }
}
