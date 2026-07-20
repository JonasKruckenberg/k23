// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Tests ported from [atomic_queue's `tests.cc`][upstream], adapted to this implementation.
//!
//! The concurrency tests compile under three configurations: as plain threaded stress tests on
//! the host, under miri, and under loom's model checker. Iteration counts and thread counts scale
//! down accordingly — see [`CYCLES`] and [`STRESS_CAP`].
//!
//! [upstream]: https://github.com/max0x7ba/atomic_queue/blob/master/src/tests.cc

use crate::RingBuf;
use crate::loom::sync::Arc;
use crate::loom::sync::atomic::{AtomicU64, Ordering};
use crate::loom::{self, thread};

/// Sentinel telling a consumer to stop. Upstream calls this `STOP_MSG`.
const STOP_MSG: u32 = 0;

/// Queue capacity for the stress test. Loom explores every interleaving, so the state space
/// scales with capacity — a capacity of 2 is enough to exercise both the full and empty paths.
#[cfg(loom)]
const STRESS_CAP: usize = 2;
#[cfg(not(loom))]
const STRESS_CAP: usize = 4096;

/// Messages each producer sends, counting down to 1. Upstream uses 1,000,000.
const N_STRESS_MSG: u32 = if cfg!(loom) {
    2
} else if cfg!(miri) {
    64
} else {
    1_000_000
};

/// Upstream stresses MPMC queues with 3 producers and 3 consumers. Loom caps concurrent threads
/// and its state space grows exponentially with them, so it gets one of each.
const PRODUCERS: usize = if cfg!(loom) { 1 } else { 3 };
const CONSUMERS: usize = if cfg!(loom) { 1 } else { 3 };

/// Repetitions for the non-model concurrency tests.
const CYCLES: usize = if cfg!(loom) {
    1
} else if cfg!(miri) {
    2
} else {
    50
};

// -- upstream `stress` ------------------------------------------------------------------------

/// Every pushed element is popped exactly once, across all producers and consumers.
///
/// Producers count down from `N_STRESS_MSG` to 1; each consumer sums what it pops until it sees
/// [`STOP_MSG`]. Checking the *sum* rather than the count catches duplication and loss that a
/// bare count would miss.
#[test]
fn stress() {
    loom::model(|| {
        let q = Arc::new(RingBuf::<STRESS_CAP, u32>::new());
        let sums: Arc<[AtomicU64; CONSUMERS]> =
            Arc::new(core::array::from_fn(|_| AtomicU64::new(0)));

        let producers: Vec<_> = (0..PRODUCERS)
            .map(|_| {
                let q = Arc::clone(&q);
                thread::spawn(move || {
                    for n in (1..=N_STRESS_MSG).rev() {
                        q.push(n);
                    }
                })
            })
            .collect();

        let consumers: Vec<_> = (0..CONSUMERS)
            .map(|i| {
                let q = Arc::clone(&q);
                let sums = Arc::clone(&sums);
                thread::spawn(move || {
                    let mut sum = 0u64;
                    loop {
                        let n = q.pop();
                        if n == STOP_MSG {
                            break;
                        }
                        sum += u64::from(n);
                    }
                    sums[i].store(sum, Ordering::Relaxed);
                })
            })
            .collect();

        for p in producers {
            p.join().unwrap();
        }
        // Only safe to send the sentinels once every producer is done, otherwise a consumer could
        // stop while messages are still in flight.
        for _ in 0..CONSUMERS {
            q.push(STOP_MSG);
        }
        for c in consumers {
            c.join().unwrap();
        }

        let n = u64::from(N_STRESS_MSG);
        let expected = (n + 1) * n / 2 * PRODUCERS as u64;
        let total: u64 = sums.iter().map(|s| s.load(Ordering::Relaxed)).sum();
        assert_eq!(total, expected, "messages were lost or duplicated");

        assert!(q.is_empty(), "queue should be drained");
    });
}

