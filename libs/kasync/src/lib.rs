//! Async executor and supporting infrastructure for k23 cooperative multitasking.
//!
//! This crate was heavily inspired by tokio and the (much better) maitake crates, to a small extend smol also influenced the design.

#![cfg_attr(not(any(test, feature = "__bench")), no_std)]
#![cfg_attr(loom, feature(arbitrary_self_types))]
#![feature(allocator_api)]
#![feature(const_type_id)]
#![feature(thread_local)]
#![feature(debug_closure_helpers)]

extern crate alloc;

pub mod executor;
#[doc(hidden)] // only public for benchmarks
pub mod loom;
pub mod park;
pub mod scheduler;
pub mod sync;
pub mod task;
#[cfg(test)]
mod test_util;
pub mod time;

/// Returns a [`Clock`] with 1ms precision that is backed by the system clock
#[cfg(any(test, feature = "__bench"))]
#[macro_export]
macro_rules! std_clock {
    () => {{
        $crate::loom::lazy_static! {
            static ref TIME_ANCHOR: ::std::time::Instant = ::std::time::Instant::now();
        }

        $crate::time::Clock::new(::core::time::Duration::from_millis(1), move || {
            $crate::time::Ticks(TIME_ANCHOR.elapsed().as_millis() as u64)
        })
    }};
}

cfg_if::cfg_if! {
    if #[cfg(any(test, feature = "__bench"))] {
        pub struct StdPark(crate::loom::thread::Thread);

        impl park::Park for StdPark {
            fn park(&self) {
                tracing::trace!("parking current thread ({:?})...", self.0);
                crate::loom::thread::park();
            }

            #[cfg(not(loom))]
            fn park_until(&self, deadline: crate::time::Deadline, clock: &crate::time::Clock) {
                let instant = deadline.as_instant(clock);
                let dur = instant.elapsed(clock);
                crate::loom::thread::park_timeout(dur);
            }

            #[cfg(loom)]
            fn park_until(&self, _deadline: Deadline, _clock: &Clock) {
                unreachable!("loom doesn't support `park_timeout`");
            }

            fn unpark(&self) {
                tracing::trace!("unparking thread {:?}...", self.0);
                self.0.unpark();
            }
        }

        impl StdPark {
            pub fn for_current() -> Self {
                Self(crate::loom::thread::current())
            }
        }
    }
}
