#![cfg_attr(not(test), no_std)]
#![feature(thread_local)]

extern crate alloc;

mod mutex;
mod raw_mutex;
mod remutex;

pub use mutex::{Mutex, MutexGuard};
pub use remutex::{ReentrantMutex, ReentrantMutexGuard};

#[cfg(test)]
macro_rules! assert_not_send {
    ($x:ty) => {
        const _: fn() -> () = || {
            struct Check<T: ?Sized>(T);
            trait AmbiguousIfImpl<A> {
                fn some_item() {}
            }

            impl<T: ?Sized> AmbiguousIfImpl<()> for Check<T> {}
            impl<T: ?Sized + Send> AmbiguousIfImpl<u8> for Check<T> {}

            <Check<$x> as AmbiguousIfImpl<_>>::some_item()
        };
    };
}
#[cfg(test)]
pub(crate) use assert_not_send;
