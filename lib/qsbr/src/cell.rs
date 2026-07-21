// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::marker::PhantomData;
use core::ops::Deref;
use core::ptr;
use core::ptr::NonNull;

use crate::loom::sync::atomic::{AtomicPtr, Ordering};
use crate::{Guard, QsbrDomain, QsbrHead};

pub struct QsbrCell<T> {
    ptr: AtomicPtr<Inner<T>>,
    domain: &'static QsbrDomain,
    /// `*mut T` suppresses the (unconditional, too-permissive) structural
    /// auto-traits of `AtomicPtr`; the explicit impls below restore them
    /// with the correct bounds.
    _marker: PhantomData<*mut T>,
}

struct Inner<T> {
    value: T,
    head: QsbrHead,
}

unsafe impl<T: Send + Sync> Send for QsbrCell<T> {}
unsafe impl<T: Send + Sync> Sync for QsbrCell<T> {}

impl<T> Inner<T> {
    fn alloc(value: T) -> NonNull<Inner<T>> {
        let boxed = Box::new(Inner {
            head: QsbrHead::new(Self::drop_inner),
            value,
        });

        unsafe { NonNull::new_unchecked(Box::into_raw(boxed)) }
    }

    unsafe fn drop_inner(node: NonNull<QsbrHead>) {
        drop(unsafe { Box::from_raw(node.as_ptr().cast::<Self>()) });
    }
}

impl<T: Send + Sync + 'static> QsbrCell<T> {
    pub fn new(value: T, domain: &'static QsbrDomain) -> Self {
        Self {
            ptr: AtomicPtr::new(Inner::alloc(value).as_ptr()),
            domain,
            _marker: PhantomData,
        }
    }

    pub fn load<'g>(&self, guard: &'g Guard) -> Shared<'g, T> {
        Shared::new(
            unsafe { NonNull::new_unchecked(self.ptr.load(Ordering::Acquire)) },
            guard,
        )
    }

    pub fn store(&self, value: T) {
        let old = self
            .ptr
            .swap(Inner::alloc(value).as_ptr(), Ordering::AcqRel);

        // SAFETY: the swap unlinked `old` from the slot.
        unsafe { self.retire(old) };
    }

    pub fn compare_exchange<'g>(
        &self,
        current: Shared<'g, T>,
        value: T,
        guard: &'g Guard,
    ) -> Result<(), (Shared<'g, T>, T)> {
        let new = Inner::alloc(value);

        match self.ptr.compare_exchange(
            current.inner.as_ptr(),
            new.as_ptr(),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(old) => {
                // SAFETY: the successful exchange unlinked `old`.
                unsafe { self.retire(old) };
                Ok(())
            }
            Err(actual) => {
                // SAFETY: `new` was never published; we still own it
                // exclusively. Move the value back out (`Retired` has no
                // drop glue), freeing the header.
                let value = unsafe { Box::from_raw(new.as_ptr()) }.value;

                // SAFETY: the slot is never null.
                let actual = unsafe { NonNull::new_unchecked(actual) };

                Err((Shared::new(actual, guard), value))
            }
        }
    }

    unsafe fn retire(&self, old: *mut Inner<T>) {
        // SAFETY: `repr(C)` puts `retired` first; projection forms no
        // reference.
        let node = unsafe { NonNull::new_unchecked(ptr::addr_of_mut!((*old).head)) };
        // SAFETY: per this function's contract the header is valid and
        // unlinked from its only shared location; readers access the slot
        // exclusively under `Guard`s of this same domain (debug-checked at
        // every load); `Inner::drop_inner` is sound from any context
        // (`T: Send`) and runs once.
        unsafe { self.domain.retire(node) };
    }
}

impl<T> Drop for QsbrCell<T> {
    fn drop(&mut self) {
        // Readers on other CPUs may still hold `Shared`s into the final
        // value — which is exactly what retirement is for.
        let old = *self.ptr.get_mut();
        // SAFETY: `old` is the slot's (never-null) leaked `Box<Inner<T>>`,
        // unlinked by `&mut self` exclusivity; `repr(C)` puts `retired`
        // first. `drop_inner` was baked in by `Inner::alloc`, so no `T`
        // bounds are needed here.
        let node = unsafe { NonNull::new_unchecked(ptr::addr_of_mut!((*old).head)) };
        unsafe { self.domain.retire(node) };
    }
}

pub struct Shared<'g, T> {
    inner: NonNull<Inner<T>>,
    _guard: &'g Guard,
}

impl<T> Clone for Shared<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Shared<'_, T> {}

impl<'g, T> Shared<'g, T> {
    fn new(inner: NonNull<Inner<T>>, guard: &'g Guard) -> Self {
        Self {
            inner,
            _guard: guard,
        }
    }

    /// Borrows the value for the whole critical section (a longer borrow
    /// than `Deref` can express).
    ///
    /// Safe because reclamation of anything reachable through an
    /// [`Atomic`] flows exclusively through crate-internal retirement: the
    /// value cannot be freed before the CPU that produced the guard reports
    /// a quiescent state, which the driver contract places after this
    /// critical section ends.
    pub fn get(&self) -> &'g T {
        // SAFETY: see above.
        &unsafe { self.inner.as_ref() }.value
    }
}

impl<T> Deref for Shared<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.get()
    }
}
