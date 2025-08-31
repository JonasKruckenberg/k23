// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::tests::wast::wast_tests;

wast_tests!(
    fib "../../../tests/fib.wast",
    fib_imported "../../../tests/fib_imported.wast",
    hostfunc_rs "../../../tests/hostfunc_rs.wast",
    hostfunc_wat "../../../tests/hostfunc_wat.wast",
    test_io_functions "../../../tests/test_io_functions.wast",
    test_process_functions "../../../tests/test_process_functions.wast",
    test_memory_functions "../../../tests/test_memory_functions.wast",
    test_time_functions "../../../tests/test_time_functions.wast",
    test_filesystem_stubs "../../../tests/test_filesystem_stubs.wast",
    test_wasi_io "../../../tests/test_wasi_io.wast",
);
