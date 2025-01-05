#![no_std]
#![feature(never_type)]
#![feature(thread_local)]

extern crate alloc;

mod eager;
mod lazy;
pub mod destructors;

use core::cell::{Cell, RefCell};
use core::fmt;
use cfg_if::cfg_if;

#[doc(hidden)]
pub use lazy::LazyStorage;
#[doc(hidden)]
pub use eager::EagerStorage;

#[macro_export]
macro_rules! thread_local {
    // empty (base case for the recursion)
    () => {};

    // declarations with constant initializers
    // process multiple declarations
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block; $($rest:tt)*) => (
        $crate::thread_local_inner!($(#[$attr])* $vis $name, $t, const $init);
        $crate::thread_local!($($rest)*);
    );
    // handle a single declaration
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block) => (
        $crate::thread_local_inner!($(#[$attr])* $vis $name, $t, const $init);
    );

    // declarations with regular initializers
    // process multiple declarations
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => (
        $crate::thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
        $crate::thread_local!($($rest)*);
    );
    // handle a single declaration
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => (
        $crate::thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
    );
}

#[doc(hidden)]
#[macro_export]
macro_rules! thread_local_inner {
    // Used to generate the `LocalKey` value for const-initialized thread locals.
    (@key $t:ty, const $init:expr) => {{
        const __INIT: $t = $init;

        unsafe {
            $crate::LocalKey::new(const {
                if ::core::mem::needs_drop::<$t>() {
                    |_| {
                        #[thread_local]
                        static VAL: $crate::EagerStorage<$t>
                            = $crate::EagerStorage::new(__INIT);
                        VAL.get()
                    }
                } else {
                    |_| {
                        #[thread_local]
                        static VAL: $t = __INIT;
                        &VAL
                    }
                }
            })
        }
    }};
    // Used to generate the `LocalKey` value for regular thread locals.
    (@key $t:ty, $init:expr) => {{
        #[inline]
        fn __init() -> $t {
            $init
        }

        unsafe {
            $crate::LocalKey::new(const {
                if ::core::mem::needs_drop::<$t>() {
                    |init| {
                        #[thread_local]
                        static VAL: $crate::LazyStorage<$t, ()>
                            = $crate::LazyStorage::new();
                        VAL.get_or_init(init, __init)
                    }
                } else {
                    |init| {
                        #[thread_local]
                        static VAL: $crate::LazyStorage<$t, !>
                            = $crate::LazyStorage::new();
                        VAL.get_or_init(init, __init)
                    }
                }
            })
        }
    }};

    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $($init:tt)*) => {
        $(#[$attr])* $vis const $name: $crate::LocalKey<$t> =
        $crate::thread_local_inner!(@key $t, $($init)*);
    };
}

#[non_exhaustive]
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AccessError;

impl fmt::Debug for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessError").finish()
    }
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("already destroyed", f)
    }
}

impl core::error::Error for AccessError {}

pub struct LocalKey<T> {
    // This outer `LocalKey<T>` type is what's going to be stored in statics,
    // but actual data inside will sometimes be tagged with #[thread_local].
    // It's not valid for a true static to reference a #[thread_local] static,
    // so we get around that by exposing an accessor through a layer of function
    // indirection (this thunk).
    //
    // Note that the thunk is itself unsafe because the returned lifetime of the
    // slot where data lives, `'static`, is not actually valid. The lifetime
    // here is actually slightly shorter than the currently running thread!
    //
    // Although this is an extra layer of indirection, it should in theory be
    // trivially devirtualizable by LLVM because the value of `inner` never
    // changes and the constant should be readonly within a crate. This mainly
    // only runs into problems when TLS statics are exported across crates.
    inner: fn(Option<&mut Option<T>>) -> *const T,
}

impl<T: 'static> fmt::Debug for LocalKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalKey").finish_non_exhaustive()
    }
}

impl<T: 'static> LocalKey<T> {
    #[doc(hidden)]
    pub const unsafe fn new(inner: fn(Option<&mut Option<T>>) -> *const T) -> LocalKey<T> {
        LocalKey { inner }
    }

    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.try_with(f).expect(
            "cannot access a Thread Local Storage value \
             during or after destruction",
        )
    }

    pub fn try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let thread_local = unsafe { (self.inner)(None).as_ref().ok_or(AccessError)? };
        Ok(f(thread_local))
    }

    /// Returns a raw pointer to the underlying thread-local data.
    ///
    /// # Panics
    ///
    /// This method panics if the thread local storage value is accessed during or after destruction.
    ///
    /// # Safety
    ///
    /// This attempts to retrieve a raw pointer to the underlying data. You should prefer to use the getter methods.
    #[must_use]
    pub unsafe fn as_ptr(&self) -> *const T {
        let value = unsafe { (self.inner)(None).as_ref().unwrap() };
        core::ptr::from_ref::<T>(value)
    }

    fn initialize_with<F, R>(&'static self, init: T, f: F) -> R
    where
        F: FnOnce(Option<T>, &T) -> R,
    {
        let mut init = Some(init);

        let reference = unsafe {
            (self.inner)(Some(&mut init)).as_ref().expect(
                "cannot access a Thread Local Storage value \
                 during or after destruction",
            )
        };

        f(init, reference)
    }
}

impl<T: 'static> LocalKey<Cell<T>> {
    pub fn set(&'static self, value: T) {
        self.initialize_with(Cell::new(value), |value, cell| {
            if let Some(value) = value {
                // The cell was already initialized, so `value` wasn't used to
                // initialize it. So we overwrite the current value with the
                // new one instead.
                cell.set(value.into_inner());
            }
        });
    }

    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(Cell::get)
    }

    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(Cell::take)
    }

    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

impl<T: 'static> LocalKey<RefCell<T>> {
    pub fn with_borrow<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|cell| f(&cell.borrow()))
    }

    pub fn with_borrow_mut<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        self.with(|cell| f(&mut cell.borrow_mut()))
    }

    pub fn set(&'static self, value: T) {
        self.initialize_with(RefCell::new(value), |value, cell| {
            if let Some(value) = value {
                // The cell was already initialized, so `value` wasn't used to
                // initialize it. So we overwrite the current value with the
                // new one instead.
                *cell.borrow_mut() = value.into_inner();
            }
        });
    }

    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(RefCell::take)
    }

    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

/// Run a callback in a scenario which must not unwind (such as a `extern "C"
/// fn` declared in a user crate). If the callback unwinds anyway, then
/// `rtabort` with a message about thread local panicking on drop.
#[inline]
#[allow(dead_code)]
fn abort_on_dtor_unwind(f: impl FnOnce()) {
    // Using a guard like this is lower cost.
    let guard = DtorUnwindGuard;
    f();
    core::mem::forget(guard);

    struct DtorUnwindGuard;
    impl Drop for DtorUnwindGuard {
        #[inline]
        fn drop(&mut self) {
            // This is not terribly descriptive, but it doesn't need to be as we'll
            // already have printed a panic message at this point.
            log::error!("thread local panicked on drop");
            abort();
        }
    }
}

pub(crate) fn abort() -> ! {
    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            compile_error!("unsupported target architecture")
        }
    }
}