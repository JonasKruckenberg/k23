// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// #![allow(unused_imports,)]

cfg_if::cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
        pub(crate) use loom::cell::UnsafeCell;
        pub(crate) use loom::thread;
        pub(crate) use loom::model;
        pub(crate) use loom::sync::Arc;
    } else {
        pub(crate) use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        #[derive(Debug)]
        #[repr(transparent)]
        pub(crate) struct UnsafeCell<T: ?Sized>(core::cell::UnsafeCell<T>);

        impl<T> UnsafeCell<T> {
            #[expect(tail_expr_drop_order, reason = "")]
            pub const fn new(data: T) -> UnsafeCell<T> {
                UnsafeCell(core::cell::UnsafeCell::new(data))
            }
        }

        impl<T: ?Sized> UnsafeCell<T> {
            #[inline(always)]
            pub fn with<F, R>(&self, f: F) -> R
            where
                F: FnOnce(*const T) -> R,
            {
                f(self.0.get())
            }
            #[inline(always)]
            pub fn with_mut<F, R>(&self, f: F) -> R
            where
                F: FnOnce(*mut T) -> R,
            {
                f(self.0.get())
            }
        }
        impl<T> UnsafeCell<T> {
            #[inline(always)]
            #[must_use]
            pub(crate) fn into_inner(self) -> T {
                self.0.into_inner()
            }
        }

        #[derive(Debug)]
        #[repr(transparent)]
        pub(crate) struct AtomicU8(core::sync::atomic::AtomicU8);

        impl AtomicU8 {
            #[inline(always)]
            pub const fn new(val: u8) -> Self {
                Self(core::sync::atomic::AtomicU8::new(val))
            }
            #[inline(always)]
            pub fn load(&self, order: Ordering) -> u8 {
                self.0.load(order)
            }
            #[inline(always)]
            pub fn  store(& self, val: u8, order: Ordering) {
                self.0.store(val, order);
            }
            #[inline(always)]
            pub fn compare_exchange(& self, current: u8, new: u8, success: Ordering, failure: Ordering) -> Result<u8, u8> {
                self.0.compare_exchange(current, new,success, failure)
            }
            #[inline(always)]
            pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut u8) -> R) -> R {
                f(self.0.get_mut())
            }
        }

        #[cfg(test)]
        pub(crate) use std::sync::Arc;
        #[cfg(test)]
        pub(crate) use std::thread;

        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<F>(f: F)
        where
            F: Fn() + Sync + Send + 'static,
        {
            f()
        }
    }
}

macro_rules! loom_const_fn {
    (
        $(#[$meta:meta])*
        $vis:vis unsafe fn $name:ident($($arg:ident: $T:ty),*) -> $Ret:ty $body:block
    ) => {
        $(#[$meta])*
        #[cfg(not(loom))]
        $vis const unsafe fn $name($($arg: $T),*) -> $Ret $body

        $(#[$meta])*
        #[cfg(loom)]
        $vis unsafe fn $name($($arg: $T),*) -> $Ret $body
    };
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident($($arg:ident: $T:ty),*) -> $Ret:ty $body:block
    ) => {
        $(#[$meta])*
        #[cfg(not(loom))]
        $vis const fn $name($($arg: $T),*) -> $Ret $body

        $(#[$meta])*
        #[cfg(loom)]
        $vis fn $name($($arg: $T),*) -> $Ret $body
    }
}

pub(crate) use loom_const_fn;
