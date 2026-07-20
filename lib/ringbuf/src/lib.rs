// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! A bounded lock-free multi-producer multi-consumer queue.
//!
//! The design follows Maxim Egorushkin's [atomic_queue]: two free-running counters (`head`,
//! `tail`) hand out slot tickets via atomic increment, and each slot carries a small state
//! machine so producers and consumers never contend on a shared lock.
//!
//! # Modes
//!
//! Each side of the queue can drop an atomic read-modify-write when only one thread uses it:
//!
//! - **MPMC** (the default, [`RingBuf<N, T>`]) — any number of producers and consumers, all
//!   operations available through `&self`.
//! - **MPSC** — [`RingBuf::split`] on a default queue yields a cloneable [`Producer`] and a unique
//!   [`Consumer`], making single-consumer use checkable by the compiler. It uses the same code as
//!   MPMC: see [`Consumer::pop`] for why the relaxation is *not* applied here.
//! - **SPSC** — `RingBuf<N, T, true>` additionally makes the producer unique, relaxing the push
//!   path too. Only reachable through [`RingBuf::split`], because a shared-reference API would let
//!   two threads push and race.
//!
//! [atomic_queue]: https://github.com/max0x7ba/atomic_queue

#![cfg_attr(not(test), no_std)]

mod loom;
#[cfg(test)]
mod tests;

use core::marker::PhantomData;
use core::mem::MaybeUninit;

use util::{CACHE_LINE_SIZE, CachePadded};

use crate::loom::cell::UnsafeCell;
use crate::loom::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// Slot holds no value; a producer may claim it.
const EMPTY: u8 = 0;
/// Slot holds a value; a consumer may claim it.
const STORED: u8 = 1;
/// A producer has claimed the slot and is writing into it.
const STORING: u8 = 2;
/// A consumer has claimed the slot and is reading out of it.
const LOADING: u8 = 4;

struct Slot<T> {
    /// The message.
    value: UnsafeCell<MaybeUninit<T>>,
    /// The state of the slot.
    state: AtomicU8,
}

/// A bounded lock-free queue of `N` elements.
///
/// `N` must be a power of two — [`Self::remap`] masks indices with `N - 1`, and the free-running
/// `u32` counters must wrap at a multiple of `N` for the index-to-slot mapping to stay consistent
/// across a counter wrap. A non-power-of-two `N` fails to compile.
///
/// `SPSC` promises the queue has exactly one producer *and* one consumer, letting both sides skip
/// their atomic read-modify-writes. The promise is not taken on trust: an `SPSC` queue exposes no
/// shared-reference push or pop, so the only way to use one is [`Self::split`], whose handles are
/// unique by construction. See the [module docs](self#modes).
pub struct RingBuf<const N: usize, T, const SPSC: bool = false> {
    head: CachePadded<AtomicU32>,
    tail: CachePadded<AtomicU32>,
    slots: [Slot<T>; N],
}

// Safety: the queue transfers ownership of `T` between threads, so `T: Send` is required and
// sufficient. Access to each slot's `UnsafeCell` is serialized by its `state` atomic: a producer
// writes only after winning `EMPTY -> STORING`, a consumer reads only after winning
// `STORED -> LOADING`, and the release/acquire pairs on `state` order the payload accesses.
unsafe impl<const N: usize, T: Send, const SPSC: bool> Send for RingBuf<N, T, SPSC> {}
// Safety: see above. Shared access hands out `T` by value from `pop`/`try_pop`, never a
// reference into a slot, so `T: Sync` is not required.
unsafe impl<const N: usize, T: Send, const SPSC: bool> Sync for RingBuf<N, T, SPSC> {}

#[inline(always)]
#[expect(clippy::cast_possible_wrap, reason = "the wrap is the point")]
const fn distance(a: u32, b: u32) -> i32 {
    a.wrapping_sub(b) as i32
}

const fn get_shuffle_bits(array_size: usize, elements_per_cache_line: usize) -> usize {
    let bits = match elements_per_cache_line {
        256 => 8,
        128 => 7,
        64 => 6,
        32 => 5,
        16 => 4,
        8 => 3,
        4 => 2,
        2 => 1,
        _ => unreachable!(),
    };
    let min_size = 1 << (bits * 2);
    if array_size < min_size { 0 } else { bits }
}

