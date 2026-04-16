// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem::offset_of;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

use cordyceps::list;
use pin_project::pin_project;

use crate::PhysicalAddress;

#[pin_project(!Unpin)]
#[derive(Debug)]
pub struct Frame {
    addr: PhysicalAddress,
    refcount: AtomicUsize,
    #[pin]
    links: list::Links<Self>,
}

impl Frame {
    pub(crate) fn from_parts(addr: PhysicalAddress, initial_refcount: usize) -> Self {
        Self {
            addr,
            refcount: AtomicUsize::new(initial_refcount),
            links: list::Links::new(),
        }
    }

    pub fn refcount(&self, ordering: Ordering) -> usize {
        self.refcount.load(ordering)
    }

    pub fn is_unique(&self) -> bool {
        self.refcount(Ordering::Relaxed) == 1
    }

    pub fn addr(&self) -> PhysicalAddress {
        self.addr
    }
}

// Safety: TODO
unsafe impl cordyceps::Linked<list::Links<Self>> for Frame {
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
