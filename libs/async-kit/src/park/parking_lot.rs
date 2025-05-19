// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::park::parker::Parker;
use crate::park::{Park, UnparkToken};
use alloc::boxed::Box;
use core::fmt;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use mpsc_queue::{MpscQueue, TryDequeueError};

pub struct ParkingLot<P> {
    /// Total number of cores
    num_threads: usize,
    /// Number of parked cores
    num_parked: AtomicUsize,
    // Each parked core stores its UnparkToken in this list
    unpark_tokens: MpscQueue<Entry<P>>,
}

struct Entry<P> {
    token: Option<UnparkToken<P>>,
    links: mpsc_queue::Links<Self>,
}

// === impl ParkingLot ===

impl<P: Park + Send + Sync> ParkingLot<P> {
    pub fn new(num_threads: usize) -> Self {
        let entry = Box::pin(Entry {
            token: None,
            links: mpsc_queue::Links::new(),
        });

        Self {
            num_threads,
            num_parked: AtomicUsize::new(0),
            unpark_tokens: MpscQueue::new_with_stub(entry),
        }
    }

    pub fn num_parked(&self) -> usize {
        self.num_parked.load(Ordering::Acquire)
    }

    /// Park the calling execution context using the provided `Parker`.
    ///
    /// Once parked, the execution context will not make progress until unparked through either
    /// `Self::unpark_one` or `Self::unpark_all`.
    pub fn park(&self, parker: Parker<P>) {
        // Increment `num_idle` before we park ourselves
        let prev = self.num_parked.fetch_add(1, Ordering::Release);
        debug_assert!(
            prev < self.num_threads,
            "ParkingLot({max}) configuration supports {max} simultaneously parked threads, but caller attempted to park {prev}",
            max = self.num_threads
        );

        let entry = Box::pin(Entry {
            token: Some(parker.clone().into_unpark()),
            links: mpsc_queue::Links::new(),
        });
        self.unpark_tokens.enqueue(entry);

        parker.park();

        // Decrement `num_idle`, we're no longer parked!
        let prev = self.num_parked.fetch_sub(1, Ordering::Release);
        debug_assert!(
            prev <= self.num_threads,
            "ParkingLot({max}) configuration supports {max} simultaneously parked threads, but caller attempted to park {prev}",
            max = self.num_threads
        );
    }

    /// Try to unpark a single  execution context, returning a [`TryDequeueError`] if the queue of
    /// parked targets is empty, or currently busy.
    ///
    /// This method will choose an arbitrary context that has previously parked themselves through
    /// `Self::park`. The order in which individual target are woken is *not defined* and may change
    /// at any point.
    ///
    /// # Errors
    ///
    /// The returned [`TryDequeueError`] indicates the state of the internal parked contexts queue,
    /// if the error is either `TryDequeueError::Busy` or `TryDequeueError::Inconsistent` then the
    /// caller might want to wait a bit and retry. `TryDequeueError::Empty` means no parked contexts
    /// are registered with the `ParkingLot` currently.
    #[expect(clippy::missing_panics_doc, reason = "internal assertions")]
    pub fn try_unpark_one(&self) -> Result<(), TryDequeueError> {
        let entry = self.unpark_tokens.try_dequeue()?;
        entry
            .token
            .as_ref()
            .expect("cannot unpark the stub parker")
            .unpark();

        Ok(())
    }

    /// Unpark a single execution context, blocking if the queue of parked targets is busy.
    /// Returns `true` when a target was unparked and `false` otherwise.
    ///
    /// This method will choose an arbitrary context that has previously parked themselves through
    /// `Self::park`. The order in which individual target are woken is *not defined* and may change
    /// at any point.
    #[expect(clippy::missing_panics_doc, reason = "internal assertions")]
    pub fn unpark_one(&self) -> bool {
        if let Some(entry) = self.unpark_tokens.dequeue() {
            entry
                .token
                .as_ref()
                .expect("cannot unpark the stub parker")
                .unpark();

            true
        } else {
            false
        }
    }

    // Unpark all currently parked execution contexts, returning the number of targets
    // that were actually unparked or `None` if the queue of targets is already being dequeued.
    //
    // This method will unpark contexts in an arbitrary order, no guarantee is made about specific
    // ordering and the underlying implementation may change at any point.
    #[expect(clippy::missing_panics_doc, reason = "internal assertions")]
    pub fn unpark_all(&self) -> Option<usize> {
        let c = self.unpark_tokens.try_consume()?;
        let mut unparked = 0;

        while let Some(entry) = c.dequeue() {
            entry
                .token
                .as_ref()
                .expect("cannot unpark the stub parker")
                .unpark();
            unparked += 1;
        }

        Some(unparked)
    }
}

// === impl Entry ===

impl<P> fmt::Debug for Entry<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("token", &self.token)
            .field("links", &self.links)
            .finish()
    }
}

// Safety: TODO
unsafe impl<P> mpsc_queue::Linked for Entry<P> {
    type Handle = Pin<Box<Self>>;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        // Safety: The implementation treats `NonNull` as pinned.
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        unsafe { Pin::new_unchecked(Box::from_raw(ptr.as_ptr())) }
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<mpsc_queue::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom::sync::{atomic::AtomicUsize, Arc};
    use crate::loom::thread;
    use crate::park::StdPark;
    use alloc::vec::Vec;
    use spin::Backoff;

    #[cfg_attr(not(loom), test)] // the test currently doesnt pass under loom :/
    fn parking_lot_basically_works() {
        crate::loom::model(|| {
            crate::loom::lazy_static! {
                static ref UNPARKED: AtomicUsize = AtomicUsize::new(0);
            }

            let lot: Arc<ParkingLot<StdPark>> = Arc::new(ParkingLot::new(4));

            let joins: Vec<_> = (0..4)
                .map(|_| {
                    let lot = lot.clone();
                    thread::spawn(move || {
                        lot.park(Parker::new(StdPark::for_current()));
                        UNPARKED.fetch_add(1, Ordering::Release);
                    })
                })
                .collect();

            let mut boff = Backoff::new();
            for _ in 0..4 {
                while !lot.unpark_one() {
                    boff.spin();
                }
                boff.reset();
            }

            for join in joins {
                join.join().unwrap();
            }

            assert_eq!(UNPARKED.load(Ordering::Acquire), 4);
        })
    }
}
