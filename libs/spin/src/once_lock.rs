// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::mem::MaybeUninit;
use core::panic::{RefUnwindSafe, UnwindSafe};

use kutil::loom_const_fn;

use super::Once;
use crate::loom::cell::UnsafeCell;

/// A synchronization primitive which can be written to only once.
///
/// Thread-safe variant of [`core::cell::OnceCell`].
pub struct OnceLock<T> {
    once: Once,
    data: UnsafeCell<MaybeUninit<T>>,
}

impl<T> OnceLock<T> {
    loom_const_fn! {
        #[must_use]
        pub const fn new() -> Self {
            Self {
                once: Once::new(),
                data: UnsafeCell::new(MaybeUninit::uninit()),
            }
        }
    }

    /// # Panics
    ///
    /// Panics if the closure panics.
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        self.once.call_once(|| {
            self.data.with_mut(|data| {
                // SAFETY: `Once` ensures this is only called once
                unsafe { (*data).as_mut_ptr().write(f()) }
            });
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
            #[allow(tail_expr_drop_order, reason = "")]
            match f() {
                Ok(val) => {
                    self.data.with_mut(|data| {
                        // SAFETY: `Once` ensures this is only called once
                        unsafe { (*data).as_mut_ptr().write(val) }
                    });
                }
                Err(err) => error = Some(err),
            }
        });

        #[allow(if_let_rescope, reason = "")]
        if let Some(err) = error {
            Err(err)
        } else {
            // SAFETY: `Once` ensures this is only called once
            unsafe { Ok(self.force_get()) }
        }
    }

    /// Gets the reference to the underlying value.
    ///
    /// Returns `None` if the cell is empty, or being initialized. This
    /// method never blocks.
    pub fn get(&self) -> Option<&T> {
        self.once.is_completed().then(|| {
            // Safety: `is_completed` ensures value is properly initialized
            unsafe { self.force_get() }
        })
    }

    /// Gets the mutable reference to the underlying value.
    ///
    /// Returns `None` if the cell is empty. This method never blocks.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.once.is_completed().then(|| {
            // Safety: `is_completed` ensures value is properly initialized
            unsafe { self.force_get_mut() }
        })
    }

    /// Blocks the current thread until the cell is initialized.
    pub fn wait(&self) -> &T {
        self.once.wait();

        // Safety: we have waited until the data is initialized
        unsafe { self.force_get() }
    }

    /// Sets the contents of this cell to `value`.
    ///
    /// May block if another thread is currently attempting to initialize the cell. The cell is
    /// guaranteed to contain a value when set returns, though not necessarily the one provided.
    ///
    /// Returns `Ok(())` if the cell's value was set by this call.
    ///
    /// # Errors
    ///
    /// Returns the value in the `Err` variant is the cell was full.
    #[inline]
    pub fn set(&self, value: T) -> Result<(), T> {
        match self.try_insert(value) {
            Ok(_) => Ok(()),
            Err((_, value)) => Err(value),
        }
    }

    /// Sets the contents of this cell to `value` if the cell was empty, then
    /// returns a reference to it.
    ///
    /// May block if another thread is currently attempting to initialize the cell. The cell is
    /// guaranteed to contain a value when set returns, though not necessarily the one provided.
    ///
    /// Returns `Ok(&value)` if the cell was empty
    ///
    /// # Errors
    ///
    /// Returns `Err(&current_value, value)` if the cell was full.
    #[inline]
    pub fn try_insert(&self, value: T) -> Result<&T, (&T, T)> {
        let mut value = Some(value);
        let res = self.get_or_init(|| {
            // Safety: we have initialized `value` to a `Some` above
            unsafe { value.take().unwrap_unchecked() }
        });
        match value {
            None => Ok(res),
            Some(value) => Err((res, value)),
        }
    }

    /// Takes the value out of this `OnceLock`, moving it back to an uninitialized state.
    ///
    /// Has no effect and returns `None` if the `OnceLock` hasn't been initialized.
    ///
    /// Safety is guaranteed by requiring a mutable reference.
    #[inline]
    pub fn take(&mut self) -> Option<T> {
        if self.is_initialized() {
            self.once = Once::new();
            self.data.with(|data| {
                // SAFETY: `self.value` is initialized and contains a valid `T`.
                // `self.once` is reset, so `is_initialized()` will be false again
                // which prevents the value from being read twice.
                Some(unsafe { MaybeUninit::assume_init_read(&*data) })
            })
        } else {
            None
        }
    }

    /// Consumes the `OnceLock`, returning the wrapped value. Returns
    /// `None` if the cell was empty.
    #[inline]
    pub fn into_inner(mut self) -> Option<T> {
        self.take()
    }

    #[inline]
    fn is_initialized(&self) -> bool {
        self.once.is_completed()
    }

    unsafe fn force_get(&self) -> &T {
        self.data.with(|data| {
            // SAFETY:
            // * `UnsafeCell`/inner deref: data never changes again
            // * `MaybeUninit`/outer deref: data was initialized
            unsafe { &*(*data).as_ptr() }
        })
    }

    unsafe fn force_get_mut(&mut self) -> &mut T {
        self.data.with_mut(|data| {
            // SAFETY:
            // * `UnsafeCell`/inner deref: data never changes again
            // * `MaybeUninit`/outer deref: data was initialized
            unsafe { &mut *(*data).as_mut_ptr() }
        })
    }
}

