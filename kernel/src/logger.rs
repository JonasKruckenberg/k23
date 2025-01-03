use core::cell::RefCell;
use core::fmt::Write;
use log::{LevelFilter, Metadata, Record};
use thread_local::declare_thread_local;

/// Initializes the global logger with the semihosting logger.
///
/// # Panics
///
/// This function will panic if it is called more than once, or if another library has already initialized a global logger.
pub fn init(lvl: LevelFilter) {
    static LOGGER: Logger = Logger;

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(lvl);
}

pub fn init_hart(hartid: usize) {
    let hio = riscv::hio::HostStream::new_stdout();

    STDOUT.initialize_with((RefCell::new(hio), hartid), |_, _| {});
}

declare_thread_local!(
    static STDOUT: (RefCell<riscv::hio::HostStream>, usize);
);

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            STDOUT.with(|(stdout, hartid)| {
                let mut stdout = stdout.borrow_mut();
                let _ = stdout.write_fmt(format_args!(
                    "[{:<5} HART {} {}] {}\n",
                    record.level(),
                    hartid,
                    record.module_path_static().unwrap_or_default(),
                    record.args()
                ));
            });
        }
    }

    fn flush(&self) {}
}
