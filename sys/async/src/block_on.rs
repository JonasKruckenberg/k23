// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Driving futures to completion by parking the calling thread.
//!
//! [`block_on`] polls a future until it returns `Poll::Ready`, parking the
//! calling thread between polls when the future is `Pending`. What "park"
//! actually means is platform-specific — `wfi` on a kernel hart,
//! `thread::park()` on a host — and is supplied by an impl of the [`Park`]
//! trait. The platform-agnostic part — coalescing multiple wakes into a
//! single park-and-resume cycle — lives in [`Notify`].

use core::future::Future;
use core::mem::ManuallyDrop;
use core::ptr;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use futures::pin_mut;
use futures::task::WakerRef;
use util::loom_const_fn;

use crate::loom::sync::atomic::{AtomicBool, Ordering};

/// Hooks for blocking and unblocking the current thread.
///
/// Implementations must satisfy "park-token" semantics: an `unpark` that
/// races with or precedes a `park` must still cause that `park` to return.
/// `std::thread::{park, Thread::unpark}` satisfies this directly; on RISC-V
/// a pending IPI latches and is taken on the next `wfi`, satisfying the
/// same property at the hardware level.
pub trait Park {
    type Error: core::fmt::Display;

    /// Block the current thread until [`unpark`](Self::unpark) is called, or
    /// until a previously-issued `unpark` token is consumed. Spurious
    /// returns are permitted; [`block_on`] re-checks state before re-parking.
    ///
    /// # Errors
    ///
    /// Return `Self::Error` when parking the thread fails.
    /// The returned error should contain more information about the reason.
    fn park(&self) -> Result<(), Self::Error>;

    /// Wake the thread that owns this `Park`, even if it is not currently
    /// parked. Calls that precede a matching `park` must be remembered.
    ///
    /// # Errors
    ///
    /// Return `Self::Error` when unparking the target thread fails.
    /// The returned error should contain more information about the reason.
    fn unpark(&self) -> Result<(), Self::Error>;
}

/// Coalesces wake notifications across polls of [`block_on`].
///
/// `Notify` owns the "is a wake pending?" flag and decides when to actually
/// invoke [`Park::park`] / [`Park::unpark`]. Multiple wakes between two
/// polls collapse into a single `unpark` and a single resume.
///
/// Because the `Waker` produced by a `Notify` may be cloned and outlive a
/// single [`block_on`] call, callers must hold the `Notify` at `'static`.
/// This lets the `RawWakerVTable` use no-op clone/drop — a stale wake on a
/// `'static` `Notify` is a wasted `unpark`, not a use-after-free.
pub struct Notify<P> {
    parker: P,
    unparked: AtomicBool,
}

impl<P: Park> Notify<P> {
    loom_const_fn! {
        pub const fn new(parker: P) -> Self {
            Self {
                parker,
                unparked: AtomicBool::new(false),
            }
        }
    }

    /// Record a wake notification and call [`Park::unpark`] if this is the
    /// first wake since the last [`drain`](Self::drain). Returns `true` if
    /// an `unpark` was issued.
    #[inline]
    pub fn wake(&self) -> bool {
        // Release: pairs with the Acquire in `drain`.
        if !self.unparked.swap(true, Ordering::Release) {
            self.parker
                .unpark()
                .inspect_err(|err| tracing::error!("failed to unpark thread. {err}"))
                .is_ok()
        } else {
            false
        }
    }

    /// Consume any pending wake. Returns `true` if a wake was pending —
    /// the caller should re-poll without parking.
    #[inline]
    pub fn drain(&self) -> bool {
        // Acquire: pairs with the Release in `wake`.
        self.unparked.swap(false, Ordering::Acquire)
    }
}