impl<const N: usize, T, const SPSC: bool> RingBuf<N, T, SPSC> {
    const ASSERT_POWER_OF_TWO: () = assert!(
        N.is_power_of_two(),
        "RingBuf capacity N must be a power of two"
    );

    /// The signed-distance comparisons below only hold while the number of outstanding tickets
    /// stays inside `i32`, so the capacity must too.
    const ASSERT_FITS_SIGNED: () = assert!(
        N <= i32::MAX as usize,
        "RingBuf capacity N must fit in an i32"
    );

    /// `N` as a signed count, for comparison against [`distance`].
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "ASSERT_FITS_SIGNED, evaluated in `new`, rules out both"
    )]
    const CAPACITY: i32 = N as i32;

    const SHUFFLE_BITS: usize = { get_shuffle_bits(N, CACHE_LINE_SIZE / size_of::<AtomicU8>()) };

    const MASK_ELEM_IDX: usize = !(!0 << Self::SHUFFLE_BITS);
    const MASK_HI: usize = !0 << (2 * Self::SHUFFLE_BITS);

    /// Maps a free-running counter value onto a slot index, swapping the low `SHUFFLE_BITS` with
    /// the next `SHUFFLE_BITS` so that consecutively-issued tickets land on different cache lines.
    #[inline(always)]
    const fn remap(index: u32) -> usize {
        ((index as usize >> Self::SHUFFLE_BITS) & Self::MASK_ELEM_IDX)
            | ((index as usize & Self::MASK_ELEM_IDX) << Self::SHUFFLE_BITS)
            | (index as usize & (Self::MASK_HI & (N - 1)))
    }

    /// Creates a new empty queue.
    #[must_use]
    #[cfg(not(loom))]
    pub const fn new() -> Self {
        let () = Self::ASSERT_POWER_OF_TWO;
        let () = Self::ASSERT_FITS_SIGNED;

        Self {
            head: CachePadded(AtomicU32::new(0)),
            tail: CachePadded(AtomicU32::new(0)),
            slots: [const {
                Slot {
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                    state: AtomicU8::new(EMPTY),
                }
            }; N],
        }
    }

    /// Creates a new empty queue.
    #[must_use]
    #[cfg(loom)]
    pub fn new() -> Self {
        let () = Self::ASSERT_POWER_OF_TWO;
        let () = Self::ASSERT_FITS_SIGNED;

        Self {
            head: CachePadded(AtomicU32::new(0)),
            tail: CachePadded(AtomicU32::new(0)),
            slots: core::array::from_fn(|_| Slot {
                value: UnsafeCell::new(MaybeUninit::uninit()),
                state: AtomicU8::new(EMPTY),
            }),
        }
    }

    /// The maximum number of elements the queue can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// The number of elements in the queue at some point during the call.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Relaxed);
        usize::try_from(distance(head, tail).max(0)).unwrap_or(0)
    }

    /// Whether the queue held no elements at some point during the call. See [`Self::len`].
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the queue was at capacity at some point during the call. See [`Self::len`].
    #[must_use]
    pub(crate) fn is_full(&self) -> bool {
        self.len() >= N
    }

    /// Splits the queue into a producer and a consumer handle.
    #[must_use]
    pub fn split(&mut self) -> (Producer<'_, N, T, SPSC>, Consumer<'_, N, T, SPSC>) {
        (
            Producer {
                queue: self,
                _not_sync: PhantomData,
            },
            Consumer {
                queue: self,
                _not_sync: PhantomData,
            },
        )
    }

    /// Pushes an element, returning it back as `Some` if the queue was full.
    #[inline]
    fn do_try_push(&self, element: T) -> Option<T> {
        let mut head = self.head.0.load(Ordering::Relaxed);

        loop {
            if distance(head, self.tail.0.load(Ordering::Relaxed)) >= Self::CAPACITY {
                return Some(element);
            }
            if SPSC {
                self.head.0.store(head.wrapping_add(1), Ordering::Relaxed);
                break;
            }
            match self.head.0.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                // Another producer took this ticket; retry against the value it left behind.
                Err(actual) => head = actual,
            }
        }

        self.push_internal(element, head);
        None
    }

    /// Pushes an element, spinning until a slot frees up if the queue is full.
    #[inline]
    fn do_push(&self, element: T) {
        let head = if SPSC {
            // Sole producer, so the counter has a single writer and needs no read-modify-write.
            let head = self.head.0.load(Ordering::Relaxed);
            self.head.0.store(head.wrapping_add(1), Ordering::Relaxed);
            head
        } else {
            self.head.0.fetch_add(1, Ordering::Relaxed)
        };
        self.push_internal(element, head);
    }

    /// Writes `element` into the slot for `head`, spinning until the slot to become free.
    #[inline(always)]
    fn push_internal(&self, element: T, head: u32) {
        let slot = &self.slots[Self::remap(head)];

        if SPSC {
            while slot.state.load(Ordering::Acquire) != EMPTY {
                crate::loom::spin_hint();
            }
        } else {
            while slot
                .state
                .compare_exchange_weak(EMPTY, STORING, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                // Spin on a plain load rather than hammering the cache line with failed CAS traffic.
                while slot.state.load(Ordering::Relaxed) != EMPTY {
                    crate::loom::spin_hint();
                }
            }
        }

        slot.value.with_mut(|value| {
            // Safety: the slot was observed `EMPTY` — and with multiple producers claimed via CAS —
            // so this thread has exclusive access. No consumer can claim it until the `STORED`
            // release store below.
            unsafe { (*value).write(element) };
        });
        slot.state.store(STORED, Ordering::Release);
    }

    /// Pops an element, returning `None` if the queue was empty.
    #[inline]
    fn do_try_pop(&self) -> Option<T> {
        let mut tail = self.tail.0.load(Ordering::Relaxed);

        loop {
            if distance(self.head.0.load(Ordering::Relaxed), tail) <= 0i32 {
                return None;
            }
            if SPSC {
                self.tail.0.store(tail.wrapping_add(1), Ordering::Relaxed);
                break;
            }
            match self.tail.0.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                // Another consumer took this ticket; retry against the value it left behind.
                Err(actual) => tail = actual,
            }
        }

        Some(self.pop_internal(tail))
    }

    /// Pops an element, spinning until one arrives if the queue is empty.
    #[inline]
    fn do_pop(&self) -> T {
        let tail = if SPSC {
            // Sole consumer, so the counter has a single writer.
            let tail = self.tail.0.load(Ordering::Relaxed);
            self.tail.0.store(tail.wrapping_add(1), Ordering::Relaxed);
            tail
        } else {
            self.tail.0.fetch_add(1, Ordering::Relaxed)
        };
        self.pop_internal(tail)
    }

    /// Reads the element out of the slot for `tail`, waiting for one to arrive.
    #[inline(always)]
    fn pop_internal(&self, tail: u32) -> T {
        let slot = &self.slots[Self::remap(tail)];

        if SPSC {
            while slot.state.load(Ordering::Acquire) != STORED {
                crate::loom::spin_hint();
            }
        } else {
            while slot
                .state
                .compare_exchange_weak(STORED, LOADING, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                while slot.state.load(Ordering::Relaxed) != STORED {
                    crate::loom::spin_hint();
                }
            }
        }

        let element = slot.value.with(|value| {
            // Safety: the slot was observed `STORED` — and with multiple consumers claimed via CAS
            // — so this thread has exclusive access, and the acquire pairs with the producer's
            // release store, making the payload write visible. `STORED` is only ever published
            // after a completed write, so the value is initialized.
            unsafe { (*value).assume_init_read() }
        });
        slot.state.store(EMPTY, Ordering::Release);

        element
    }
}

