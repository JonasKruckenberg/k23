#![no_std]
#![feature(thread_local)]

#[macro_export]
macro_rules! declare_thread_local {
    // empty (base case for the recursion)
    () => {};

    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty; $($rest:tt)*) => (
        $crate::declare_thread_local_inner!($(#[$attr])* $vis $name, $t);
        $crate::declare_thread_local!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty) => (
        $crate::declare_thread_local_inner!($(#[$attr])* $vis $name, $t);
    );

    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block; $($rest:tt)*) => (
        $crate::declare_thread_local_inner!($(#[$attr])* $vis $name, $t, const $init);
        $crate::declare_thread_local!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block) => (
        $crate::declare_thread_local_inner!($(#[$attr])* $vis $name, $t, const $init);
    );

    // process multiple declarations
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => (
        $crate::declare_thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
        $crate::declare_thread_local!($($rest)*);
    );

    // handle a single declaration
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => (
        $crate::declare_thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
    );
}

#[doc(hidden)]
#[macro_export]
macro_rules! declare_thread_local_inner {
    // used to generate the `LocalKey` value for const-initialized thread locals
    (@key $name:ident, $t:ty, const $init:expr) => {{
        #[inline]
        unsafe fn __getit(_init: Option<&mut Option<$t>>) -> Option<&'static $t> {
            #[allow(clippy::declare_interior_mutable_const)] // TODO figore out
            const INIT_EXPR: $t = $init;

            #[thread_local]
            #[cfg_attr(debug_assertions, no_mangle)]
            // Use a mutable static to prevent the compiler from placing the init expression into tdata
            // even when its all zeros.
            static mut $name: $t = INIT_EXPR;

            Some(&*::core::ptr::addr_of!($name))
        }

        $crate::LocalKey::new(__getit)
    }};
    // used to generate the `LocalKey` value for lazily-initialized thread locals
    (@key $name:ident, $t:ty, $init:expr) => {{
        #[inline]
        fn __init() -> $t { $init }

        #[inline]
        unsafe fn __getit(init: Option<&mut Option<$t>>) -> Option<&'static $t> {
            #[thread_local]
            #[cfg_attr(debug_assertions, no_mangle)]
            static $name: ::core::cell::UnsafeCell<Option<$t>> = ::core::cell::UnsafeCell::new(None);
            let ptr = $name.get();

            if (&*ptr).is_none() {
                let value = init.map(|inner| inner.take()).unwrap_or_else(|| Some(__init()));
                let _ = ::core::mem::replace(&mut *ptr, value);
            }

            match *ptr {
                Some(ref x) => Some(x),
                None => ::core::hint::unreachable_unchecked(),
            }
        }

        $crate::LocalKey::new(__getit)
    }};
    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $($init:tt)*) => {
        $(#[$attr])* $vis const $name: $crate::LocalKey<$t> =
            $crate::declare_thread_local_inner!(@key $name, $t, $($init)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty) => {
        $(#[$attr])* $vis const $name: $crate::LocalKey<$t> =
            $crate::declare_thread_local_inner!(@key $name, $t, panic!("Thread Local Storage value is not initialized"));
    }
}

pub struct LocalKey<T: 'static> {
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
    inner: unsafe fn(Option<&mut Option<T>>) -> Option<&'static T>,
}

impl<T: 'static> LocalKey<T> {
    #[doc(hidden)]
    pub const fn new(inner: unsafe fn(Option<&mut Option<T>>) -> Option<&'static T>) -> Self {
        Self { inner }
    }

    /// Access the value in this thread-local storage.
    ///
    /// # Panics
    ///
    /// This method panics if the thread local storage value is accessed during or after destruction.
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.try_with(f).expect(
            "cannot access a Thread Local Storage value \
             during or after destruction",
        )
    }

    pub fn try_with<F, R>(&'static self, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R,
    {
        let value = unsafe { (self.inner)(None)? };
        let result = f(value);
        Some(result)
    }

    /// Initializes the thread-local storage with the provided value and then calls the given closure.
    ///
    /// # Panics
    ///
    /// This method panics if the thread local storage value is accessed during or after destruction.
    pub fn initialize_with<F, R>(&'static self, init: T, f: F) -> R
    where
        F: FnOnce(Option<T>, &T) -> R,
    {
        unsafe {
            let mut init = Some(init);
            let reference = (self.inner)(Some(&mut init)).expect(
                "cannot access a Thread Local Storage value \
                 during or after destruction",
            );
            f(init, reference)
        }
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
        let value = unsafe {
            (self.inner)(None).expect(
                "cannot access a Thread Local Storage value \
             during or after destruction",
            )
        };
        core::ptr::from_ref::<T>(value)
    }
}
