#![cfg_attr(target_os = "none", no_std)]
#![feature(used_with_arg)]

#[doc(hidden)]
pub mod __private;

mod args;
pub mod assert;
mod printer;
mod run;

use crate::assert::Failed;
pub use args::Arguments;
use core::fmt;
pub use ktest_macros::{setup_harness, test};

#[doc(hidden)]
pub use run::run_tests;

pub type TestResult = Result<(), Failed>;

/// A single test case
pub struct Test {
    pub run: fn() -> Outcome,
    pub info: TestInfo<'static>,
}

/// Metadata associated with a test case
pub struct TestInfo<'a> {
    pub module: &'a str,
    pub name: &'a str,
    pub ignored: bool,
}

/// The outcome of performing a test.
#[derive(Clone)]
pub enum Outcome {
    /// The test passed.
    Passed,
    /// The test failed.
    Failed(Failed),
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
    /// have passed, 101 if there have been failures). This uses
    /// [`process::exit`], meaning that destructors are not ran. Consider
    /// using [`Self::exit_code`] instead for a proper program cleanup.
    pub fn exit(&self) -> ! {
        self.exit_if_failed();
        __private::exit(0)
    }

    /// Exits the application with error code 101 if there were any failures.
    /// Otherwise, returns normally. This uses [`process::exit`], meaning that
    /// destructors are not ran. Consider using [`Self::exit_code`] instead for
    /// a proper program cleanup.
    pub fn exit_if_failed(&self) {
        if self.has_failed() {
            __private::exit(101)
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

pub struct SetupInfo {
    pub is_std: bool,
    #[cfg(target_os = "none")]
    pub boot_info: &'static loader_api::BootInfo,
}

impl SetupInfo {
    #[cfg(target_os = "none")]
    pub fn new(boot_info: &'static loader_api::BootInfo) -> Self {
        Self {
            is_std: false,
            boot_info,
        }
    }

    #[cfg(not(target_os = "none"))]
    pub fn new() -> Self {
        Self { is_std: true }
    }
}

/// Marks a type which may be used as a test return type.
pub trait TestReport {
    /// Report any errors to `tracing`, then returns either `true` for a
    /// success, or `false` for a failure.
    fn report(self) -> Outcome;
}

impl TestReport for () {
    fn report(self) -> Outcome {
        Outcome::Passed
    }
}

impl TestReport for Result<(), Failed> {
    fn report(self) -> Outcome {
        match self {
            Ok(_) => Outcome::Passed,
            Err(failed) => Outcome::Failed(failed),
        }
    }
}

impl<T: fmt::Debug> TestReport for Result<(), T> {
    fn report(self) -> Outcome {
        match self {
            Ok(_) => Outcome::Passed,
            Err(_err) => Outcome::Failed(Failed::default()),
        }
    }
}