/// The shared-reference MPMC API, available only when `SPSC` is `false`.
///
/// An `SPSC` queue deliberately omits these: its relaxed push path is only sound with a single
/// producer, which `&self` cannot guarantee. Use [`RingBuf::split`] instead.
impl<const N: usize, T> RingBuf<N, T, false> {
    /// Pushes an element, returning it back as `Some` if the queue was full.
    #[inline]
    pub fn try_push(&self, element: T) -> Option<T> {
        self.do_try_push(element)
    }

    /// Pushes an element, spinning until a slot frees up if the queue is full.
    #[inline]
    pub fn push(&self, element: T) {
        self.do_push(element);
    }

    /// Pops an element, returning `None` if the queue was empty.
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        self.do_try_pop()
    }

    /// Pops an element, spinning until one arrives if the queue is empty.
    #[inline]
    pub fn pop(&self) -> T {
        self.do_pop()
    }
}

/// The producing half of a [split](RingBuf::split) queue.
///
/// `Clone` when the queue is not `SPSC`, giving any number of producers. On an `SPSC` queue it is
/// deliberately not `Clone`, so a single producer is guaranteed and the push path can drop its
/// read-modify-writes.
pub struct Producer<'a, const N: usize, T, const SPSC: bool> {
    queue: &'a RingBuf<N, T, SPSC>,
    /// Keeps the handle from being *shared* between threads. Two threads pushing through one `&`
    /// would defeat the single-producer guarantee just as two clones would.
    _not_sync: PhantomData<*const ()>,
}

