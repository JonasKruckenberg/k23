//! `no_std` per-CPU storage primitive.
//!
//! The `cpu_local!` macro is essentially just a convenience wrapper around the (nightly only)
//! [`#[thread_local]`][thread_local_attr] attribute, which translates to LLVMs `thread_local` attribute.
//!
//! # Comparison with `thread_local`
//!
//! `cpu_local` mirrors the API of `thread_local` and uses the same compiler internals, but has one big conceptual difference:
//! `thread_local` runs the TLS values `Drop` impls when e.g. the thread gets torn down, while `cpu_local` doesn't.
//! This is usually fine for `no_std` environments where a "thread" is the same as a physical CPU, and
//! means `cpu_local` is vastly simpler, doesn't depend on `alloc` and has a panic-free core API.
//!
//! # Why no mutable access?
//!
//! While CPU-local values are safe from race-conditions, they still allow you to obtain multiple
//! references to the same data from different places on the call stack. Which could still allow
//! you to obtain multiple mutable references to the same value at the same time, a big no-no!
//!
//! Just like regular statics, you can use [`Cell`] or [`RefCell`] to work around this limitation and
//! [`LocalKey`] even provides convenience methods for those two containers.
//!
//! # `const` initializers
//!
//! The default `cpu_local!` declaration will lazily initialize the storage on first access.
//! Types which have `const` constructors however, can opt into a more optimized representation by
//! using a `const {}` block in the declaration:
//!
//! ```
//! #![feature(thread_local)]
//! # use std::cell::Cell;
//! use cpu_local::cpu_local;
//!
//!  // the default declaration works great
//! cpu_local! {
//!     static FOO: Cell<u32> = Cell::new(0);
//! }
//! // but `Cell::new` is const, so we can opt into the more optimized representation like so
//! cpu_local! {
//!     static BAR: Cell<u32> = const { Cell::new(0) };
//! }
//! ```
//!
//! # `no_std` support
//!
//! This crate supports `no_std` by default, but depending on the target you **need** to set up the machine
//! state (e.g. set the thread pointer correctly).
//!
//! **IF YOU DO NOT TO SET UP TLS CORRECTLY ALL CODE HERE HAS UNDEFINED BEHAVIOUR**.
//!
//! (The methods in this crate will likely attempt to access arbitrary memory locations)
//!
//! Correctly setting up the machine state for TLS greatly depends on the target you are compiling for,
//! but here is the rough outline for TLS support on RISC-V with the `"tls-model": "local-exec"` (which
//! is what the k23 kernel uses):
//!
//! LLVM will place all *non-zero initialized* `cpu_local` statics into a special `TLS` ELF segment.
//! The segments size on-disk without any zero-initialized statics is called its `file_size`, while the
//! size including all zero-initialized statics is called its `memory_size` (because that is how many
//! bytes the final segment will take up in memory). At boot time, you need to parse this data from
//! the ELF file and allocate `memory_size` chunks of zero-initialized memory for each CPU that you
//! wish to bring online. You then need to copy the TLS segments data into the first `file_size` bytes
//! of each chunk (tdata always comes before tbss). Finally you need to set the RISC-V thread pointer
//! `tp` to the beginning of the CPUs allocated TLS chunk.
//!
//! If you are unsure whether your `no_std` target supports TLS or what model it uses, chances are it
//! doesn't. In that case, you will need to define a [custom target specification] that does.
//!
//! [thread_local_attr]: <https://github.com/rust-lang/rust/issues/29594>
//! [custom target specification]: <https://doc.rust-lang.org/beta/rustc/targets/custom.html>

#![cfg_attr(not(test), no_std)]
#![feature(thread_local)]

use core::cell::{Cell, RefCell};
use core::ptr::NonNull;

