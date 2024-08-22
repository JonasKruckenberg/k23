//! A logging implementation for [`log`] that prints to the host's stdout via the `semihosting` API.
#![no_std]
#![cfg_attr(feature = "hartid", feature(thread_local))]

#[cfg(feature = "hartid")]
pub mod hartid;

use log::{LevelFilter, Metadata, Record};

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

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            cfg_if::cfg_if! {
                if #[cfg(feature = "hartid")] {
                    print(format_args!(
                        "[{:<5} HART {} {}] {}\n",
                        record.level(),
                        hartid::get(),
                        record.module_path_static().unwrap_or_default(),
                        record.args()
                    ));
                } else {
                    print(format_args!(
                        "[{:<5} {}] {}\n",
                        record.level(),
                        record.module_path_static().unwrap_or_default(),
                        record.args()
                    ));
                }
            }
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
