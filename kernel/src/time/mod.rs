// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![allow(unused_imports, reason = "reexporting")]

pub mod clock;
mod instant;
mod sleep;
mod system_time;
mod timeout;
mod timer;

pub const NANOS_PER_SEC: u64 = 1_000_000_000;

use crate::arch::device::cpu::with_cpu;
pub use clock::Clock;
use core::future::Future;
use core::pin::pin;
use core::task::Context;
pub use core::time::Duration;
pub use instant::Instant;
pub use sleep::{Sleep, sleep, sleep_until};
pub use timeout::{Elapsed, Timeout, timeout};
pub use timer::{Deadline, Timer};

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use core::time::Duration;
//
//     #[ktest::test]
//     fn measure_and_timeout() {
//         // let start_sys = SystemTime::now();
//         let start = Instant::now();
//
//         unsafe {
//             sleep(Duration::from_secs(1));
//         }
//
//         let end = Instant::now();
//         let elapsed = end.duration_since(start);
//         tracing::trace!("Time elapsed: {elapsed:?}");
//
//         assert_eq!(elapsed.as_secs(), 1);
//         // assert_eq!(start_sys.elapsed().unwrap().as_secs(), 1)
//     }
// }