impl<P: Park + Sync + 'static> Notify<P> {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw,
        Self::wake_raw,
        Self::wake_by_ref_raw,
        Self::drop_raw,
    );

    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        // NB: `data` the `&'static Notify<P>`, so
        // no refcounting (like other waker impls) required.
        RawWaker::new(data, &Self::VTABLE)
    }

    unsafe fn wake_raw(data: *const ()) {
        // Safety: `data` is the `&'static Notify<P>` handed to
        // `RawWaker::new` in `waker_ref`.
        let n = unsafe { &*data.cast::<Self>() };
        n.wake();
    }

    unsafe fn wake_by_ref_raw(data: *const ()) {
        // Safety: `data` is the `&'static Notify<P>` handed to
        // `RawWaker::new` in `waker_ref`.
        let n = unsafe { &*data.cast::<Self>() };
        n.wake();
    }

    unsafe fn drop_raw(_data: *const ()) {
        // NB: `data` the `&'static Notify<P>`, so nothing to do here.
        // This mirrors the `Self::clone_raw` impl.
    }

    /// Build a [`WakerRef`] pointing at this `Notify`. Cloning the resulting
    /// `Waker` is allocation-free; dropping is a no-op.
    pub fn waker_ref(&'static self) -> WakerRef<'static> {
        // Safety: the `RawWaker`'s data pointer is `&'static Self`, so any
        // waker clone — including ones that outlive the current `block_on`
        // frame — points at live memory.
        let waker = ManuallyDrop::new(unsafe {
            Waker::from_raw(RawWaker::new(
                ptr::from_ref(self).cast::<()>(),
                &Self::VTABLE,
            ))
        });
        WakerRef::new_unowned(waker)
    }
}

