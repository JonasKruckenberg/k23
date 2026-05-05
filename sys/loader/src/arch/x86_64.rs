// Copyright 2026 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::x86_64::__cpuid_count;

use fdt::Fdt;

use crate::Result;

pub fn boot_hart_id(_fdt: Option<&Fdt<'_>>) -> Result<usize> {
    todo!()
}