// Safety: synchronization primitive
unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
// Safety: synchronization primitive
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

impl<T: Clone> Clone for OnceLock<T> {
    #[inline]
    fn clone(&self) -> OnceLock<T> {
        let cell = Self::new();
        if let Some(value) = self.get() {
            match cell.set(value.clone()) {
                Ok(()) => (),
                Err(_) => unreachable!(),
            }
        }
        cell
    }
}

impl<T> From<T> for OnceLock<T> {
    /// Creates a new cell with its contents set to `value`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::OnceLock;
    ///
    /// # fn main() -> Result<(), i32> {
    /// let a = OnceLock::from(3);
    /// let b = OnceLock::new();
    /// b.set(3)?;
    /// assert_eq!(a, b);
    /// Ok(())
    /// # }
    /// ```
    #[inline]
    fn from(value: T) -> Self {
        let cell = Self::new();
        match cell.set(value) {
            Ok(()) => cell,
            Err(_) => unreachable!(),
        }
    }
}

impl<T: PartialEq> PartialEq for OnceLock<T> {
    /// Equality for two `OnceLock`s.
    ///
    /// Two `OnceLock`s are equal if they either both contain values and their
    /// values are equal, or if neither contains a value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::OnceLock;
    ///
    /// let five = OnceLock::new();
    /// five.set(5).unwrap();
    ///
    /// let also_five = OnceLock::new();
    /// also_five.set(5).unwrap();
    ///
    /// assert!(five == also_five);
    ///
    /// assert!(OnceLock::<u32>::new() == OnceLock::<u32>::new());
    /// ```
    #[inline]
    fn eq(&self, other: &OnceLock<T>) -> bool {
        self.get() == other.get()
    }
}

