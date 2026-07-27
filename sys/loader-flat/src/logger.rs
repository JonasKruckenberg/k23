// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
use core::fmt::Write;

use log::{Level, Metadata, Record};

pub fn init() {
    static LOGGER: Logger = Logger;

    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::STATIC_MAX_LEVEL);
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

            #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
            let _ = DebugConsole.write_fmt(format_args!(
                "[{color}{:<5}\x1b[0m {}] {}\n",
                record.level(),
                record.module_path_static().unwrap_or_default(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
struct DebugConsole;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
impl fmt::Write for DebugConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut bytes = s.as_bytes();

        // firmware is free accept fewer bytes that we're trying to write.
        // instead of discarding unwritten bytes we simply try again until the buffer is drained.
        while !bytes.is_empty() {
            let written =
                riscv::sbi::dbcn::debug_console_write(bytes.len(), bytes.as_ptr().addr(), 0)
                    .map_err(|_| fmt::Error)?;

            // if no bytes were written _at all_ we can assume the firmware DBCN is not accepting
            // any writes. Lets not keep trying.
            if written == 0 {
                return Err(core::fmt::Error);
            }

            bytes = &bytes[written..];
        }

        Ok(())
    }
}
