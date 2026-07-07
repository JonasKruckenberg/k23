// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Model-checking the scheduler's handshakes with [`loom`](https://docs.rs/loom).
//!
//! ```sh
//! just loom //lib/heartbeat:heartbeat_loom_tests
//! ```
//!
//! # What is left to check
//!
//! Almost all of the scheduler's shared state lives inside the lock, and is
//! therefore not interesting to loom: the promoted-job list and the idle list are
//! plain data structures that can only be named through the guard. What the lock
//! does *not* cover is the handoff at its edges, and that is what these tests are
//! about:
//!
//! - a worker decides to sleep under the guard, but sleeps only after dropping it
//!   (we cannot hold a spinlock across a `wfi`), and a waker signals only after
//!   dropping it (a signal can be an `ecall`). The window between the two is
//!   covered by nothing but [`Park`]'s token;
//! - a thief publishes a job's result and then unparks its owner, while the owner
//!   is polling that same result.
//!
//! Both are exactly the shape "make a condition true, then record a token" against
//! "check the condition, then sleep", and neither is safe to eyeball.
//!
//! # Scope
//!
//! These tests do **not** drive [`Scheduler`](crate::Scheduler) itself. It locks
//! with `spin::IrqMutex`, and `spin` is not built with `--cfg loom` here, so its
//! atomics are invisible to the model and its spin loop would never yield — a loom
//! model would hang inside it. The lock is stood in for by `loom::sync::Mutex`,
//! which is the same mutual exclusion with a scheduler loom can actually drive.

use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

use crate::{Park, ParkVTable};

/// A [`Park`] whose backend never sleeps — it just yields, which lets loom schedule
/// the other thread anywhere it likes. That is a strictly stronger adversary than a
/// real `wfi`, and a legal backend: [`ParkVTable`] permits spurious returns, and
/// permits not sleeping at all.
fn loom_park() -> Park {
    fn park(_ptr: *const ()) {
        thread::yield_now();
    }
    fn unpark(_ptr: *const ()) {}
    fn drop(_ptr: *const ()) {}

    static LOOM_PARK_VTABLE: ParkVTable = ParkVTable { park, unpark, drop };

    // Safety: `ptr` is never dereferenced by any of the three functions above.
    unsafe { Park::new(core::ptr::null(), &LOOM_PARK_VTABLE) }
}

/// The scheduler's wake protocol, which every park site in the crate is an instance
/// of: a waker makes a *persistent condition* true and then records a token; a
/// sleeper parks and re-checks the condition. **No interleaving may leave the
/// sleeper asleep while the condition is true.**
///
/// Swap the two stores in the waker — token before condition — and loom finds the
/// resulting deadlock in well under a second.
#[test]
fn wake_racing_park_is_never_lost() {
    loom::model(|| {
        let park = Arc::new(loom_park());
        let ready = Arc::new(AtomicBool::new(false));

        let waker = {
            let (park, ready) = (park.clone(), ready.clone());
            thread::spawn(move || {
                // Condition first, token second. This order is the contract, and it
                // is what `TypedJob::execute` does with `is_ready`.
                ready.store(true, Ordering::Release);
                park.unpark();
            })
        };

        // Exactly `Worker::await_shared_job`.
        while !ready.load(Ordering::Acquire) {
            park.park();
        }

        waker.join().unwrap();
    });
}