impl<T: Eq> Eq for OnceLock<T> {}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<#[may_dangle] T> Drop for OnceLock<T> {
    #[inline]
    fn drop(&mut self) {
        if self.is_initialized() {
            self.data.with_mut(|data| {
                // SAFETY: The cell is initialized and being dropped, so it can't
                // be accessed again. We also don't touch the `T` other than
                // dropping it, which validates our usage of #[may_dangle].
                unsafe { MaybeUninit::assume_init_drop(&mut *data) }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom;
    use crate::loom::sync::atomic::{AtomicUsize, Ordering};
    use crate::loom::thread;

    fn spawn_and_wait<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        thread::spawn(f).join().unwrap()
    }

    #[test]
    fn sync_once_cell() {
        loom::model(|| {
            crate::loom::lazy_static! {
                static ref ONCE_CELL: OnceLock<i32> = OnceLock::new();
            }

            assert!(ONCE_CELL.get().is_none());

            spawn_and_wait(|| {
                ONCE_CELL.get_or_init(|| 92);
                assert_eq!(ONCE_CELL.get(), Some(&92));
            });

            ONCE_CELL.get_or_init(|| panic!("Kaboom!"));
            assert_eq!(ONCE_CELL.get(), Some(&92));
        })
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn sync_once_cell_get_mut() {
        let mut c = OnceLock::new();
        assert!(c.get_mut().is_none());
        c.set(90).unwrap();
        *c.get_mut().unwrap() += 2;
        assert_eq!(c.get_mut(), Some(&mut 92));
    }

    #[test]
    fn sync_once_cell_drop() {
        loom::model(|| {
            crate::loom::lazy_static! {
                static ref DROP_CNT: AtomicUsize = AtomicUsize::new(0);
            }
            struct Dropper;
            impl Drop for Dropper {
                fn drop(&mut self) {
                    DROP_CNT.fetch_add(1, Ordering::SeqCst);
                }
            }

            let x = OnceLock::new();
            spawn_and_wait(move || {
                x.get_or_init(|| Dropper);
                assert_eq!(DROP_CNT.load(Ordering::SeqCst), 0);
                drop(x);
            });

            assert_eq!(DROP_CNT.load(Ordering::SeqCst), 1);
        })
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn sync_once_cell_drop_empty() {
        let x = OnceLock::<String>::new();
        drop(x);
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn clone() {
        let s = OnceLock::new();
        let c = s.clone();
        assert!(c.get().is_none());

        s.set("hello".to_string()).unwrap();
        let c = s.clone();
        assert_eq!(c.get().map(String::as_str), Some("hello"));
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn from_impl() {
        assert_eq!(OnceLock::from("value").get(), Some(&"value"));
        assert_ne!(OnceLock::from("foo").get(), Some(&"bar"));
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn partialeq_impl() {
        assert!(OnceLock::from("value") == OnceLock::from("value"));
        assert!(OnceLock::from("foo") != OnceLock::from("bar"));

        assert!(OnceLock::<String>::new() == OnceLock::new());
        assert!(OnceLock::<String>::new() != OnceLock::from("value".to_owned()));
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn into_inner() {
        let cell: OnceLock<String> = OnceLock::new();
        assert_eq!(cell.into_inner(), None);
        let cell = OnceLock::new();
        cell.set("hello".to_string()).unwrap();
        assert_eq!(cell.into_inner(), Some("hello".to_string()));
    }

    #[test]
    fn is_sync_send() {
        fn assert_traits<T: Send + Sync>() {}
        assert_traits::<OnceLock<String>>();
    }

    #[test]
    fn eval_once_macro() {
        loom::model(|| {
            macro_rules! eval_once {
                (|| -> $ty:ty {
                    $($body:tt)*
                }) => {{
                    $crate::loom::lazy_static! {
                        static ref ONCE_CELL: OnceLock<$ty> = OnceLock::new();
                    }
                    fn init() -> $ty {
                        $($body)*
                    }
                    ONCE_CELL.get_or_init(init)
                }};
            }

            let fib: &'static Vec<i32> = eval_once! {
                || -> Vec<i32> {
                    let mut res = vec![1, 1];
                    for i in 0..10 {
                        let next = res[i] + res[i + 1];
                        res.push(next);
                    }
                    res
                }
            };
            assert_eq!(fib[5], 8)
        })
    }

    #[test]
    fn dropck() {
        loom::model(|| {
            let cell = OnceLock::new();
            {
                let s = String::new();
                cell.set(&s).unwrap();
            }
        })
    }
}
