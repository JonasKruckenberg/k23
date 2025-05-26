// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::sync::wait_cell::WaitCell;
use crate::time::Ticks;
use core::marker::PhantomPinned;
use core::mem::offset_of;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use util::loom_const_fn;

/// An entry in a timing [`Wheel`][crate::time::timer::Wheel].
#[derive(Debug)]
pub(in crate::time) struct Entry {
    pub(in crate::time) deadline: Ticks,
    pub(in crate::time) is_registered: AtomicBool,
    /// The currently-registered waker
    pub(in crate::time) waker: WaitCell,
    pub(in crate::time) links: linked_list::Links<Self>,
    _pin: PhantomPinned,
}

impl Entry {
    loom_const_fn! {
        pub(in crate::time) const fn new(deadline: Ticks) -> Entry {
            Self {
                deadline,
                waker: WaitCell::new(),
                is_registered: AtomicBool::new(false),
                links: linked_list::Links::new(),
                _pin: PhantomPinned,
            }
        }
    }

    pub(in crate::time) fn fire(&self) {
        let was_registered =
            self.is_registered
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire);
        tracing::trace!(was_registered = was_registered.is_ok(), "firing sleep!");
        self.waker.wake();
    }
}

// Safety: TODO
unsafe impl linked_list::Linked for Entry {
    type Handle = NonNull<Self>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}
