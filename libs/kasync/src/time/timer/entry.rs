// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomPinned;
use core::mem::offset_of;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use cordyceps::{Linked, list};
use k32_util::loom_const_fn;
use pin_project::pin_project;

use crate::sync::wait_cell::WaitCell;
use crate::time::Ticks;

/// An entry in a timing [`Wheel`][crate::time::timer::Wheel].
#[pin_project]
#[derive(Debug)]
pub(in crate::time) struct Entry {
    pub(in crate::time) deadline: Ticks,
    pub(in crate::time) is_registered: AtomicBool,
    /// The currently-registered waker
    pub(in crate::time) waker: WaitCell,
    #[pin]
    links: list::Links<Self>,
    // This type is !Unpin due to the heuristic from:
    // <https://github.com/rust-lang/rust/pull/82834>
    _pin: PhantomPinned,
}

impl Entry {
    loom_const_fn! {
        pub(in crate::time) const fn new(deadline: Ticks) -> Entry {
            Self {
                deadline,
                waker: WaitCell::new(),
                is_registered: AtomicBool::new(false),
                links: list::Links::new(),
                _pin: PhantomPinned,
            }
        }
    }

    pub(in crate::time) fn fire(&self) {
        let was_registered =
            self.is_registered
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire);
        tracing::trace!(was_registered = was_registered.is_ok(), "firing sleep!");
        self.waker.close();
    }
}

// Safety: TODO
unsafe impl Linked<list::Links<Entry>> for Entry {
    type Handle = NonNull<Self>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}
