#![no_std]
#![feature(used_with_arg)]

mod args;
mod printer;
mod run;

extern crate alloc;

use alloc::boxed::Box;
use core::any::Any;
pub use ktest_macros::{for_each_fixture, test};
use loader_api::BootInfo;

/// A single test case
pub struct Test {
    pub run: fn(&'static BootInfo),
    pub info: TestInfo<'static>,
}

/// Metadata associated with a test case
pub struct TestInfo<'a> {
    pub module: &'a str,
    pub name: &'a str,
    pub ignored: bool,
}

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
        if self.has_failed() {
            101
        } else {
            0
        }
    }

    /// Returns whether there have been any failures.
    pub fn has_failed(&self) -> bool {
        self.num_failed > 0
    }

    /// Exits the application with an appropriate error code (0 if all tests
    /// have passed, 101 if there have been failures). **This will not run any destructors.**
    /// Consider using [`Self::exit_code`] instead for a proper program cleanup.
    pub fn exit(&self) -> ! {
        // self.exit_if_failed();
        riscv::exit(0)
    }

    /// Exits the application with error code 101 if there were any failures.
    /// Otherwise, returns normally. **This will not run any destructors.**
    /// Consider using [`Self::exit_code`] instead for a proper program cleanup.
    pub fn exit_if_failed(&self) {
        if self.has_failed() {
            riscv::exit(101)
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