/// Declare a new [cpu local] storage key.
///
/// The macro wraps any number of statics and makes them thread local.
///
/// # Example
///
/// ```rust
/// # #![feature(thread_local)]
///  use core::cell::{Cell, RefCell};
///  use cpu_local::cpu_local;
///
///  cpu_local! {
///     pub static FOO: Cell<u32> = Cell::new(1);
///
///     static BAR: RefCell<Vec<f32>> = RefCell::new(vec![1.0, 2.0]);
///  }
///
///  assert_eq!(FOO.get(), 1);
///  BAR.with_borrow(|v| assert_eq!(v[1], 2.0));
/// ```
///
/// Just like the stdlib's version this you can only obtain shared references (`&T`), so to modify
/// the CPU-local you will need an interior-mutability container such as [`Cell`] or [`RefCell`]
///
/// [cpu local]: crate#what-do-i-use-this-for
#[macro_export]
macro_rules! cpu_local {
    // empty (base case for the recursion)
    () => {};
    // declarations with constant initializers
    // process multiple declarations
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block; $($rest:tt)*) => (
        $crate::cpu_local_inner!($(#[$attr])* $vis $name, $t, const $init);
        $crate::cpu_local!($($rest)*);
    );
    // handle a single declaration
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block) => (
        $crate::cpu_local_inner!($(#[$attr])* $vis $name, $t, const $init);
    );

    // declarations with regular initializers
    // process multiple declarations
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => (
        $crate::cpu_local_inner!($(#[$attr])* $vis $name, $t, $init);
        $crate::cpu_local!($($rest)*);
    );
    // handle a single declaration
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => (
        $crate::cpu_local_inner!($(#[$attr])* $vis $name, $t, $init);
    );
}

#[doc(hidden)]
#[macro_export]
macro_rules! cpu_local_inner {
    // Used to generate the `LocalKey` value for const-initialized thread locals.
    // Note the explicit use of the expr_2021 specifier to distinguish between const and non-const
    // expressions since we have different implementations for them.
    (@key $t:ty, const $init:expr_2021) => {{
        // Safety: we correctly construct the TLS accessor below
        unsafe {
            $crate::LocalKey::new(const {
                |_| {
                    #[thread_local]
                    static VAL: $t = $init;
                    ::core::ptr::NonNull::from(&VAL)
                }
            })
        }
    }};

    // Used to generate the `LocalKey` value for regular thread locals.
    (@key $t:ty, $init:expr_2021) => {{
        #[inline]
        fn __init() -> $t {
            $init
        }

        // Safety: we correctly construct the TLS accessor below
        unsafe {
            $crate::LocalKey::new(const {
                |_| {
                    #[thread_local]
                    static VAL: ::core::cell::UnsafeCell<Option<$t>> = ::core::cell::UnsafeCell::new(None);

                    ::core::ptr::NonNull::from((*VAL.get()).get_or_insert_with(__init))
                }
            })
        }
    }};

    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $($init:tt)*) => {
        $(#[$attr])* $vis const $name: $crate::LocalKey<$t> =
        $crate::cpu_local_inner!(@key $t, $($init)*);
    };
}

/// A CPU local storage key which owns its contents.
///
/// It is instantiated with the [`cpu_local`] macro and in addition to the
/// primary [`with`] method provides a number of convenience methods
/// for working CPU-local [`Cell`]s and [`RefCell`]s.
///
/// The [`with`] method yields a reference to the contained value which cannot outlive the current thread or escape the given closure.
///
/// [`with`]: LocalKey::with
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
    //
    // This approach is copied from stdlib.
    inner: fn(Option<&mut Option<T>>) -> NonNull<T>,
}

impl<T> LocalKey<T> {
    /// Construct a new LocalKey from it's accessor function. DO NOT USE THIS DIRECTLY!
    #[doc(hidden)]
    pub const unsafe fn new(inner: fn(Option<&mut Option<T>>) -> NonNull<T>) -> Self {
        Self { inner }
    }

    /// Acquires a reference to the contained value.
    ///
    /// This will lazily initialize the value if necessary.
    ///
    /// # Example
    ///
    /// ```
    /// # #![feature(thread_local)]
    /// # use cpu_local::cpu_local;
    ///
    /// cpu_local! {
    ///     pub static STATIC: String = String::from("I am");
    /// }
    ///
    /// assert_eq!(
    ///     STATIC.with(|original_value| format!("{} initialized", original_value.as_str())),
    ///     "I am initialized",
    /// );
    /// ```
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // Safety: pointer is always valid
        let local = unsafe { (self.inner)(None).as_ref() };
        f(local)
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
    pub unsafe fn as_ptr(&self) -> *const T {
        // Safety: pointer is always valid
        let value = unsafe { (self.inner)(None).as_ref() };
        core::ptr::from_ref::<T>(value)
    }

    fn initialize_with<F, R>(&'static self, init: T, f: F) -> R
    where
        F: FnOnce(Option<T>, &T) -> R,
    {
        let mut init = Some(init);

        // Safety: pointer is always valid
        let reference = unsafe { (self.inner)(Some(&mut init)).as_ref() };

        f(init, reference)
    }
}

impl<T: 'static> LocalKey<Cell<T>> {
    /// Sets or initializes the contained value.
    ///
    /// Unlike the other methods, this will not run the lazy initializer of the thread local. Instead, it will be directly initialized with the given value if it wasn’t initialized yet.
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

    /// Returns a copy of the contained value.
    ///
    /// This will lazily initialize the value if necessary.
    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(Cell::get)
    }

    /// Takes the contained value, leaving [`Default::default`] in its place.
    ///
    /// This will lazily initialize the value if necessary.
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(Cell::take)
    }

    /// Replaces the contained value, returning the old value.
    ///
    /// This will lazily initialize the value if necessary.
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

impl<T: 'static> LocalKey<RefCell<T>> {
    /// Acquires a reference to the contained value.
    ///
    /// This will lazily initialize the value if necessary.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently mutably borrowed.
    pub fn with_borrow<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|cell| f(&cell.borrow()))
    }

    /// Acquires a mutable reference to the contained value.
    ///
    /// This will lazily initialize the value if necessary.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently borrowed.
    pub fn with_borrow_mut<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        self.with(|cell| f(&mut cell.borrow_mut()))
    }

    /// Sets or initializes the contained value.
    ///
    /// Unlike the other methods, this will not run the lazy initializer of the thread local. Instead, it will be directly initialized with the given value if it wasn’t initialized yet.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently borrowed.
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

    /// Takes the contained value, leaving [`Default::default`] in its place.
    ///
    /// This will lazily initialize the value if necessary.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently borrowed.
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(RefCell::take)
    }

    /// Replaces the contained value, returning the old value.
    ///
    /// This will lazily initialize the value if necessary.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently borrowed.
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    #[test]
    fn basically_works() {
        cpu_local! {
            static FOO: Cell<u32> = const { Cell::new(1) };
            static BAR: Cell<u32> = Cell::new(1);
        }

        assert_eq!(FOO.get(), 1);
        assert_eq!(FOO.replace(2), 1);
        assert_eq!(FOO.take(), 2);

        assert_eq!(BAR.get(), 1);
        assert_eq!(BAR.replace(2), 1);
        assert_eq!(BAR.take(), 2);
    }

    #[test]
    fn multi_thread() {
        cpu_local! {
            static FOO: Cell<u32> = const { Cell::new(1) };
            static BAR: Cell<u32> = Cell::new(1);
        }

        // run the same checks as above to verify the TLS still works
        std::thread::spawn(|| {
            assert_eq!(FOO.get(), 1);
            assert_eq!(FOO.replace(2), 1);
            assert_eq!(FOO.take(), 2);

            assert_eq!(BAR.get(), 1);
            assert_eq!(BAR.replace(2), 1);
            assert_eq!(BAR.take(), 2);
        });

        // and then a second time to ensure we get a fresh copy
        std::thread::spawn(|| {
            assert_eq!(FOO.get(), 1);
            assert_eq!(FOO.replace(2), 1);
            assert_eq!(FOO.take(), 2);

            assert_eq!(BAR.get(), 1);
            assert_eq!(BAR.replace(2), 1);
            assert_eq!(BAR.take(), 2);
        });
    }
}
