// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use log::{Level, LevelFilter, Metadata, Record};

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
            let color = match record.level() {
                Level::Trace => "\x1b[36m",
                Level::Debug => "\x1b[34m",
                Level::Info => "\x1b[32m",
                Level::Warn => "\x1b[33m",
                Level::Error => "\x1b[31;1m",
            };
            
            print(format_args!(
                "[{color}{:<5}\x1b[0m {}] {}\n",
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
