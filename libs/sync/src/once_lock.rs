// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::Once;
use core::{
    cell::UnsafeCell,
    fmt,
    mem::MaybeUninit,
    panic::{RefUnwindSafe, UnwindSafe},
};

/// A synchronization primitive which can be written to only once.
///
/// Thread-safe variant of [`core::cell::OnceCell`].
pub struct OnceLock<T> {
    once: Once,
    data: UnsafeCell<MaybeUninit<T>>,
}

impl<T> OnceLock<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            once: Once::new(),
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// # Panics
    ///
    /// Panics if the closure panics.
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        self.once.call_once(|| {
            // SAFETY: `Once` ensures this is only called once
            unsafe {
                (*self.data.get()).as_mut_ptr().write(f());
            }
        });

        // SAFETY: `Once` ensures this is only called once
        unsafe { self.force_get() }
    }

    /// # Errors
    ///
    /// Returns an error if the given closure errors.
    ///
    /// # Panics
    ///
    /// Panics if the closure panics.
    pub fn get_or_try_init<F, E>(&self, f: F) -> Result<&T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let mut error = None;

        self.once.call_once(|| {
            #[allow(tail_expr_drop_order)]
            match f() {
                Ok(val) => {
                    // SAFETY: `Once` ensures this is only called once
                    unsafe {
                        (*self.data.get()).as_mut_ptr().write(val);
                    }
                }
                Err(err) => error = Some(err),
            }
        });

        #[allow(if_let_rescope)]
        if let Some(err) = error {
            Err(err)
        } else {
            // SAFETY: `Once` ensures this is only called once
            unsafe { Ok(self.force_get()) }
        }
    }

    pub fn get(&self) -> Option<&T> {
        self.once
            .is_completed()
            .then(|| unsafe { self.force_get() })
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.once
            .is_completed()
            .then(|| unsafe { self.force_get_mut() })
    }

    unsafe fn force_get(&self) -> &T {
        // SAFETY:
        // * `UnsafeCell`/inner deref: data never changes again
        // * `MaybeUninit`/outer deref: data was initialized
        unsafe { &*(*self.data.get()).as_ptr() }
    }

    unsafe fn force_get_mut(&mut self) -> &mut T {
        // SAFETY:
        // * `UnsafeCell`/inner deref: data never changes again
        // * `MaybeUninit`/outer deref: data was initialized
        unsafe { &mut *(*self.data.get()).as_mut_ptr() }
    }
}

unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

impl<T: RefUnwindSafe + UnwindSafe> RefUnwindSafe for OnceLock<T> {}
impl<T: UnwindSafe> UnwindSafe for OnceLock<T> {}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_tuple("OnceLock");
        match self.get() {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}
