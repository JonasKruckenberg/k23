// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::slice;
use core::marker::PhantomData;
use core::mem::offset_of;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{fmt, ptr};

use cordyceps::{Linked, list};
use kmem::PhysicalAddress;
use static_assertions::assert_impl_all;

use crate::arch;
use crate::mem::frame_alloc::FRAME_ALLOC;

/// Soft limit on the amount of references that may be made to a `Frame`.
const MAX_REFCOUNT: usize = isize::MAX as usize;

/// A thread-safe reference-counted pointer to a frame of physical memory.
///
/// This type is similar to [`alloc::sync::Arc`][1] and provides shared ownership of a [`FrameInfo`] instance
/// and by extension its associated physical memory. Just like [`Arc`][1] invoking [`clone`][Frame::clone] will
/// produce a new `Frame` instance, which points to the same [`FrameInfo`] instance as the source `Frame`, while
/// increasing its reference count. When the last `Frame` is dropped instance is dropped, the underlying
/// [`FrameInfo`] is marked as *free* and returned to the frame allocator freelists.
///
/// [1]: [alloc::sync::Arc]
pub struct Frame {
    ptr: NonNull<FrameInfo>,
    phantom: PhantomData<FrameInfo>,
}

pub struct FrameInfo {
    /// Links to other frames in a freelist either a global `Arena` or cpu-local page cache.
    links: list::Links<FrameInfo>,
    /// Number of references to this frame, zero indicates a free frame.
    refcount: AtomicUsize,
    /// The physical address of the frame.
    addr: PhysicalAddress,
}
assert_impl_all!(FrameInfo: Send, Sync);

// === Frame ===

// Safety: assert_impl_all! above ensures that `FrameInfo` is `Send`
unsafe impl Send for Frame {}

// Safety: assert_impl_all! above ensures that `FrameInfo` is `Sync`
unsafe impl Sync for Frame {}

impl Clone for Frame {
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
        let old_size = self.info().refcount.fetch_add(1, Ordering::Relaxed);
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

        // Safety: self was valid so it's info ptr is as well
        unsafe { Self::from_info(self.ptr) }
    }
}
impl Drop for Frame {
    /// Drops the `Frame`.
    ///
    /// This will decrement the reference count. If the reference count reaches zero
    /// then this frame will be marked as free and returned to the frame allocator.
    fn drop(&mut self) {
        if self.info().refcount.fetch_sub(1, Ordering::Release) != 1 {
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

impl Frame {
    #[inline]
    #[must_use]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        ptr::addr_eq(this.ptr.as_ptr(), other.ptr.as_ptr())
    }

    /// Asserts the `Frame` is in a valid state.
    pub fn assert_valid(&self, ctx: &str) {
        let refcount = self.info().refcount.load(Ordering::Acquire);
        assert!(
            refcount < MAX_REFCOUNT,
            "{ctx}refcount overflowed (must only be within 0..{MAX_REFCOUNT})"
        );
        self.info().assert_valid(ctx);
    }

    #[inline]
    fn info(&self) -> &FrameInfo {
        // Safety: Through `Clone` and `Drop` we're guaranteed that the FrameInfo remains valid as long as
        // this Frame is alive, we also know that `FrameInfo` is `Sync` and therefore - analogous to `Arc` -
        // handing out an immutable reference is fine.
        // Because it is an immutable reference, safe code can also not move out of `FrameInner`.
        unsafe { self.ptr.as_ref() }
    }

    pub(crate) unsafe fn from_free_info(info: NonNull<FrameInfo>) -> Frame {
        // Safety: caller has to ensure ptr is valid
        unsafe {
            let prev_refcount = info.as_ref().refcount.swap(1, Ordering::Acquire);
            debug_assert_eq!(
                prev_refcount, 0,
                "attempted to create Frame from non-free FrameInfo"
            );

            Self::from_info(info)
        }
    }

    #[inline]
    unsafe fn from_info(info: NonNull<FrameInfo>) -> Self {
        Self {
            ptr: info,
            phantom: PhantomData,
        }
    }

    pub fn get_mut(this: &mut Frame) -> Option<&mut FrameInfo> {
        if this.is_unique() {
            // Safety: This unsafety is ok because we're guaranteed that the pointer
            // returned is the *only* pointer that will ever be returned to T. Our
            // reference count is guaranteed to be 1 at this point, and we required
            // the Arc itself to be `mut`, so we're returning the only possible
            // reference to the inner data.
            unsafe { Some(Frame::get_mut_unchecked(this)) }
        } else {
            None
        }
    }

    pub unsafe fn get_mut_unchecked(this: &mut Self) -> &mut FrameInfo {
        // Safety: construction ensures the base ptr is valid
        unsafe { this.ptr.as_mut() }
    }

    pub fn is_unique(&self) -> bool {
        self.refcount() == 1
    }

    #[inline(never)]
    fn drop_slow(&mut self) {
        // TODO if we ever add more fields to FrameInfo we should reset them here

        let alloc = FRAME_ALLOC
            .get()
            .expect("cannot access FRAME_ALLOC before it is initialized");
        let mut cpu_local_cache = alloc.cpu_local_cache.get().unwrap().borrow_mut();
        cpu_local_cache.free_list.push_back(self.ptr);
    }
}

impl Deref for Frame {
    type Target = FrameInfo;

    #[inline]
    fn deref(&self) -> &FrameInfo {
        self.info()
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Safety: construction ensures the base ptr is valid
        fmt::Debug::fmt(unsafe { self.ptr.as_ref() }, f)
    }
}

impl fmt::Pointer for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr, f)
    }
}

// === FrameInfo ===

impl FrameInfo {
    /// Private constructor for use in `frame_alloc/arena.rs`
    pub(crate) fn new(addr: PhysicalAddress) -> Self {
        Self {
            links: list::Links::default(),
            addr,
            refcount: AtomicUsize::new(0),
        }
    }

    /// The physical address of this frame.
    pub fn addr(&self) -> PhysicalAddress {
        self.addr
    }

    /// Gets the number of references to this frame.
    #[inline]
    #[must_use]
    pub fn refcount(&self) -> usize {
        self.refcount.load(Ordering::Relaxed)
    }

    /// Returns a slice of the corresponding physical memory
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        let base = arch::phys_to_virt(self.addr).as_ptr();
        // Safety: construction ensures the base ptr is valid
        unsafe { slice::from_raw_parts(base, arch::PAGE_SIZE) }
    }

    /// Returns a mutable slice of the corresponding physical memory
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        let base = arch::phys_to_virt(self.addr).as_mut_ptr();
        // Safety: construction ensures the base ptr is valid
        unsafe { slice::from_raw_parts_mut(base, arch::PAGE_SIZE) }
    }

    #[inline]
    pub fn assert_valid(&self, _ctx: &str) {
        // TODO add asserts here as we add more fields
    }
}

/// Implement the cordyceps [Linked] trait so that [FrameInfo] can be used
/// with coryceps linked [List].
// Safety: unsafe trait
unsafe impl Linked<list::Links<FrameInfo>> for FrameInfo {
    type Handle = NonNull<FrameInfo>;

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

impl fmt::Debug for FrameInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameInfo")
            .field("refcount", &self.refcount)
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}
