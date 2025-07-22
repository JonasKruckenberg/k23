// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::{fmt, ptr};

use util::loom_const_fn;

use super::Once;
use super::once::ExclusiveState;
use crate::loom::UnsafeCell;

union Data<T, F> {
    value: ManuallyDrop<T>,
    f: ManuallyDrop<F>,
}

/// A synchronization primitive which initializes its value on first access.
///
/// This is a spin-based port of the [`std::sync::LazyLock`](https://doc.rust-lang.org/std/sync/struct.LazyLock.html) type.
pub struct LazyLock<T, F = fn() -> T> {
    once: Once,
    data: UnsafeCell<Data<T, F>>,
}

impl<T, F: FnOnce() -> T> LazyLock<T, F> {
    loom_const_fn! {
        pub const fn new(f: F) -> Self {
            Self {
                once: Once::new(),
                data: UnsafeCell::new(Data {
                    f: ManuallyDrop::new(f),
                }),
            }
        }
    }

    /// # Errors
    ///
    /// Returns the initialization closure as the error in case the lock is not yet initialized.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned.
    pub fn into_inner(mut this: Self) -> Result<T, F> {
        let state = this.once.state();
        match state {
            ExclusiveState::Poisoned => panic!("LazyLock instance has previously been poisoned"),
            state => {
                let this = ManuallyDrop::new(this);
                // Safety: constructor ensures this.data is initialized
                let data = unsafe { ptr::read(&this.data) }.into_inner();
                match state {
                    // Safety: complete means self.data contains the init function, the other code
                    // upholds this
                    ExclusiveState::Incomplete => Err(ManuallyDrop::into_inner(unsafe { data.f })),
                    // Safety: complete means self.data contains the data, the other code upholds this
                    ExclusiveState::Complete => Ok(ManuallyDrop::into_inner(unsafe { data.value })),
                    ExclusiveState::Poisoned => unreachable!(),
                }
            }
        }
    }

    #[inline]
    pub fn force(this: &LazyLock<T, F>) -> &T {
        this.once.call_once(|| {
            let data = this.data.with_mut(|data| {
                // SAFETY: `call_once` only runs this closure once, ever.
                unsafe { &mut *data }
            });

            // Safety: `call_once` ensures that data contains the init function
            let f = unsafe { ManuallyDrop::take(&mut data.f) };
            let value = f();
            data.value = ManuallyDrop::new(value);
        });

        this.data.with(|data| {
            // Safety: the above infallibly initialized the value
            unsafe { &(*data).value }
        })
    }
}

impl<T, F> LazyLock<T, F> {
    /// Get the inner value if it has already been initialized.
    fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            self.data.with(|data| {
                // Safety: The closure has been run successfully, so `value` has been initialized
                // and will not be modified again.
                Some(unsafe { &*(*data).value })
            })
        } else {
            None
        }
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyLock<T, F> {
    type Target = T;

    /// Dereferences the value.
    ///
    /// This method will block the calling thread if another initialization
    /// routine is currently running.
    ///
    #[inline]
    fn deref(&self) -> &T {
        LazyLock::force(self)
    }
}

impl<T, F> Drop for LazyLock<T, F> {
    fn drop(&mut self) {
        match self.once.state() {
            ExclusiveState::Incomplete => {
                self.data.with_mut(|data| {
                    // Safety: complete means self.data still contains the init function, the other code
                    // upholds this
                    unsafe { ManuallyDrop::drop(&mut (*data).f) }
                });
            }
            ExclusiveState::Complete => {
                self.data.with_mut(|data| {
                    // Safety: complete means self.data still contains the init function, the other code
                    // upholds this
                    unsafe { ManuallyDrop::drop(&mut (*data).value) }
                });
            }
            ExclusiveState::Poisoned => {}
        }
    }
}

impl<T: Default> Default for LazyLock<T> {
    /// Creates a new lazy value using `Default` as the initializing function.
    #[inline]
    fn default() -> LazyLock<T> {
        LazyLock::new(T::default)
    }
}

impl<T: fmt::Debug, F> fmt::Debug for LazyLock<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_tuple("LazyLock");
        match self.get() {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}

// Safety: synchronization primitive
unsafe impl<T: Sync + Send, F: Send> Sync for LazyLock<T, F> {}

impl<T: RefUnwindSafe + UnwindSafe, F: UnwindSafe> RefUnwindSafe for LazyLock<T, F> {}
impl<T: UnwindSafe, F: UnwindSafe> UnwindSafe for LazyLock<T, F> {}

#[cfg(test)]
mod tests {
    use std::cell::LazyCell;

    use super::*;
    use crate::loom::{AtomicUsize, Ordering, thread};
    use crate::{Mutex, OnceLock};

    fn spawn_and_wait<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        thread::spawn(f).join().unwrap()
    }

    #[test]
    fn lazy_default() {
        static CALLED: AtomicUsize = AtomicUsize::new(0);

        struct Foo(u8);
        impl Default for Foo {
            fn default() -> Self {
                CALLED.fetch_add(1, Ordering::SeqCst);
                Foo(42)
            }
        }

        let lazy: LazyCell<Mutex<Foo>> = <_>::default();

        assert_eq!(CALLED.load(Ordering::SeqCst), 0);

        assert_eq!(lazy.lock().0, 42);
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);

        lazy.lock().0 = 21;

        assert_eq!(lazy.lock().0, 21);
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sync_lazy_new() {
        static CALLED: AtomicUsize = AtomicUsize::new(0);
        static SYNC_LAZY: LazyLock<i32> = LazyLock::new(|| {
            CALLED.fetch_add(1, Ordering::SeqCst);
            92
        });

        assert_eq!(CALLED.load(Ordering::SeqCst), 0);

        spawn_and_wait(|| {
            let y = *SYNC_LAZY - 30;
            assert_eq!(y, 62);
            assert_eq!(CALLED.load(Ordering::SeqCst), 1);
        });

        let y = *SYNC_LAZY - 30;
        assert_eq!(y, 62);
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sync_lazy_default() {
        static CALLED: AtomicUsize = AtomicUsize::new(0);

        struct Foo(u8);
        impl Default for Foo {
            fn default() -> Self {
                CALLED.fetch_add(1, Ordering::SeqCst);
                Foo(42)
            }
        }

        let lazy: LazyLock<Mutex<Foo>> = <_>::default();

        assert_eq!(CALLED.load(Ordering::SeqCst), 0);

        assert_eq!(lazy.lock().0, 42);
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);

        lazy.lock().0 = 21;

        assert_eq!(lazy.lock().0, 21);
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn static_sync_lazy() {
        static XS: LazyLock<Vec<i32>> = LazyLock::new(|| {
            let mut xs = Vec::new();
            xs.push(1);
            xs.push(2);
            xs.push(3);
            xs
        });

        spawn_and_wait(|| {
            assert_eq!(&*XS, &vec![1, 2, 3]);
        });

        assert_eq!(&*XS, &vec![1, 2, 3]);
    }

    #[test]
    fn static_sync_lazy_via_fn() {
        fn xs() -> &'static Vec<i32> {
            static XS: OnceLock<Vec<i32>> = OnceLock::new();
            XS.get_or_init(|| {
                let mut xs = Vec::new();
                xs.push(1);
                xs.push(2);
                xs.push(3);
                xs
            })
        }
        assert_eq!(xs(), &vec![1, 2, 3]);
    }
}
