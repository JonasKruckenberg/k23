// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::ptr::NonNull;
use core::{fmt, ptr};

use util::loom_const_fn;

use crate::loom::sync::atomic::{AtomicPtr, Ordering};
use crate::{Guard, Reclaimable};

/// An atomic pointer that can be safely shared between threads.
///
/// All read access requires a [`Guard`].
pub struct Atomic<T: Reclaimable> {
    ptr: AtomicPtr<T>,
    /// `AtomicPtr<T>` is `Send` and `Sync` for every `T`; this pins the auto
    /// impls to the ones justified below.
    _value: PhantomData<*const T>,
}

// SAFETY: an `Atomic<T>` hands the pointee to whichever CPU loads or replaces
// it, which is sending a `T` and sharing a `&T`.
unsafe impl<T: Send + Sync + Reclaimable> Send for Atomic<T> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync + Reclaimable> Sync for Atomic<T> {}

impl<T: Reclaimable> Atomic<T> {
    loom_const_fn! {
        /// Returns an `Atomic` holding no value.
        #[must_use]
        pub const fn null() -> Self {
            Self {
                ptr: AtomicPtr::new(ptr::null_mut()),
                _value: PhantomData,
            }
        }
    }

    /// Loads a [`Shared`] from the atomic pointer.
    #[inline]
    pub fn load<'g>(&self, order: Ordering, _: &'g Guard<'_>) -> Option<Shared<'g, T>> {
        NonNull::new(self.ptr.load(order)).map(Shared::from_ptr)
    }

    /// Stores `ptr` into the atomic pointer.
    ///
    /// # Safety
    ///
    /// The pointee must outlive every read section that can observe it.
    /// In practice this means you *must* use the QSBR
    /// [`retire`][crate::Local::retire] mechanism.
    #[inline]
    pub unsafe fn store(&self, ptr: Option<NonNull<T>>, order: Ordering) {
        self.ptr
            .store(ptr.map_or(ptr::null_mut(), NonNull::as_ptr), order);
    }

    /// Stores `ptr` and returns the previous [`Shared`].
    ///
    /// # Safety
    ///
    /// As [`store`][Self::store].
    #[inline]
    pub unsafe fn swap<'g>(
        &self,
        ptr: Option<NonNull<T>>,
        order: Ordering,
        _: &'g Guard<'_>,
    ) -> Option<Shared<'g, T>> {
        let ptr = ptr.map_or(ptr::null_mut(), NonNull::as_ptr);

        NonNull::new(self.ptr.swap(ptr, order)).map(Shared::from_ptr)
    }

    /// Stores `new` if the current value is `current`, returning the previous
    /// value on success and the one actually found on failure.
    ///
    /// Prefer [`compare_exchange_weak`][Self::compare_exchange_weak] in a retry
    /// loop.
    ///
    /// # Errors
    ///
    /// Fails if the current value is not `current`, returning what was found
    /// instead.
    ///
    /// # Safety
    ///
    /// As [`store`][Self::store].
    #[inline]
    pub unsafe fn compare_exchange<'g>(
        &self,
        current: Option<Shared<'_, T>>,
        new: Option<NonNull<T>>,
        success: Ordering,
        failure: Ordering,
        _: &'g Guard<'_>,
    ) -> Result<Option<Shared<'g, T>>, Option<Shared<'g, T>>> {
        let current = current.map_or(ptr::null_mut(), |current| current.as_ptr().as_ptr());
        let new = new.map_or(ptr::null_mut(), NonNull::as_ptr);

        match self.ptr.compare_exchange(current, new, success, failure) {
            Ok(previous) => Ok(NonNull::new(previous).map(Shared::from_ptr)),
            Err(actual) => Err(NonNull::new(actual).map(Shared::from_ptr)),
        }
    }

    /// Like [`compare_exchange`][Self::compare_exchange], but allowed to fail
    /// spuriously.
    ///
    /// This is the one to reach for in a retry loop: the architectures we
    /// target are load-linked/store-conditional, where the strong form is
    /// itself a loop, so using it inside one nests two.
    ///
    /// # Errors
    ///
    /// Fails if the current value is not `current`, returning what was found
    /// instead — or spuriously, even when it is.
    ///
    /// # Safety
    ///
    /// As [`store`][Self::store].
    #[inline]
    pub unsafe fn compare_exchange_weak<'g>(
        &self,
        current: Option<Shared<'_, T>>,
        new: Option<NonNull<T>>,
        success: Ordering,
        failure: Ordering,
        _: &'g Guard<'_>,
    ) -> Result<Option<Shared<'g, T>>, Option<Shared<'g, T>>> {
        let current = current.map_or(ptr::null_mut(), |current| current.as_ptr().as_ptr());
        let new = new.map_or(ptr::null_mut(), NonNull::as_ptr);

        match self
            .ptr
            .compare_exchange_weak(current, new, success, failure)
        {
            Ok(previous) => Ok(NonNull::new(previous).map(Shared::from_ptr)),
            Err(actual) => Err(NonNull::new(actual).map(Shared::from_ptr)),
        }
    }
}

impl<T: Reclaimable> Default for Atomic<T> {
    fn default() -> Self {
        Self::null()
    }
}

impl<T: Reclaimable> fmt::Debug for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr.load(Ordering::Relaxed), f)
    }
}

