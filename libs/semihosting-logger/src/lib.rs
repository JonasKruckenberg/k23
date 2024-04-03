#![no_std]
use log::{LevelFilter, Metadata, Record};

static LOGGER: Logger = Logger;

struct Logger;

pub fn init(filter: LevelFilter) {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(filter);
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    use riscv_semihosting::hio;
                    use core::fmt::Write;

                    if let Ok(mut stdio) = hio::hstdout() {
                        let _ = writeln!(
                            stdio,
                            "[{:<5} {}] {}",
                            record.level(),
                            record.module_path_static().unwrap_or_default(),
                            record.args()
                        );
                    }
                }
            }
        }
    }

    fn flush(&self) {}
}
