// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod args;
mod printer;
mod smoke;
mod spectest;
mod wast;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::any::Any;
use core::ptr::addr_of;
use core::sync::atomic::{AtomicU64, Ordering};
use core::{hint, slice};

use futures::FutureExt;
use futures::future::try_join_all;
use ktest::Test;

use crate::tests::args::Arguments;
use crate::tests::printer::Printer;
use crate::{arch, state};

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
    pub num_filtered_out: AtomicU64,

    /// Number of passed tests.
    pub num_passed: AtomicU64,

    /// Number of failed tests and benchmarks.
    pub num_failed: AtomicU64,

    /// Number of ignored tests and benchmarks.
    pub num_ignored: AtomicU64,

    /// Number of benchmarks that successfully ran.
    pub num_measured: AtomicU64,
}

impl Conclusion {
    /// Exits the application with error code 101 if there were any failures.
    /// Otherwise, returns normally. **This will not run any destructors.**
    /// Consider using [`Self::exit_code`] instead for a proper program cleanup.
    pub fn exit_if_failed(&self) {
        if self.has_failed() {
            arch::exit(101)
        }
    }

    /// Returns whether there have been any failures.
    pub fn has_failed(&self) -> bool {
        self.num_failed.load(Ordering::Acquire) > 0
    }

    fn empty() -> Self {
        Self {
            num_filtered_out: AtomicU64::new(0),
            num_passed: AtomicU64::new(0),
            num_failed: AtomicU64::new(0),
            num_ignored: AtomicU64::new(0),
            num_measured: AtomicU64::new(0),
        }
    }
}

pub async fn run_tests(global: &'static state::Global) -> Conclusion {
    let chosen = global.device_tree.find_by_path("/chosen").unwrap();
    let args = if let Some(prop) = chosen.property("bootargs") {
        let str = prop.as_str().unwrap();
        Arguments::from_str(str)
    } else {
        Arguments::default()
    };

    let tests = all_tests();

    // Create printer which is used for all output.
    let printer = Printer::new(args.format);

    // If `--list` is specified, just print the list and return.
    if args.list {
        printer.print_list(tests, args.ignored);
        return Conclusion::empty();
    }

    // Print number of tests
    printer.print_title(tests.len() as u64);

    let printer = Arc::new(printer);
    let conclusion = Arc::new(Conclusion::empty());

    let tests = tests.iter().map(|test| {
        if args.is_ignored(test) {
            printer.print_test(&test.info);
            conclusion.num_ignored.fetch_add(1, Ordering::Release);
            futures::future::Either::Left(core::future::ready(Ok(())))
        } else {
            let printer = printer.clone();
            let conclusion = conclusion.clone();

            let h = global
                .executor
                .try_spawn(async move {
                    // Print `test foo    ...`, run the test, then print the outcome in
                    // the same line.
                    printer.print_test(&test.info);
                    (test.run)().await;
                })
                .unwrap()
                .inspect(move |res| match res {
                    Ok(_) => {
                        conclusion.num_passed.fetch_add(1, Ordering::Release);
                    }
                    Err(_) => {
                        conclusion.num_failed.fetch_add(1, Ordering::Release);
                    }
                });

            futures::future::Either::Right(h)
        }
    });

    // we handle test failures through `Conclusion` so it is safe to ignore the result here
    let _ = try_join_all(tests).await;

    printer.print_summary(&conclusion);

    Arc::into_inner(conclusion).unwrap()
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
