// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::{abort_on_dtor_unwind, destructors};
use core::cell::UnsafeCell;
use core::hint::unreachable_unchecked;
use core::ptr;

#[expect(clippy::missing_safety_doc, reason = "")]
pub unsafe trait DestroyedState: Sized {
    fn register_dtor<T>(s: &LazyStorage<T, Self>);
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl DestroyedState for ! {
    fn register_dtor<T>(_: &LazyStorage<T, !>) {}
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl DestroyedState for () {
    fn register_dtor<T>(s: &LazyStorage<T, ()>) {
        // Safety: this will only be called once
        unsafe {
            destructors::register(ptr::from_ref(s).cast_mut().cast(), destroy::<T>);
        }
    }
}

enum State<T, D> {
    Initial,
    Alive(T),
    Destroyed(D),
}

pub struct LazyStorage<T, D> {
    state: UnsafeCell<State<T, D>>,
}

impl<T, D> Default for LazyStorage<T, D>
where
    D: DestroyedState,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, D> LazyStorage<T, D>
where
    D: DestroyedState,
{
    pub const fn new() -> LazyStorage<T, D> {
        LazyStorage {
            state: UnsafeCell::new(State::Initial),
        }
    }

    #[inline]
    pub fn get_or_init(&self, i: Option<&mut Option<T>>, f: impl FnOnce() -> T) -> *const T {
        // Safety: memory location is always initialized
        let state = unsafe { &*self.state.get() };
        match state {
            State::Alive(v) => v,
            State::Destroyed(_) => ptr::null(),
            // Safety: thread local is not initialized yet
            State::Initial => unsafe { self.initialize(i, f) },
        }
    }

    #[cold]
    unsafe fn initialize(&self, i: Option<&mut Option<T>>, f: impl FnOnce() -> T) -> *const T {
        // Perform initialization

        let v = i.and_then(Option::take).unwrap_or_else(f);

        // Safety: memory location is always initialized
        let old = unsafe { self.state.get().replace(State::Alive(v)) };
        match old {
            // If the variable is not being recursively initialized, register
            // the destructor. This might be a noop if the value does not need
            // destruction.
            State::Initial => D::register_dtor(self),
            // Else, drop the old value. This might be changed to a panic.
            val => drop(val),
        }

        // SAFETY: the state was just set to `Alive`
        unsafe {
            let State::Alive(v) = &*self.state.get() else {
                unreachable_unchecked()
            };
            v
        }
    }
}

/// Transition an `Alive` TLS variable into the `Destroyed` state, dropping its
/// value.
///
/// # Safety
/// * Must only be called at thread destruction.
/// * `ptr` must point to an instance of `Storage<T, ()>` and be valid for
///   accessing that instance.
unsafe extern "C" fn destroy<T>(ptr: *mut u8) {
    // Print a nice abort message if a panic occurs.
    abort_on_dtor_unwind(|| {
        // Safety: it is up to caller to ensure `ptr` is valid
        let storage = unsafe { &*(ptr as *const LazyStorage<T, ()>) };
        // Update the state before running the destructor as it may attempt to
        // access the variable.
        // Safety: memory location is always initialized
        let val = unsafe { storage.state.get().replace(State::Destroyed(())) };
        drop(val);
    });
}
