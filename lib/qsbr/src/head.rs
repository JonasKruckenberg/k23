// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::{self, NonNull};

use cordyceps::{stack, Linked};

pub struct QsbrHead {
    links: stack::Links<Self>,
    /// The epoch this node was retired in, set by
    /// [`QsbrDomain::retire`](crate::QsbrDomain::retire).
    /// This node can be reclaimed once that epoch is complete.
    pub(crate) epoch: u64,
    /// Destructor of the type that embedded the retirement node.
    /// Run by [`QsbrDomain::reclaim`](crate::QsbrDomain::reclaim).
    pub(crate) drop_fn: unsafe fn(NonNull<QsbrHead>),
}

impl QsbrHead {
    /// Creates an unqueued retirement node with the given destructor.
    pub const fn new(drop_fn: unsafe fn(NonNull<Self>)) -> Self {
        Self {
            links: stack::Links::new(),
            epoch: 0,
            drop_fn,
        }
    }
}

// SAFETY: `Handle = NonNull<Retired>` is non-owning; validity and pinning
// of queued nodes is exactly the contract of `QsbrDomain::retire`.
unsafe impl Linked<stack::Links<QsbrHead>> for QsbrHead {
    type Handle = NonNull<QsbrHead>;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(target: NonNull<Self>) -> NonNull<stack::Links<QsbrHead>> {
        // SAFETY: raw field projection; no intermediate reference formed.
        unsafe { NonNull::new_unchecked(ptr::addr_of_mut!((*target.as_ptr()).links)) }
    }
}
