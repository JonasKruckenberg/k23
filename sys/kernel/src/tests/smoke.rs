// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::tests::wast::wast_tests;

wast_tests!(
    fib "../../wast/tests/fib.wast",
    fib_imported "../../wast/tests/fib_imported.wast",
    hostfunc_rs "../../wast/tests/hostfunc_rs.wast",
    hostfunc_wat "../../wast/tests/hostfunc_wat.wast",
    trap "../../wast/tests/trap.wast",
);
