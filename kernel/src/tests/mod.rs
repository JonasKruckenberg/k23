// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod args;
mod printer;

use crate::tests::args::Arguments;
use crate::tests::printer::Printer;
use crate::{arch, device_tree};
use alloc::boxed::Box;
use core::any::Any;
use core::fmt::Write;
use core::ptr::addr_of;
use core::{hint, slice};
use ktest::{Test, TestInfo};

/// The outcome of performing a single test.
pub enum Outcome {
    /// The test passed.
    Passed,
    /// The test failed.
    Failed(Box<dyn Any + Send + 'static>),
    /// The test was ignored.
    Ignored,
}

/// Conclusion of running the whole test suite
pub struct Conclusion {
    /// Number of tests and benchmarks that were filtered out (either by the
    /// filter-in pattern or by `--skip` arguments).
    pub num_filtered_out: u64,

    /// Number of passed tests.
    pub num_passed: u64,

    /// Number of failed tests and benchmarks.
    pub num_failed: u64,

    /// Number of ignored tests and benchmarks.
    pub num_ignored: u64,

    /// Number of benchmarks that successfully ran.
    pub num_measured: u64,
}

impl Conclusion {
    /// Returns an exit code that can be returned from `main` to signal
    /// success/failure to the calling process.
    pub fn exit_code(&self) -> i32 {
        if self.has_failed() { 101 } else { 0 }
    }

    /// Returns whether there have been any failures.
    pub fn has_failed(&self) -> bool {
        self.num_failed > 0
    }

    /// Exits the application with an appropriate error code (0 if all tests
    /// have passed, 101 if there have been failures). **This will not run any destructors.**
    /// Consider using [`Self::exit_code`] instead for a proper program cleanup.
    pub fn exit(&self) -> ! {
        arch::exit(0);
    }

    /// Exits the application with error code 101 if there were any failures.
    /// Otherwise, returns normally. **This will not run any destructors.**
    /// Consider using [`Self::exit_code`] instead for a proper program cleanup.
    pub fn exit_if_failed(&self) {
        if self.has_failed() {
            arch::exit(101)
        }
    }

    fn empty() -> Self {
        Self {
            num_filtered_out: 0,
            num_passed: 0,
            num_failed: 0,
            num_ignored: 0,
            num_measured: 0,
        }
    }
}

pub fn run_tests(write: &mut dyn Write, boot_info: &'static loader_api::BootInfo) -> Conclusion {
    let chosen = device_tree::device_tree().find_by_path("/chosen").unwrap();
    let args = if let Some(prop) = chosen.property("bootargs") {
        let str = prop.as_str().unwrap();
        Arguments::from_str(str)
    } else {
        Arguments::default()
    };

    let tests = all_tests();

    let mut conclusion = Conclusion::empty();

    // Create printer which is used for all output.
    let mut printer = Printer::new(write, args.format);

    // If `--list` is specified, just print the list and return.
    if args.list {
        printer.print_list(tests, args.ignored);
        return Conclusion::empty();
    }

    // Print number of tests
    printer.print_title(tests.len() as u64);

    let mut handle_outcome = |outcome: Outcome, test: &TestInfo, printer: &mut Printer| {
        printer.print_single_outcome(test, &outcome);

        // Handle outcome
        match outcome {
            Outcome::Passed => conclusion.num_passed += 1,
            Outcome::Failed(_reason) => {
                conclusion.num_failed += 1;
            }
            Outcome::Ignored => conclusion.num_ignored += 1,
        }
    };

    for test in tests {
        // Print `test foo    ...`, run the test, then print the outcome in
        // the same line.
        printer.print_test(&test.info);
        let outcome = if args.is_ignored(test) {
            Outcome::Ignored
        } else {
            match crate::panic::catch_unwind(|| (test.run)(boot_info)) {
                Ok(_) => Outcome::Passed,
                Err(err) => Outcome::Failed(err),
            }
        };
        handle_outcome(outcome, &test.info, &mut printer);
    }

    printer.print_summary(&conclusion);

    conclusion
}

pub fn all_tests() -> &'static [Test] {
    #[used(linker)]
    #[unsafe(link_section = "k23_tests")]
    static mut LINKME_PLEASE: [Test; 0] = [];

    unsafe extern "C" {
        #[allow(improper_ctypes)]
        static __start_k23_tests: Test;
        #[allow(improper_ctypes)]
        static __stop_k23_tests: Test;
    }

    let start = addr_of!(__start_k23_tests);
    let stop = addr_of!(__stop_k23_tests);

    let stride = size_of::<Test>();
    let byte_offset = stop as usize - start as usize;
    let len = match byte_offset.checked_div(stride) {
        Some(len) => len,
        // The #[distributed_slice] call checks `size_of::<T>() > 0` before
        // using the unsafe `private_new`.
        None => unsafe { hint::unreachable_unchecked() },
    };

    unsafe { slice::from_raw_parts(start, len) }
}
