// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use alloc::vec::Vec;
use std::convert::Infallible;
use std::sync::Mutex;

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

/// Process-global owner of every per-thread [`NOTIFY`].
///
/// Each `Notify` must outlive the thread that created it: the executor keeps
/// `Waker`s cloned from it, and `block_on` requires `&'static Notify`. So they
/// are intentionally never freed. Rooting them in this `static` keeps the
/// leaked allocations *reachable*, so Miri's leak checker treats them — and the
/// `Thread` handles they transitively own — as live rather than flagging them.
static NOTIFY_REGISTRY: Mutex<Vec<&'static Notify<StdPark>>> = Mutex::new(Vec::new());

crate::loom::thread_local! {
    /// Per-thread `Notify<StdPark>`. Lazily allocated on first use so that
    /// `StdPark::current()` captures the calling thread, and leaked so the
    /// `&'static Notify` requirement of `block_on` is satisfied.
    static NOTIFY: &'static Notify<StdPark> = {
        let notify: &'static Notify<StdPark> =
            Box::leak(Box::new(Notify::new(StdPark::current())));
        NOTIFY_REGISTRY.lock().unwrap().push(notify);
        notify
    };
}

pub fn block_on<F: Future>(f: F) -> F::Output {
    NOTIFY.with(|notify| {
        // Safety: `StdPark::Error` is infallible
        unsafe { crate::block_on::block_on(*notify, f).unwrap_unchecked() }
    })
}