/// A pointer loaded from an [`Atomic`], valid for the rest of the read section
/// it was loaded in.
///
/// `'g` is borrowed from the [`Guard`] [`Atomic::load`] was handed, so a
/// reference taken out of it cannot outlive the section that keeps it alive.
pub struct Shared<'g, T> {
    ptr: NonNull<T>,
    /// Ties this pointer to the read section, and keeps it off other CPUs,
    /// where the section proves nothing.
    _guard: PhantomData<(&'g T, *const ())>,
}

impl<'g, T> Shared<'g, T> {
    /// Private on purpose: `'g` is only meaningful when it comes from the guard
    /// an [`Atomic`] was read with, and forging one would be a use-after-free.
    #[inline]
    fn from_ptr(ptr: NonNull<T>) -> Self {
        Self {
            ptr,
            _guard: PhantomData,
        }
    }

    /// Borrows the value for the rest of the read section.
    #[inline]
    #[must_use]
    pub fn as_ref(self) -> &'g T {
        // SAFETY: `Atomic::store` promised the value outlives every read
        // section that can observe it, and `'g` is bounded by this one.
        unsafe { self.ptr.as_ref() }
    }

    /// Returns the raw pointer.
    ///
    /// Dereferencing it is only sound for as long as [`as_ref`][Self::as_ref]'s
    /// return value would have been.
    #[inline]
    #[must_use]
    pub fn as_ptr(self) -> NonNull<T> {
        self.ptr
    }
}

impl<T> Clone for Shared<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Shared<'_, T> {}

impl<T> PartialEq for Shared<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<T> Eq for Shared<'_, T> {}

impl<T> fmt::Debug for Shared<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr, f)
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use core::mem::offset_of;
    use core::sync::atomic::AtomicUsize;

    use super::*;
    use crate::{Domain, Links, Local};

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    /// Stands in for a task header: an identity to validate against, a payload,
    /// and an intrusive hook that is deliberately not the first field.
    struct Item {
        id: u64,
        links: Links,
    }

    impl Item {
        fn new(id: u64) -> NonNull<Self> {
            NonNull::from(Box::leak(Box::new(Item {
                id,
                links: Links::new(),
            })))
        }
    }

    impl Drop for Item {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    // SAFETY: `links` is `Item`'s own hook, and `from_ptr` undoes the
    // `Box::leak` in `new`.
    unsafe impl Reclaimable for Item {
        type Handle = Box<Self>;
        const LINKS_OFFSET: usize = offset_of!(Self, links);

        unsafe fn from_ptr(ptr: NonNull<Self>) -> Box<Self> {
            unsafe { Box::from_raw(ptr.as_ptr()) }
        }
    }

    /// The shape a task table's lookup takes: resolve, then validate identity
    /// against the item itself, with no `unsafe` at all.
    fn lookup<'g>(slot: &Atomic<Item>, id: u64, guard: &'g Guard<'_>) -> Option<Shared<'g, Item>> {
        let item = slot.load(Ordering::Acquire, guard)?;

        (item.as_ref().id == id).then_some(item)
    }

    #[test]
    fn load_resolves_until_the_slot_turns_over() {
        let domain = Domain::<1>::new();
        // SAFETY: the only `Local` naming CPU 0 in this domain, and it never
        // leaves this thread.
        let local = unsafe { Local::new(&domain, 0) };
        // SAFETY: never active, so not inside a read section.
        unsafe { local.exit_idle() };

        let slot = Atomic::<Item>::null();
        let first = Item::new(1);

        // SAFETY: freshly allocated, and only ever freed by retiring it to
        // `domain` below, after it is unlinked.
        unsafe { slot.store(Some(first), Ordering::Release) };

        local.read(|g| {
            assert_eq!(lookup(&slot, 1, g).map(Shared::as_ptr), Some(first));
            assert!(lookup(&slot, 2, g).is_none(), "identity must be checked");
        });

        // `Shared` cannot leave the read section, so take the raw pointer while
        // still inside it.
        let unlinked = local.read(|g| {
            // SAFETY: as above.
            unsafe { slot.swap(Some(Item::new(2)), Ordering::AcqRel, g) }.map(Shared::as_ptr)
        });
        assert_eq!(unlinked, Some(first));

        local.read(|g| {
            assert!(
                lookup(&slot, 1, g).is_none(),
                "a stale id must not resolve to the new tenant"
            );
            assert!(lookup(&slot, 2, g).is_some());
        });

        // SAFETY: unlinked above, so unreachable to any future reader, and
        // retired exactly once.
        unsafe { local.retire(unlinked.unwrap()) };

        // The first reclaim only rotates the batch and opens its epoch.
        assert_eq!(local.reclaim(usize::MAX), 0);
        assert_eq!(DROPS.load(Ordering::Relaxed), 0, "grace period not elapsed");

        // SAFETY: not inside a read section.
        unsafe { local.quiescent() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(DROPS.load(Ordering::Relaxed), 1);

        // Anything left in the slot leaks, which fails miri's leak check.
        let last = local.read(|g| {
            // SAFETY: as above.
            unsafe { slot.swap(None, Ordering::AcqRel, g) }.map(Shared::as_ptr)
        });

        // SAFETY: unlinked above, so unreachable to any future reader, and
        // retired exactly once.
        unsafe { local.retire(last.unwrap()) };

        assert_eq!(local.reclaim(usize::MAX), 0, "rotates the batch");
        // SAFETY: not inside a read section.
        unsafe { local.quiescent() };
        assert_eq!(local.reclaim(usize::MAX), 1);
        assert_eq!(DROPS.load(Ordering::Relaxed), 2);
    }
}
