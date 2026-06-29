// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::Write;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use log::{Metadata, Record};
use uefi::proto::console::text::Output;

static LOGGER: Logger = Logger::new();

pub fn init() {
    uefi::system::with_stdout(|stdout| {
        LOGGER.set_output(stdout);
    });

    // Set the logger.
    log::set_logger(&LOGGER).unwrap(); // Can only fail if already initialized.

    // Set logger max level to level specified by log features
    log::set_max_level(log::STATIC_MAX_LEVEL);
}

struct Logger {
    output: AtomicPtr<Output>,
}

// Safety: the logger is not thread-safe, but the loader boots on one CPU only
unsafe impl Sync for Logger {}
// Safety: the logger is not thread-safe, but the loader boots on one CPU only
unsafe impl Send for Logger {}

impl Logger {
    const fn new() -> Self {
        Self {
            output: AtomicPtr::new(ptr::null_mut()),
        }
    }

    fn set_output(&self, output: *mut Output) {
        self.output.store(output, Ordering::Release);
    }

    #[must_use]
    fn output(&self) -> *mut Output {
        self.output.load(Ordering::Acquire)
    }
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        // The cached `Output` lives in boot-services memory; once boot services
        // are gone (post `exit_boot_services`) the pointer dangles, so writing
        // through it would be a use-after-free.
        if !crate::are_boot_services_active() {
            return;
        }

        // Safety: `new` and `set_output` ensure the pointer is either NUL or points to a valid, initialized output
        if let Some(output) = unsafe { self.output().as_mut() } {
            let _ = writeln!(
                output,
                "[{:<5}][{}] {}",
                record.level(),
                record.module_path_static().unwrap_or_default(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}
