// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod args;
mod panic;
mod printer;
mod smoke;
mod spectest;
pub(crate) mod wast;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::ptr::addr_of;
use core::sync::atomic::{AtomicU64, Ordering};
use core::{hint, slice};

use futures::FutureExt;
use futures::future::try_join_all;
use test::Test;

use crate::state;
use crate::tests::args::Arguments;
use crate::tests::printer::Printer;
use crate::util::either::Either;

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
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            riscv::semihosting::exit(101);

            loop {
                hint::spin_loop();
            }
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

pub async fn run_tests(global: &'static state::Global) -> crate::Result<Conclusion> {
    let raw = crate::bootargs::read_raw(&global.device_tree).unwrap_or("");
    let args = Arguments::parse(raw)?;
    let all_tests = all_tests().iter();

    #[expect(
        clippy::needless_collect,
        reason = "we need the collect to ensure we have an ExactSizeIterator later."
    )]
    let tests = if let Some(test_name) = args.test_name {
        let tests: Vec<_> = all_tests
            .filter(|test| test.info.ident.contains(test_name))
            .collect();

        Either::Left(tests.into_iter())
    } else {
        Either::Right(all_tests)
    };

    // Create printer which is used for all output.
    let printer = Printer::new(args.format);

    // If `--list` is specified, just print the list and return.
    if args.list {
        printer.print_list(tests, args.ignored);
        return Ok(Conclusion::empty());
    }

    // Print number of tests
    printer.print_title(tests.len() as u64);

    let printer = Arc::new(printer);
    let conclusion = Arc::new(Conclusion::empty());

    let tests = tests.map(|test| {
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

    Ok(Arc::into_inner(conclusion).unwrap())
}

pub fn all_tests() -> &'static [Test] {
    #[used(linker)]
    #[unsafe(link_section = "k23_tests")]
    static mut LINKME_PLEASE: [Test; 0] = [];

    unsafe extern "C" {
        #[expect(
            improper_ctypes,
            reason = "These are linker-defined markers, not real C externs. We only take their addresses."
        )]
        static __start_k23_tests: Test;
        #[expect(
            improper_ctypes,
            reason = "These are linker-defined markers, not real C externs. We only take their addresses."
        )]
        static __stop_k23_tests: Test;
    }

    let start = addr_of!(__start_k23_tests);
    let stop = addr_of!(__stop_k23_tests);

    let stride = size_of::<Test>();
    let byte_offset = stop as usize - start as usize;

    let Some(len) = byte_offset.checked_div(stride) else {
        // Safety: `byte_offset.checked_div(stride)` returns `None` only when `stride == 0`,
        // i.e. `size_of::<Test>() == 0`. The `#[distributed_slice]` macro asserts `size_of::<T>() > 0`
        // so this branch is unreachable.
        unsafe { hint::unreachable_unchecked() }
    };

    // Safety: the linker populates `k23_tests` with properly aligned, initialized `Test` entries
    // (registered by `#[ktest::test]`).
    unsafe { slice::from_raw_parts(start, len) }
}
