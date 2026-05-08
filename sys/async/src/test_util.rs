// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use std::convert::Infallible;

use crate::block_on::{Notify, Park};
use crate::loom::thread::{self, Thread};

/// `Park` implementation backed by `std::thread::{park, Thread::unpark}`
/// (or the loom equivalent).
///
/// `unpark` is a token: a call before a matching `park` causes the next
/// `park` on the captured thread to return immediately, which is what
/// [`Notify`] requires.
pub struct StdPark {
    thread: Thread,
}

impl StdPark {
    pub fn current() -> Self {
        Self {
            thread: thread::current(),
        }
    }
}

impl Park for StdPark {
    type Error = Infallible;

    fn park(&self) -> Result<(), Self::Error> {
        thread::park();
        Ok(())
    }

    fn unpark(&self) -> Result<(), Self::Error> {
        self.thread.unpark();
        Ok(())
    }
}

crate::loom::thread_local! {
    /// Per-thread `Notify<StdPark>`. Lazily allocated on first use so that
    /// `StdPark::current()` captures the calling thread, and leaked so the
    /// `&'static Notify` requirement of `block_on` is satisfied.
    static NOTIFY: &'static Notify<StdPark> =
        Box::leak(Box::new(Notify::new(StdPark::current())));
}

pub fn block_on<F: Future>(f: F) -> F::Output {
    NOTIFY.with(|notify| crate::block_on::block_on(*notify, f).unwrap())
}