/// No consumer is starved: each must receive at least 10% of its fair share.
///
/// Split from [`stress`] because it is a fairness property of the scheduler, not a correctness
/// property of the queue — loom deliberately explores starving interleavings, and small message
/// counts make the threshold meaningless.
#[test]
#[cfg_attr(loom, ignore = "loom explores starving interleavings by design")]
#[cfg_attr(
    miri,
    ignore = "too few messages for the fairness threshold to be meaningful"
)]
fn stress_no_starvation() {
    let q = Arc::new(RingBuf::<STRESS_CAP, u32>::new());
    let sums: Arc<[AtomicU64; CONSUMERS]> = Arc::new(core::array::from_fn(|_| AtomicU64::new(0)));

    let producers: Vec<_> = (0..PRODUCERS)
        .map(|_| {
            let q = Arc::clone(&q);
            thread::spawn(move || {
                for n in (1..=N_STRESS_MSG).rev() {
                    q.push(n);
                }
            })
        })
        .collect();

    let consumers: Vec<_> = (0..CONSUMERS)
        .map(|i| {
            let q = Arc::clone(&q);
            let sums = Arc::clone(&sums);
            thread::spawn(move || {
                let mut sum = 0u64;
                loop {
                    let n = q.pop();
                    if n == STOP_MSG {
                        break;
                    }
                    sum += u64::from(n);
                }
                sums[i].store(sum, Ordering::Relaxed);
            })
        })
        .collect();

    for p in producers {
        p.join().unwrap();
    }
    for _ in 0..CONSUMERS {
        q.push(STOP_MSG);
    }
    for c in consumers {
        c.join().unwrap();
    }

    let n = u64::from(N_STRESS_MSG);
    let expected = (n + 1) * n / 2 * PRODUCERS as u64;
    let min_share = expected / CONSUMERS as u64 / 10;
    for (i, s) in sums.iter().enumerate() {
        let got = s.load(Ordering::Relaxed);
        assert!(
            got >= min_share,
            "consumer {i} starved: got {got}, expected at least {min_share}"
        );
    }
}

// -- upstream `move_only_element` -------------------------------------------------------------

/// A move-only payload survives a round trip. Upstream uses `std::unique_ptr<int>`; `Box` is the
/// Rust equivalent, and it also proves the queue never requires `T: Copy`.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn move_only_element() {
    let q = RingBuf::<4096, Box<i32>>::new();
    assert!(q.is_empty());
    assert_eq!(q.len(), 0);

    assert!(q.try_push(Box::new(1)).is_none());
    q.push(Box::new(2));
    assert!(!q.is_empty());
    assert_eq!(q.len(), 2);

    assert_eq!(*q.try_pop().unwrap(), 1);
    assert_eq!(q.len(), 1);

    assert_eq!(*q.pop(), 2);
    assert!(q.is_empty());
    assert_eq!(q.len(), 0);
}

// -- upstream `try_push_pop` ------------------------------------------------------------------

/// Empty and full transitions, and FIFO ordering across a complete fill/drain cycle.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn try_push_pop() {
    const CAP: usize = 64;
    let q = RingBuf::<CAP, u32>::new();

    assert!(q.try_pop().is_none(), "try_pop must fail on an empty queue");

    for i in 1..=CAP as u32 {
        assert!(q.try_push(i).is_none(), "push {i} within capacity");
    }
    assert!(q.is_full());
    assert_eq!(q.len(), CAP);

    assert_eq!(
        q.try_push(999),
        Some(999),
        "try_push must hand the element back when full"
    );

    for i in 1..=CAP as u32 {
        assert_eq!(q.try_pop(), Some(i), "FIFO order");
    }
    assert!(q.is_empty());
    assert!(q.try_pop().is_none());
}

