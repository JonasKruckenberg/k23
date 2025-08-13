// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::cmp::PartialEq;
use core::fmt;
use core::fmt::Debug;
use core::mem::offset_of;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic;
use core::sync::atomic::{AtomicUsize, Ordering};

use cordyceps::{Linked, list};
use pin_project::pin_project;

use crate::PhysicalAddress;
use crate::frame_alloc::FrameAllocator;

/// Soft limit on the amount of references that may be made to a `Frame`.
const MAX_REFCOUNT: usize = isize::MAX as usize;

pub struct FrameRef {
    frame: NonNull<Frame>,
    frame_alloc: &'static dyn FrameAllocator,
}

#[pin_project(!Unpin)]
#[derive(Debug)]
pub struct Frame {
    addr: PhysicalAddress,
    refcount: AtomicUsize,
    #[pin]
    links: list::Links<Self>,
}

// ===== impl FrameRef =====

impl Clone for FrameRef {
    /// Makes a clone of the `Frame`.
    ///
    /// This creates reference to the same `FrameInfo`, increasing the reference count by one.
    fn clone(&self) -> Self {
        // Increase the reference count by one. Using relaxed ordering, as knowledge of the
        // original reference prevents other threads from erroneously deleting
        // the object.
        //
        // Again, restating what the `Arc` implementation quotes from the
        // [Boost documentation][1]:
        //
        // > Increasing the reference counter can always be done with memory_order_relaxed: New
        // > references to an object can only be formed from an existing
        // > reference, and passing an existing reference from one thread to
        // > another must already provide any required synchronization.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        let old_size = self.refcount.fetch_add(1, Ordering::Relaxed);
        debug_assert_ne!(old_size, 0);

        // Just like with `Arc` we want to prevent excessive refcounts in the case that we are leaking
        // `Frame`s somewhere (which we really shouldn't but just in case). Overflowing the refcount
        // would *really* bad as it would treat the frame as free and potentially cause a use-after-free
        // scenario. Realistically this branch should never be taken.
        //
        // Also worth noting: Just like `Arc`, the refcount could still overflow when in between
        // the load above and this check some other cpu increased the refcount from `isize::MAX` to
        // `usize::MAX` but that seems unlikely. The other option, doing the comparison and update in
        // one conditional atomic operation produces much worse code, so if its good enough for the
        // standard library, it is good enough for us.
        assert!(old_size <= MAX_REFCOUNT, "Frame refcount overflow");

        unsafe { Self::from_raw_parts(self.frame, self.frame_alloc.clone()) }
    }
}

impl Drop for FrameRef {
    /// Drops the `Frame`.
    ///
    /// This will decrement the reference count. If the reference count reaches zero
    /// then this frame will be marked as free and returned to the frame allocator.
    fn drop(&mut self) {
        if self.refcount.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }

        // Ensure uses of `FrameInfo` happen before freeing it.
        // Because it is marked `Release`, the decreasing of the reference count synchronizes
        // with this `Acquire` fence. This means that use of `FrameInfo` happens before decreasing
        // the reference count, which happens before this fence, which happens before freeing `FrameInfo`.
        //
        // This section of the [Boost documentation][1] as quoted in Rusts `Arc` implementation and
        // may explain further:
        //
        // > It is important to enforce any possible access to the object in one
        // > thread (through an existing reference) to *happen before* deleting
        // > the object in a different thread. This is achieved by a "release"
        // > operation after dropping a reference (any access to the object
        // > through this reference must obviously happened before), and an
        // > "acquire" operation before deleting the object.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        atomic::fence(Ordering::Acquire);

        self.drop_slow();
    }
}

impl Debug for FrameRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameRef")
            .field("ptr", &self.frame)
            .finish_non_exhaustive()
    }
}

impl Deref for FrameRef {
    type Target = Frame;

    fn deref(&self) -> &Self::Target {
        unsafe { self.frame.as_ref() }
    }
}

impl FrameRef {
    pub unsafe fn from_raw_parts(frame: NonNull<Frame>, alloc: &'static dyn FrameAllocator) -> Self {
        Self { frame, frame_alloc: alloc }
    }
    
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        a.frame == b.frame
    }

    #[inline(never)]
    fn drop_slow(&mut self) {
        let layout = unsafe {
            Layout::from_size_align_unchecked(self.frame_alloc.page_size(), self.frame_alloc.page_size())
        };
        unsafe {
            self.frame_alloc.deallocate(self.frame, layout);
        }
    }
}

// ===== impl Frame =====

// Safety: assert_impl_all! above ensures that `FrameInfo` is `Send`
unsafe impl Send for Frame {}

// Safety: assert_impl_all! above ensures that `FrameInfo` is `Sync`
unsafe impl Sync for Frame {}

impl PartialEq<Frame> for &Frame {
    fn eq(&self, other: &Frame) -> bool {
        self.refcount() == other.refcount() && self.addr == other.addr
    }
}

impl Frame {
    pub fn new(addr: PhysicalAddress, initial_refcount: usize) -> Self {
        Self {
            addr,
            refcount: AtomicUsize::new(initial_refcount),
            links: list::Links::new(),
        }
    }

    pub fn refcount(&self) -> usize {
        self.refcount.load(Ordering::Relaxed)
    }

    pub fn is_unique(&self) -> bool {
        self.refcount() == 1
    }
    
    pub fn addr(&self) -> PhysicalAddress {
        self.addr
    }
}

unsafe impl Linked<list::Links<Self>> for Frame {
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
