// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod instant;
mod system_time;

pub use core::time::Duration;
pub use instant::Instant;

const NANOS_PER_SEC: u64 = 1_000_000_000;

pub fn ticks_to_duration(ticks: u64, timebase_freq: u64) -> Duration {
    let secs = ticks / timebase_freq;
    #[expect(clippy::cast_possible_truncation, reason = "truncation on purpose")]
    let subsec_nanos = ((ticks % timebase_freq) * NANOS_PER_SEC / timebase_freq) as u32;
    Duration::new(secs, subsec_nanos)
}

/// # Safety
///
/// In release mode this function does not protect against integer overflow, it is the
/// callers responsibility to ensure that durations passed to this function are within as safe
/// margin.
pub unsafe fn duration_to_ticks_unchecked(d: Duration, timebase_freq: u64) -> u64 {
    d.as_secs() * timebase_freq + u64::from(d.subsec_nanos()) * (timebase_freq / NANOS_PER_SEC)
}

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
//         log::trace!("Time elapsed: {elapsed:?}");
//
//         assert_eq!(elapsed.as_secs(), 1);
//         // assert_eq!(start_sys.elapsed().unwrap().as_secs(), 1)
//     }
// }
