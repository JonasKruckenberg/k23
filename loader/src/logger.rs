use log::{LevelFilter, Metadata, Record};

pub fn init(lvl: LevelFilter) {
    static LOGGER: Logger = Logger;

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(lvl);
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            print(format_args!(
                "[{:<5} {}] {}\n",
                record.level(),
                record.module_path_static().unwrap_or_default(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

fn print(args: core::fmt::Arguments) {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::hio::_print(args);
        } else {
            compile_error!("unsupported target architecture");
        }
    }
}
