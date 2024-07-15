use crate::arch;
use log::{LevelFilter, Metadata, Record};

pub fn init() {
    static LOGGER: Logger = Logger;

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(LevelFilter::Trace);
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            arch::HARTID.with(|hartid| {
                kstd::heprintln!(
                    "[{:<5} HART {} {}] {}",
                    record.level(),
                    hartid,
                    record.module_path_static().unwrap_or_default(),
                    record.args()
                );
            });
        }
    }

    fn flush(&self) {}
}