/// `Worker::main_loop` against `Worker::heartbeat`, which is the one place the lock
/// hands off to the token.
///
/// The worker decides *there is no work* and *registers as idle* together, under the
/// guard — that is the whole reason the scheduler takes a lock at all — but it only
/// sleeps after dropping it. The promoter publishes the job and takes the worker off
/// the idle list together, under the guard, but only signals after dropping it. So
/// the two threads can interleave freely in the gap, and the token is all that keeps
/// the worker from sleeping through the job.
///
/// **A worker that goes to sleep must always be woken again.** Every deadlock this
/// scheduler has had was a violation of that; if one comes back, loom reports a
/// deadlock here rather than a benchmark hanging for ten minutes.
#[test]
fn a_worker_that_registers_as_idle_is_never_left_asleep() {
    /// Stands in for `Synced`. Whether a job is waiting, and whether the worker is
    /// linked into the idle list.
    struct Synced {
        shared: bool,
        idle: bool,
    }

    loom::model(|| {
        let synced = Arc::new(Mutex::new(Synced {
            shared: false,
            idle: false,
        }));
        let park = Arc::new(loom_park());
        let ran = Arc::new(AtomicBool::new(false));

        // `Worker::heartbeat`: publish the job and pop an idle worker under the
        // guard; unpark it *outside* the guard.
        let promoter = {
            let (synced, park) = (synced.clone(), park.clone());
            thread::spawn(move || {
                let to_wake = {
                    let mut synced = synced.lock().unwrap();
                    synced.shared = true;
                    core::mem::replace(&mut synced.idle, false)
                };

                if to_wake {
                    park.unpark();
                }
            })
        };

        // `Worker::main_loop`.
        loop {
            let work = {
                let mut synced = synced.lock().unwrap();
                if synced.shared {
                    // Take the job, and take ourselves back out of the idle list —
                    // `main_loop`'s `links.is_linked()` cleanup.
                    synced.shared = false;
                    synced.idle = false;
                    true
                } else {
                    synced.idle = true;
                    false
                }
            };

            if work {
                ran.store(true, Ordering::Release);
                break;
            }

            // The guard is gone: from here to the token being consumed, nothing but
            // `Park` is holding this together.
            park.park();
        }

        promoter.join().unwrap();
        assert!(
            ran.load(Ordering::Acquire),
            "the worker never ran the shared job"
        );
    });
}

/// Two wakers racing: the token hands exactly one of them the "you woke it" edge, so
/// exactly one signal is issued. Under-signalling would strand a parked hart;
/// over-signalling is only a wasted IPI. The token must survive either way.
#[test]
fn concurrent_unparks_record_exactly_one_token() {
    loom::model(|| {
        let park = Arc::new(loom_park());
        let woke = Arc::new(AtomicUsize::new(0));

        let wakers: Vec<_> = (0..2)
            .map(|_| {
                let (park, woke) = (park.clone(), woke.clone());
                thread::spawn(move || {
                    if park.unpark() {
                        woke.fetch_add(1, Ordering::AcqRel);
                    }
                })
            })
            .collect();

        for w in wakers {
            w.join().unwrap();
        }

        assert_eq!(
            woke.load(Ordering::Acquire),
            1,
            "the token must be claimed exactly once"
        );

        // And the surviving token must still wake the parker.
        park.park();
    });
}

/// A worker can park *inside* a job it is helping to execute: `await_shared_job` runs
/// stolen jobs, and those jobs fork and join in turn. So an inner park loop can
/// consume a token that was meant for an outer one. That must strand nobody — the
/// *condition* is the state, and the token is only ever a kick.
#[test]
fn a_token_stolen_by_a_nested_loop_strands_nobody() {
    loom::model(|| {
        let park = Arc::new(loom_park());
        let outer = Arc::new(AtomicBool::new(false));
        let inner = Arc::new(AtomicBool::new(false));

        let wakers: Vec<_> = [outer.clone(), inner.clone()]
            .into_iter()
            .map(|flag| {
                let park = park.clone();
                thread::spawn(move || {
                    flag.store(true, Ordering::Release);
                    park.unpark();
                })
            })
            .collect();

        // The inner loop drains tokens meant for either condition...
        while !inner.load(Ordering::Acquire) {
            park.park();
        }
        // ...and the outer one must still make progress.
        while !outer.load(Ordering::Acquire) {
            park.park();
        }

        for w in wakers {
            w.join().unwrap();
        }
    });
}

/// The join handshake, end to end: a thief publishes a job's result, sets `is_ready`,
/// and unparks the owner; the owner waits on `is_ready` and then reads the result.
/// The result must always be visible — this is what the `Release` on `is_ready` and
/// the `Acquire` in `await_shared_job` are for.
#[test]
fn a_stolen_jobs_result_is_always_visible_to_the_joiner() {
    loom::model(|| {
        let park = Arc::new(loom_park());
        let is_ready = Arc::new(AtomicBool::new(false));
        // Stands in for the `Stage` union the thief writes the result into.
        let result = Arc::new(AtomicUsize::new(0));

        let thief = {
            let (park, is_ready, result) = (park.clone(), is_ready.clone(), result.clone());
            thread::spawn(move || {
                result.store(42, Ordering::Relaxed);
                is_ready.store(true, Ordering::Release);
                park.unpark();
            })
        };

        while !is_ready.load(Ordering::Acquire) {
            park.park();
        }
        assert_eq!(
            result.load(Ordering::Relaxed),
            42,
            "the joiner read a result the thief had not published yet"
        );

        thief.join().unwrap();
    });
}
