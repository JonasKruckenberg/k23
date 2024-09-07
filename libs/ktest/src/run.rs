use crate::args::Arguments;
use crate::printer::Printer;
use crate::{Conclusion, Outcome, Test, TestInfo};
use core::ptr::addr_of;
use core::{fmt, hint, mem, slice};
use loader_api::BootInfo;

pub fn run_tests(
    write: &mut dyn fmt::Write,
    args: Arguments,
    boot_info: &'static BootInfo,
) -> Conclusion {
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
            match panic_unwind::catch_unwind(|| (test.run)(boot_info)) {
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
    #[link_section = "k23_tests"]
    static mut LINKME_PLEASE: [Test; 0] = [];

    extern "C" {
        #[allow(improper_ctypes)]
        static __start_k23_tests: Test;
        #[allow(improper_ctypes)]
        static __stop_k23_tests: Test;
    }

    let start = addr_of!(__start_k23_tests);
    let stop = addr_of!(__stop_k23_tests);

    let stride = mem::size_of::<Test>();
    let byte_offset = stop as usize - start as usize;
    let len = match byte_offset.checked_div(stride) {
        Some(len) => len,
        // The #[distributed_slice] call checks `size_of::<T>() > 0` before
        // using the unsafe `private_new`.
        None => unsafe { hint::unreachable_unchecked() },
    };

    unsafe { slice::from_raw_parts(start, len) }
}