/// Drive `f` to completion on the current thread, parking via `notify` while
/// `f` is `Pending`.
///
/// Tasks the future spawns on a runtime are **not** polled by this
/// function — it only polls `f` itself. Drive a worker future
/// (e.g. `Worker::run`) to also poll spawned tasks.
///
/// Caller must not nest `block_on` calls on the same `notify`: the inner
/// `drain()` would swallow the outer's pending wake.
///
/// # Errors
///
/// Returns `P::Error` when blocking the current thread fails.
/// The returned error contains more information about the reason.
// NB: we require the static lifetime here so we can safely turn this into a ptr in `waker_ref`.
// instead of forcing callers into an `Arc` allocation they can now simply use `cpu_local!`
pub fn block_on<P: Park + Sync + 'static, F: Future>(
    notify: &'static Notify<P>,
    f: F,
) -> Result<F::Output, P::Error> {
    pin_mut!(f);
    let waker = notify.waker_ref();
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(t) = f.as_mut().poll(&mut cx) {
            return Ok(t);
        }
        while !notify.drain() {
            notify.parker.park()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::task::{Context, Poll, Waker};
    use std::convert::Infallible;

    use futures::future;

    use super::*;
    use crate::loom;
    use crate::loom::sync::atomic::AtomicUsize;
    use crate::loom::sync::{Arc, Mutex};
    use crate::loom::thread;
    use crate::test_util::StdPark;

    /// Wake-from-the-same-thread happy path: `block_on` returns without ever
    /// parking when the future is immediately `Ready`.
    #[test]
    fn ready_returns_immediately() {
        loom::model(|| {
            loom::lazy_static! {
                static ref NOTIFY: Notify<StdPark> = Notify::new(StdPark::current());
            }
            assert_eq!(block_on(&NOTIFY, async { 42_u32 }).unwrap(), 42);
        });
    }

    /// Two threads race to wake the same `Notify`. The flag must serialize
    /// them: exactly one `wake()` returns `true` (i.e. exactly one `unpark`
    /// would be issued), regardless of interleaving.
    ///
    /// This is the core invariant the kernel refactor relies on — without
    /// it, a wake could be silently dropped and a parked hart would never
    /// resume.
    ///
    /// Uses `NoopPark` to isolate the flag mechanic from any thread parking
    /// state — loom's `Thread::unpark` bookkeeping isn't what we're testing.
    #[test]
    fn concurrent_wakes_coalesce() {
        struct NoopPark;
        impl Park for NoopPark {
            type Error = Infallible;
            fn park(&self) -> Result<(), Self::Error> {
                Ok(())
            }
            fn unpark(&self) -> Result<(), Self::Error> {
                Ok(())
            }
        }

        loom::model(|| {
            loom::lazy_static! {
                static ref NOTIFY: Notify<NoopPark> = Notify::new(NoopPark);
            }

            let unparks = Arc::new(AtomicUsize::new(0));
            let u_a = unparks.clone();
            let u_b = unparks.clone();

            let t_a = thread::spawn(move || {
                if NOTIFY.wake() {
                    u_a.fetch_add(1, Ordering::SeqCst);
                }
            });
            let t_b = thread::spawn(move || {
                if NOTIFY.wake() {
                    u_b.fetch_add(1, Ordering::SeqCst);
                }
            });

            t_a.join().unwrap();
            t_b.join().unwrap();

            assert_eq!(unparks.load(Ordering::SeqCst), 1);
            assert!(NOTIFY.drain(), "flag should be set after at least one wake");
            assert!(!NOTIFY.drain(), "flag should be cleared after drain");
        });
    }

    /// Future returns `Pending` on first poll (stashing the waker), then
    /// `Ready` on subsequent polls. A second thread fires the stashed waker.
    /// `block_on` must observe the wake whether it arrives before the park,
    /// during the park, or after the next poll has already started.
    ///
    /// Loom explores all those interleavings.
    #[test]
    fn cross_thread_wake_unblocks() {
        loom::model(|| {
            loom::lazy_static! {
                static ref NOTIFY: Notify<StdPark> = Notify::new(StdPark::current());
            }
            let waker_slot: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
            let slot_for_waker = waker_slot.clone();

            let waker_thread = thread::spawn(move || {
                // Spin until the future has registered the waker.
                loop {
                    if let Some(w) = slot_for_waker.lock().unwrap().take() {
                        w.wake();
                        return;
                    }
                    thread::yield_now();
                }
            });

            let slot_for_future = waker_slot;
            let polls = Arc::new(AtomicUsize::new(0));
            let polls_for_future = polls.clone();
            let fut = future::poll_fn(move |cx: &mut Context<'_>| {
                let n = polls_for_future.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    *slot_for_future.lock().unwrap() = Some(cx.waker().clone());
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            });

            block_on(&NOTIFY, fut).unwrap();
            waker_thread.join().unwrap();
        });
    }

    /// A future that wakes itself from inside `poll` and returns `Pending`
    /// must not park: the wake set the flag, so `drain` returns true on the
    /// next loop iteration and the future is re-polled immediately.
    ///
    /// This is the simplest concrete case of the wake-before-park race —
    /// single-threaded, so any regression in the `poll` -> `drain` -> `park`
    /// ordering shows up as a hung test.
    #[test]
    fn self_wake_does_not_park() {
        loom::model(|| {
            loom::lazy_static! {
                static ref NOTIFY: Notify<StdPark> = Notify::new(StdPark::current());
            }

            let polls = Arc::new(AtomicUsize::new(0));
            let polls_for_future = polls.clone();
            block_on(
                &NOTIFY,
                future::poll_fn(move |cx: &mut Context<'_>| {
                    if polls_for_future.fetch_add(1, Ordering::SeqCst) == 0 {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else {
                        Poll::Ready::<()>(())
                    }
                }),
            )
            .unwrap();
            assert_eq!(polls.load(Ordering::SeqCst), 2);
        });
    }

    /// Soundness: the `Waker` may be cloned and used after `block_on`
    /// returns. With the no-op vtable backing a `&'static Notify`, this
    /// must not be a use-after-free — the wake just lands as a wasted
    /// unpark on the still-live `Notify`.
    #[test]
    fn waker_outlives_block_on() {
        loom::model(|| {
            loom::lazy_static! {
                static ref NOTIFY: Notify<StdPark> = Notify::new(StdPark::current());
            }
            let stashed: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
            let stashed_c = stashed.clone();

            block_on(
                &NOTIFY,
                future::poll_fn(move |cx: &mut Context<'_>| {
                    *stashed_c.lock().unwrap() = Some(cx.waker().clone());
                    Poll::Ready::<()>(())
                }),
            )
            .unwrap();

            let waker = stashed.lock().unwrap().take().expect("waker stashed");
            // Must not UAF; the parker is still live.
            waker.wake();
            // Hygiene: drain the wasted-unpark token so it doesn't leak
            // into another iteration's loom state.
            assert!(NOTIFY.drain());
        });
    }
}
