// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Supervisor Trap Value Register

use super::{read_csr_as_usize, set_csr_as_usize};

read_csr_as_usize!(0x143);
set_csr_as_usize!(0x143);
