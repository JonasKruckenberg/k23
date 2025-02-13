// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::{abort_on_dtor_unwind, destructors};
use core::cell::{Cell, UnsafeCell};
use core::ptr;

#[derive(Clone, Copy)]
enum State {
    Initial,
    Alive,
    Destroyed,
}

pub struct EagerStorage<T> {
    state: Cell<State>,
    val: UnsafeCell<T>,
}

impl<T> EagerStorage<T> {
    pub const fn new(val: T) -> EagerStorage<T> {
        EagerStorage {
            state: Cell::new(State::Initial),
            val: UnsafeCell::new(val),
        }
    }

    /// Gets a pointer to the TLS value. If the TLS variable has been destroyed,
    /// a null pointer is returned.
    ///
    /// The resulting pointer may not be used after thread destruction has
    /// occurred.
    ///
    /// # Safety
    /// The `self` reference must remain valid until the TLS destructor is run.
    #[inline]
    pub fn get(&self) -> *const T {
        match self.state.get() {
            State::Alive => self.val.get(),
            State::Destroyed => ptr::null(),
            // Safety: thread local is not initialized yet
            State::Initial => unsafe { self.initialize() },
        }
    }

    #[cold]
    unsafe fn initialize(&self) -> *const T {
        // Register the destructor

        // SAFETY:
        // The caller guarantees that `self` will be valid until thread destruction.
        unsafe {
            destructors::register(ptr::from_ref(self).cast_mut().cast(), destroy::<T>);
        }

        self.state.set(State::Alive);
        self.val.get()
    }
}

/// Transition an `Alive` TLS variable into the `Destroyed` state, dropping its
/// value.
///
/// # Safety
/// * Must only be called at thread destruction.
/// * `ptr` must point to an instance of `Storage` with `Alive` state and be
///   valid for accessing that instance.
unsafe extern "C" fn destroy<T>(ptr: *mut u8) {
    // Print a nice abort message if a panic occurs.
    abort_on_dtor_unwind(|| {
        // Safety: caller has to ensure `ptr` is valid
        let storage = unsafe { &*(ptr as *const EagerStorage<T>) };
        // Update the state before running the destructor as it may attempt to
        // access the variable.
        storage.state.set(State::Destroyed);

        // Safety: destroy is only called during destruction and the rest of the crate ensures
        // reads during or after destruction fail
        unsafe {
            ptr::drop_in_place(storage.val.get());
        }
    });
}