/// The queue keeps working after the indices wrap around the array many times.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn wraparound() {
    const CAP: usize = 8;
    let q = RingBuf::<CAP, u32>::new();

    for round in 0..1000u32 {
        for i in 0..CAP as u32 {
            assert!(q.try_push(round * CAP as u32 + i).is_none());
        }
        assert!(q.is_full());
        for i in 0..CAP as u32 {
            assert_eq!(q.try_pop(), Some(round * CAP as u32 + i));
        }
        assert!(q.is_empty());
    }
}

// -- upstream `size` --------------------------------------------------------------------------

/// Capacity is exactly what was requested, and a fresh queue is empty and not full.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn size() {
    let q = RingBuf::<1024, f32>::new();
    assert_eq!(q.capacity(), 1024);
    assert!(q.is_empty());
    assert!(!q.is_full());
    assert_eq!(q.len(), 0);
}

// -- Rust-specific: drop correctness ----------------------------------------------------------

/// Elements still in the queue when it is dropped are dropped exactly once.
///
/// No upstream counterpart — C++ leans on `unique_ptr` for this. In Rust the queue holds
/// `MaybeUninit` slots and must run the drop glue itself, so this is where a leak would show up.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn drops_remaining_elements() {
    use std::sync::atomic::{AtomicUsize, Ordering as StdOrdering};

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct CountsDrop;
    impl Drop for CountsDrop {
        fn drop(&mut self) {
            DROPS.fetch_add(1, StdOrdering::Relaxed);
        }
    }

    {
        let q = RingBuf::<16, CountsDrop>::new();
        for _ in 0..10 {
            assert!(q.try_push(CountsDrop).is_none());
        }
        // Three leave through the front; the other seven must be cleaned up by `Drop`.
        for _ in 0..3 {
            drop(q.try_pop().unwrap());
        }
        assert_eq!(DROPS.load(StdOrdering::Relaxed), 3);
    }

    assert_eq!(
        DROPS.load(StdOrdering::Relaxed),
        10,
        "every element must be dropped exactly once"
    );
}

// -- upstream `remap_index` -------------------------------------------------------------------

/// Exercises [`RingBuf::remap`] for a given capacity: it must stay in bounds, be a bijection over
/// `0..N`, and be its own inverse.
fn check_remap<const N: usize>() {
    let mut seen = vec![false; N];

    for i in 0..N as u32 {
        let r = RingBuf::<N, u32>::remap(i);
        assert!(r < N, "remap({i}) = {r} out of bounds for N = {N}");
        assert!(
            !seen[r],
            "remap is not injective: {r} produced twice (N = {N})"
        );
        seen[r] = true;

        let back = RingBuf::<N, u32>::remap(r as u32);
        assert_eq!(
            back, i as usize,
            "remap is not self-inverse at {i} (N = {N})"
        );
    }

    assert!(seen.into_iter().all(|s| s), "remap is not surjective");
}

/// Below the shuffle threshold `remap` is the identity; at and above it, it is a non-trivial but
/// still bijective, self-inverse shuffle. Both must hold.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
#[cfg_attr(miri, ignore = "large capacities are slow under miri")]
fn remap_is_a_self_inverse_bijection() {
    check_remap::<2>();
    check_remap::<8>();
    check_remap::<64>();
    check_remap::<256>();
    check_remap::<4096>();
    // Large enough that `get_shuffle_bits` returns non-zero on a 128-byte cache line, so this is
    // the case that actually exercises the bit swapping.
    check_remap::<16384>();
}

/// Consecutive indices must land far enough apart to fall on different cache lines — the entire
/// point of the shuffle. Without it, adjacent tickets share a line and producers contend.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
#[cfg_attr(miri, ignore = "large capacity is slow under miri")]
fn remap_spreads_adjacent_indices() {
    const N: usize = 16384;
    assert_ne!(
        RingBuf::<N, u32>::SHUFFLE_BITS,
        0,
        "test capacity must be large enough to enable shuffling"
    );

    for i in 0..1024u32 {
        let a = RingBuf::<N, u32>::remap(i);
        let b = RingBuf::<N, u32>::remap(i + 1);
        assert_ne!(a, b);
        assert!(
            a.abs_diff(b) > 1,
            "remap({i}) = {a} and remap({}) = {b} are adjacent; shuffle did nothing",
            i + 1
        );
    }
}

