// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::park::Park;
use crate::time::{Clock, Deadline};

/// Returns a [`Clock`] with 1ms precision that is backed by the system clock
macro_rules! std_clock {
    () => {{
        crate::loom::lazy_static! {
            static ref TIME_ANCHOR: ::std::time::Instant = ::std::time::Instant::now();
        }

        $crate::time::Clock::new(::core::time::Duration::from_millis(1), move || {
            $crate::time::Ticks(TIME_ANCHOR.elapsed().as_millis() as u64)
        })
    }};
}
use crate::executor::Executor;
pub(crate) use std_clock;

pub struct StdPark(crate::loom::thread::Thread);

impl Park for StdPark {
    fn park(&self) {
        tracing::trace!("parking current thread ({:?})...", self.0);
        crate::loom::thread::park();
    }

    #[cfg(not(loom))]
    fn park_until(&self, deadline: Deadline, clock: &Clock) {
        let instant = deadline.as_instant(clock);
        let dur = instant.elapsed(clock);
        crate::loom::thread::park_timeout(dur);
    }

    #[cfg(loom)]
    fn park_until(&self, _deadline: Deadline, _clock: &Clock) {
        unreachable!("loom doesn't support `park_timeout`");
    }

    fn unpark(&self) {
        tracing::trace!("unparking thread {:?}...", self.0);
        self.0.unpark();
    }
}

impl StdPark {
    pub fn for_current() -> Self {
        Self(crate::loom::thread::current())
    }
}

#[must_use]
pub struct StopOnPanic<'e, P: Park + Send + Sync> {
    exec: &'e Executor<P>,
}
impl<'e, P: Park + Send + Sync> StopOnPanic<'e, P> {
    pub fn new(exec: &'e Executor<P>) -> Self {
        Self { exec }
    }
}
impl<'e, P: Park + Send + Sync> Drop for StopOnPanic<'e, P> {
    fn drop(&mut self) {
        self.exec.stop();
    }
}
