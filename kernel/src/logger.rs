// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::RefCell;
use core::fmt::Write;
use log::{LevelFilter, Metadata, Record};
use thread_local::thread_local;

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
    STATE.with_borrow_mut(|state| state.1 = hartid);
}

thread_local!(
    static STATE: RefCell<(riscv::hio::HostStream, usize)> =
        RefCell::new((riscv::hio::HostStream::new_stdout(), 0));
);

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let _ = STATE.try_with(|state| {
                let (stdout, hartid) = unsafe { &mut *state.as_ptr() };
                let _ = stdout.write_fmt(format_args!(
                    "[{:<5} HART {} {}] {}\n",
                    record.level(),
                    *hartid,
                    record.module_path_static().unwrap_or_default(),
                    record.args()
                ));
            });
        }
    }

    fn flush(&self) {}
}