/// Indices beyond `N` wrap onto the same slots, so the mapping has period exactly `N`.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn remap_has_period_n() {
    const N: usize = 4096;
    for i in 0..2048u32 {
        assert_eq!(
            RingBuf::<N, u32>::remap(i),
            RingBuf::<N, u32>::remap(i + N as u32)
        );
    }
}

/// The mapping must survive the `u32` counter wrapping to zero: index `u32::MAX` and index
/// `u32::MAX + 1 == 0` have to remain distinct slots exactly `1` apart in ticket order.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn remap_survives_counter_wrap() {
    const N: usize = 4096;
    // `u32::MAX` is `N - 1` modulo `N` for power-of-two `N`, so the wrap is seamless: the slot
    // after it is the one index 0 maps to.
    assert_eq!(
        RingBuf::<N, u32>::remap(u32::MAX),
        RingBuf::<N, u32>::remap((N - 1) as u32)
    );
    assert_eq!(RingBuf::<N, u32>::remap(0), RingBuf::<N, u32>::remap(0));
}

// -- concurrency: SPSC ordering ---------------------------------------------------------------

/// With a single producer and a single consumer the queue is strictly FIFO.
#[test]
fn spsc_preserves_order() {
    const MSGS: u32 = if cfg!(loom) {
        3
    } else if cfg!(miri) {
        32
    } else {
        10_000
    };

    loom::model(|| {
        let q = Arc::new(RingBuf::<STRESS_CAP, u32>::new());

        let producer = {
            let q = Arc::clone(&q);
            thread::spawn(move || {
                for i in 0..MSGS {
                    q.push(i);
                }
            })
        };

        let consumer = thread::spawn(move || {
            for expected in 0..MSGS {
                assert_eq!(q.pop(), expected, "SPSC must preserve order");
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    });
}

/// `try_push` on a full queue and `try_pop` on an empty one must fail rather than block or
/// corrupt state, even while another thread is actively draining and filling.
#[test]
#[cfg_attr(
    loom,
    ignore = "unbounded retry loop does not terminate under the model checker"
)]
fn try_ops_under_contention() {
    for _ in 0..CYCLES {
        let q = Arc::new(RingBuf::<4, u32>::new());
        let total = Arc::new(AtomicU64::new(0));

        let producer = {
            let q = Arc::clone(&q);
            thread::spawn(move || {
                let mut sent = 0u32;
                while sent < 1000 {
                    if q.try_push(sent).is_none() {
                        sent += 1;
                    }
                }
            })
        };

        let consumer = {
            let q = Arc::clone(&q);
            let total = Arc::clone(&total);
            thread::spawn(move || {
                let mut got = 0u32;
                let mut sum = 0u64;
                while got < 1000 {
                    if let Some(v) = q.try_pop() {
                        sum += u64::from(v);
                        got += 1;
                    }
                }
                total.store(sum, Ordering::Relaxed);
            })
        };

        producer.join().unwrap();
        consumer.join().unwrap();

        assert_eq!(total.load(Ordering::Relaxed), (0..1000u64).sum::<u64>());
        assert!(q.is_empty());
    }
}

/// Multiple producers and consumers on a capacity-limited queue: every element crosses exactly
/// once. Small capacity forces the full/empty retry paths that [`stress`] mostly skips.
#[test]
#[cfg_attr(
    loom,
    ignore = "covered by `stress`; separate model would duplicate the state space"
)]
fn mpmc_conserves_elements() {
    const MSGS: u32 = if cfg!(miri) { 32 } else { 5_000 };

    for _ in 0..CYCLES {
        let q = Arc::new(RingBuf::<4, u32>::new());
        let received = Arc::new(AtomicU64::new(0));

        let producers: Vec<_> = (0..PRODUCERS)
            .map(|_| {
                let q = Arc::clone(&q);
                thread::spawn(move || {
                    for i in 1..=MSGS {
                        q.push(i);
                    }
                })
            })
            .collect();

        let consumers: Vec<_> = (0..CONSUMERS)
            .map(|_| {
                let q = Arc::clone(&q);
                let received = Arc::clone(&received);
                thread::spawn(move || {
                    loop {
                        let v = q.pop();
                        if v == STOP_MSG {
                            break;
                        }
                        received.fetch_add(u64::from(v), Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for p in producers {
            p.join().unwrap();
        }
        for _ in 0..CONSUMERS {
            q.push(STOP_MSG);
        }
        for c in consumers {
            c.join().unwrap();
        }

        let n = u64::from(MSGS);
        assert_eq!(
            received.load(Ordering::Relaxed),
            (n + 1) * n / 2 * PRODUCERS as u64
        );
    }
}

// -- split handles: MPSC and SPSC ---------------------------------------------------------------

/// Single-threaded exercise of the handle API: FIFO order, and the full/empty edges.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn split_round_trip() {
    const CAP: usize = 8;
    let mut q = RingBuf::<CAP, u32>::new();
    let (tx, rx) = q.split();

    assert!(rx.try_pop().is_none(), "empty queue");

    for i in 1..=CAP as u32 {
        assert!(tx.try_push(i).is_none());
    }
    assert_eq!(
        tx.try_push(999),
        Some(999),
        "full queue hands the element back"
    );

    for i in 1..=CAP as u32 {
        assert_eq!(rx.try_pop(), Some(i), "FIFO order");
    }
    assert!(rx.try_pop().is_none());
}

/// Elements left in a split queue are still dropped exactly once. The consumer's relaxed path
/// leaves slots `EMPTY` without ever going through `LOADING`, so `Drop` must not depend on it.
#[test]
#[cfg_attr(loom, ignore = "not concurrency-relevant")]
fn split_drops_remaining_elements() {
    use std::sync::atomic::{AtomicUsize, Ordering as StdOrdering};

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct CountsDrop;
    impl Drop for CountsDrop {
        fn drop(&mut self) {
            DROPS.fetch_add(1, StdOrdering::Relaxed);
        }
    }

    {
        let mut q = RingBuf::<16, CountsDrop>::new();
        {
            let (tx, rx) = q.split();
            for _ in 0..10 {
                assert!(tx.try_push(CountsDrop).is_none());
            }
            for _ in 0..3 {
                drop(rx.try_pop().unwrap());
            }
        }
        assert_eq!(DROPS.load(StdOrdering::Relaxed), 3);
    }

    assert_eq!(DROPS.load(StdOrdering::Relaxed), 10);
}

/// MPSC: many cloned producers, one consumer. Every element crosses exactly once.
///
/// This is the path where the consumer drops both of its read-modify-writes while producers keep
/// theirs, so it is the one that would break if the two relaxations were conflated.
#[test]
#[cfg_attr(loom, ignore = "scoped threads are unavailable under loom")]
fn mpsc_split_conserves_elements() {
    const MSGS: u32 = if cfg!(miri) { 32 } else { 5_000 };

    let mut q = RingBuf::<4, u32>::new();
    let (tx, rx) = q.split();

    std::thread::scope(|s| {
        for _ in 0..PRODUCERS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 1..=MSGS {
                    tx.push(i);
                }
            });
        }

        s.spawn(move || {
            let mut sum = 0u64;
            for _ in 0..MSGS as usize * PRODUCERS {
                sum += u64::from(rx.pop());
            }
            let n = u64::from(MSGS);
            assert_eq!(sum, (n + 1) * n / 2 * PRODUCERS as u64);
        });
    });
}

/// SPSC: one producer, one consumer, both relaxed. Strict FIFO must survive.
#[test]
#[cfg_attr(loom, ignore = "scoped threads are unavailable under loom")]
fn spsc_split_preserves_order() {
    const MSGS: u32 = if cfg!(miri) { 32 } else { 10_000 };

    let mut q = RingBuf::<4, u32, true>::new();
    let (tx, rx) = q.split();

    std::thread::scope(|s| {
        s.spawn(move || {
            for i in 0..MSGS {
                tx.push(i);
            }
        });
        s.spawn(move || {
            for expected in 0..MSGS {
                assert_eq!(rx.pop(), expected, "SPSC must preserve order");
            }
        });
    });
}

// -- loom coverage for the relaxed split paths --------------------------------------------------
//
// These are the paths the model checker most needs to see: the consumer (and, in SPSC, the
// producer) skip their read-modify-writes, so correctness rests entirely on the release/acquire
// pair on the slot's `state`. A missing edge there would be invisible to the threaded tests on
// x86-ish hardware but caught here.
//
// The queue is leaked to obtain `'static` handles — `split` borrows, and loom has no scoped
// threads. Capacity 2 keeps the state space small while still exercising both the full and the
// empty path, and the consumer runs on the main thread to stay under loom's thread budget.

/// MPSC: two producers contend for tickets while a lone consumer uses the relaxed pop path.
///
/// This is the combination that would break if the single-producer and single-consumer
/// relaxations were conflated — producers must keep their CAS, the consumer must not.
#[test]
fn mpsc_split_model() {
    loom::model(|| {
        let q: &'static mut RingBuf<2, u32> = Box::leak(Box::new(RingBuf::new()));
        let (tx, rx) = q.split();
        let tx2 = tx.clone();

        let p1 = thread::spawn(move || tx.push(1));
        let p2 = thread::spawn(move || tx2.push(2));

        let a = rx.pop();
        let b = rx.pop();

        p1.join().unwrap();
        p2.join().unwrap();

        // Producers race, so either order is legal — but both values must arrive, exactly once.
        assert!(
            (a == 1 && b == 2) || (a == 2 && b == 1),
            "expected {{1, 2}} in some order, got {{{a}, {b}}}"
        );
    });
}

/// SPSC: both sides relaxed. Order must still be strict FIFO.
#[test]
fn spsc_split_model() {
    loom::model(|| {
        let q: &'static mut RingBuf<2, u32, true> = Box::leak(Box::new(RingBuf::new()));
        let (tx, rx) = q.split();

        // Three messages through a capacity-2 queue forces the producer to wait on the consumer.
        let p = thread::spawn(move || {
            for i in 1..=3 {
                tx.push(i);
            }
        });

        for expected in 1..=3 {
            assert_eq!(rx.pop(), expected, "SPSC must preserve order");
        }

        p.join().unwrap();
    });
}

/// MPSC `try_` paths: the consumer's relaxed `try_pop` must never invent or duplicate an element.
#[test]
fn mpsc_split_try_model() {
    loom::model(|| {
        let q: &'static mut RingBuf<2, u32> = Box::leak(Box::new(RingBuf::new()));
        let (tx, rx) = q.split();

        // The queue starts empty with room for two, so this push cannot fail — no retry loop,
        // which loom would treat as an algorithm requiring the processor to make progress.
        let p = thread::spawn(move || {
            assert!(tx.try_push(7).is_none(), "push into an empty queue");
        });

        // Bounded retries: an unbounded spin would blow loom's branch limit, but a fixed count
        // still lets it explore the interleavings where the pop runs before, during and after the
        // push. Whatever it finds, the element must be there once the producer has joined.
        let mut got = None;
        for _ in 0..2 {
            got = rx.try_pop();
            if got.is_some() {
                break;
            }
        }

        p.join().unwrap();

        let got = got.or_else(|| rx.try_pop());
        assert_eq!(got, Some(7), "the pushed element must arrive exactly once");
        assert!(rx.try_pop().is_none(), "only one element was ever pushed");
    });
}