// Safety: the handle may be moved to another thread — that is how a producer is put to work — but
// `_not_sync` keeps it from being shared, so `SPSC` mode still sees exactly one pusher.
unsafe impl<const N: usize, T: Send, const SPSC: bool> Send for Producer<'_, N, T, SPSC> {}

impl<const N: usize, T> Clone for Producer<'_, N, T, false> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue,
            _not_sync: PhantomData,
        }
    }
}

impl<const N: usize, T, const SPSC: bool> Producer<'_, N, T, SPSC> {
    /// Pushes an element, returning it back as `Some` if the queue was full.
    #[inline]
    pub fn try_push(&self, element: T) -> Option<T> {
        self.queue.do_try_push(element)
    }

    /// Pushes an element, spinning until a slot frees up if the queue is full.
    #[inline]
    pub fn push(&self, element: T) {
        self.queue.do_push(element);
    }

    /// The maximum number of elements the queue can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N
    }
}

/// The consuming half of a [split](RingBuf::split) queue.
///
/// Never `Clone`, in either mode, so there is provably one consumer.
pub struct Consumer<'a, const N: usize, T, const SPSC: bool> {
    queue: &'a RingBuf<N, T, SPSC>,
    /// Keeps the handle from being shared between threads; see [`Producer`].
    _not_sync: PhantomData<*const ()>,
}

// Safety: as for `Producer` — movable between threads, never shared, so there is exactly one
// consumer however the handle travels.
unsafe impl<const N: usize, T: Send, const SPSC: bool> Send for Consumer<'_, N, T, SPSC> {}

impl<const N: usize, T, const SPSC: bool> Consumer<'_, N, T, SPSC> {
    /// Pops an element, returning `None` if the queue was empty.
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        self.queue.do_try_pop()
    }

    /// Pops an element, spinning until one arrives if the queue is empty.
    ///
    /// Uniqueness would make the relaxed pop path sound in MPSC mode too, but it is deliberately
    /// only taken when `SPSC` — that is, when the producer is relaxed as well. Measured on
    /// aarch64, relaxing *only* the consumer while producers still CAS is about 4.8x slower than
    /// leaving both on the CAS path (60 vs 289 Melem/s at one producer). The likely cause is that
    /// the relaxed consumer loads the slot state shared and then upgrades to store `EMPTY`, an
    /// extra coherence transaction per element, where the CAS takes the line exclusive once. With
    /// both sides relaxed the pattern is symmetric and the effect disappears.
    ///
    /// So MPSC buys a compiler-checked single-consumer API, not extra speed.
    #[inline]
    pub fn pop(&self) -> T {
        self.queue.do_pop()
    }

    /// The maximum number of elements the queue can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N
    }
}

impl<const N: usize, T, const SPSC: bool> Default for RingBuf<N, T, SPSC> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, T, const SPSC: bool> Drop for RingBuf<N, T, SPSC> {
    fn drop(&mut self) {
        // NB: `&mut self` ensures exclusive ownership. Every slot is either `EMPTY` or `STORED`
        // and we need no synchronization.
        for slot in &self.slots {
            if slot.state.load(Ordering::Relaxed) == STORED {
                slot.value.with_mut(|value| {
                    // Safety: `STORED` is only published after a completed write, so the value is
                    // initialized, and exclusive access means nothing else can read it after.
                    unsafe { (*value).assume_init_drop() };
                });
            }
        }
    }
}
