// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::time::Duration;

#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// No global default clock has been set
    NoGlobalClock,
    DurationTooLong {
        requested: Duration,
        max: Duration,
    },
}
