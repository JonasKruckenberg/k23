use crate::args::Arguments;
use crate::printer::Printer;
use crate::{Conclusion, Outcome, Test, TestInfo};
use core::ffi::CStr;
use core::fmt::Write;
use core::ptr::addr_of;
use core::{fmt, hint, mem, slice};
use dtb_parser::{DevTree, Node, Visitor};

#[no_mangle]
extern "Rust" fn kmain(_hartid: usize, boot_info: &'static loader_api::BootInfo) -> ! {
    struct Log;

    impl fmt::Write for Log {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            riscv::hio::HostStream::new_stdout().write_str(s)?;

            Ok(())
        }
    }

    let machine_info = unsafe { MachineInfo::from_dtb(boot_info.fdt_offset.as_raw() as *const u8) };
    let args = machine_info
        .bootargs
        .map(|bootargs| Arguments::from_str(bootargs.to_str().unwrap()))
        .unwrap_or_default();

    run_tests(&mut Log, args, boot_info).exit();
}

pub fn run_tests(
    write: &mut dyn Write,
    args: Arguments,
    boot_info: &'static loader_api::BootInfo,
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

#[cfg(target_os = "none")]
pub struct MachineInfo<'dt> {
    pub bootargs: Option<&'dt CStr>,
}

#[cfg(target_os = "none")]
impl MachineInfo<'_> {
    /// # Safety
    ///
    /// The caller has to ensure the provided pointer actually points to a FDT in memory.
    pub unsafe fn from_dtb(dtb_ptr: *const u8) -> Self {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }.unwrap();
        let mut v = BootInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        MachineInfo {
            bootargs: v.bootargs,
        }
    }
}

#[cfg(target_os = "none")]
#[derive(Default)]
struct BootInfoVisitor<'dt> {
    bootargs: Option<&'dt CStr>,
}

#[cfg(target_os = "none")]
impl<'dt> Visitor<'dt> for BootInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name == "chosen" || name.is_empty() {
            node.visit(self)?;
        }

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "bootargs" {
            self.bootargs = Some(CStr::from_bytes_until_nul(value).unwrap());
        }

        Ok(())
    }
}
