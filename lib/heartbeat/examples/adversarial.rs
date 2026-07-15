// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Adversarial benchmarks for the scheduler's known hard cases:
//!
//! 1. **Idle-transition latency** (`short`): repeated sums of a tree small
//!    enough that stolen subtrees finish in well under a heartbeat interval,
//!    so workers constantly finish, go idle, and need re-feeding. A steal only
//!    becomes available when a victim's beat re-advertises, so refeed latency
//!    is bounded by the (staggered) tick.
//!
//! 2. **Skew** (`skewed`): one sum of a heavily lopsided tree (90/10 split),
//!    so the promotable jobs at any moment differ in size by orders of
//!    magnitude and *which* job a thief gets matters as much as getting one.
//!
//! Sizing `short` down (`HEARTBEAT_SHORT_N=1000`) turns it into the tiny-scope
//! case: scopes shorter than a tick, where promotion is pure overhead.
//!
//! ```sh
//! buck2 run //lib/heartbeat:heartbeat-adversarial -m release -- [short|skewed|all] [threads...]
//! HEARTBEAT_SHORT_N=300000 HEARTBEAT_SKEWED_N=10000000  # size overrides
//! ```

use std::hint::black_box;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use heartbeat::{Job, Park, ParkVTable, Scheduler, Scope, Worker};

/// Interval at which each worker is handed a heartbeat.
const HEARTBEAT_INTERVAL: Duration = Duration::from_micros(100);

struct Node {
    value: i64,
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

impl Node {
    /// Pre-order allocation with the pivot at `num/den` of the range: the left
    /// subtree holds that fraction of the values. `1/2` is the balanced tree;
    /// `9/10` makes the *forked* (right) subtrees hold ~10% of whatever
    /// remains, so queued job sizes fall off geometrically along the spine.
    fn lopsided(from: i64, to: i64, num: i64, den: i64) -> Box<Node> {
        let value = from + (to - from) * num / den;
        let mut node = Box::new(Node {
            value,
            left: None,
            right: None,
        });
        if value > from {
            node.left = Some(Self::lopsided(from, value - 1, num, den));
        }
        if value < to {
            node.right = Some(Self::lopsided(value + 1, to, num, den));
        }
        node
    }

    fn sum(&self) -> i64 {
        let mut res = self.value;
        if let Some(child) = &self.left {
            res += child.sum();
        }
        if let Some(child) = &self.right {
            res += child.sum();
        }
        res
    }
}

fn sum(mut s: Scope<'_, '_>, node: &Node) -> i64 {
    // Loaded before the forks so the load's latency hides behind the subtree
    // walk (P3); see `examples/binary_tree.rs`.
    let value = node.value;
    match (&node.left, &node.right) {
        (Some(left), Some(right)) => {
            let (l, r) = s.fork_join(|s| sum(s, left), |s| sum(s, right));
            value + l + r
        }
        (Some(child), None) | (None, Some(child)) => value + sum(s, child),
        (None, None) => value,
    }
}

/// A [`Park`] backed by `std::thread::park`; same as the other examples.
fn std_park() -> Park {
    fn park(ptr: *const ()) {
        let me = ManuallyDrop::new(unsafe { Arc::from_raw(ptr.cast::<thread::Thread>()) });
        debug_assert_eq!(me.id(), thread::current().id());

        thread::park();
    }
    fn unpark(ptr: *const ()) {
        let me = ManuallyDrop::new(unsafe { Arc::from_raw(ptr.cast::<thread::Thread>()) });
        me.unpark();
    }
    fn drop(ptr: *const ()) {
        unsafe { Arc::decrement_strong_count(ptr.cast::<thread::Thread>()) };
    }

    static STD_PARK_VTABLE: ParkVTable = ParkVTable { park, unpark, drop };

    let state = Arc::new(thread::current());
    // Safety: `std::thread` park/unpark is a sticky permit, and the `Arc` keeps
    // the handle alive until `drop` releases it.
    unsafe { Park::new(Arc::into_raw(state).cast::<()>(), &STD_PARK_VTABLE) }
}

fn bench(name: &str, n: i64, expected: i64, mut f: impl FnMut() -> i64) {
    let warmup = Duration::from_secs_f64(
        std::env::var("SPICE_WARMUP_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3.0),
    );
    let n_samples: usize = std::env::var("SPICE_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    assert_eq!(f(), expected);

    let start = Instant::now();
    while start.elapsed() < warmup {
        std::hint::black_box(f());
    }

    let mut samples = Vec::with_capacity(n_samples);
    for _ in 0..n_samples {
        let t0 = Instant::now();
        let got = std::hint::black_box(f());
        let elapsed = t0.elapsed().as_nanos() as f64;
        assert_eq!(got, expected);
        samples.push(elapsed / n as f64);
    }

    let min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!("{name:>24}:  Min: {min:.4} ns  Mean: {mean:.4} ns  Max: {max:.4} ns");
}

/// Run `f` with the full scheduler wiring — `threads - 1` main-loop workers
/// plus the staggered heartbeat timer — handing it the requesting worker.
fn with_scheduler(threads: usize, f: impl FnOnce(&mut Worker<'_>)) {
    let sched = Scheduler::new();
    let flags: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    thread::scope(|scope| {
        let idle: Vec<_> = (1..threads)
            .map(|_| {
                scope.spawn(|| {
                    let stub = Job::stub();
                    let mut worker = Worker::new(&sched, std_park(), &stub);
                    flags
                        .lock()
                        .unwrap()
                        .push(&raw const *worker.heartbeat_flag() as usize);

                    let _ = worker.main_loop();
                })
            })
            .collect();

        // Staggered round-robin timer, as in `examples/binary_tree.rs` (P14).
        let timer = scope.spawn(|| {
            let mut i = 0;
            while !sched.is_stopping() {
                let mut to_sleep = HEARTBEAT_INTERVAL;
                {
                    let flags = flags.lock().unwrap();
                    if !flags.is_empty() {
                        i %= flags.len();
                        // Safety: every worker outlives the joins below, and this
                        // loop ends once the scheduler is stopping — which is also
                        // the only way a worker returns.
                        unsafe {
                            (*(flags[i] as *const AtomicBool)).store(true, Ordering::Relaxed);
                        };
                        i += 1;
                        to_sleep /= flags.len() as u32;
                    }
                }

                thread::sleep(to_sleep);
            }
        });

        let stub = Job::stub();
        let mut worker = Worker::new(&sched, std_park(), &stub);
        flags
            .lock()
            .unwrap()
            .push(&raw const *worker.heartbeat_flag() as usize);

        f(&mut worker);

        sched.stop();
        for hart in idle {
            hart.join().unwrap();
        }
        timer.join().unwrap();
    });
}

fn env_size(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let which = args.next().unwrap_or_else(|| "all".to_string());
    let threads: Vec<usize> = args.map(|a| a.parse().expect("bad thread count")).collect();
    let threads = if threads.is_empty() {
        vec![1, 2, 4, 8]
    } else {
        threads
    };

    if which == "short" || which == "all" {
        // Small enough that a whole sum is only a few heartbeat intervals long:
        // stolen subtrees are *short*, so workers cycle through idle constantly.
        let n = env_size("HEARTBEAT_SHORT_N", 300_000);
        let expected = n * (n + 1) / 2;
        println!("== short: repeated sums of a balanced {n}-node tree");
        let root = Node::lopsided(1, n, 1, 2);

        for &t in &threads {
            with_scheduler(t, |worker| {
                bench(&format!("short {t} thr"), n, expected, || {
                    worker.scope(|s| sum(s, black_box(&root)))
                });
            });
        }
    }

    if which == "skewed" || which == "all" {
        // 90/10 split: the forked jobs queued along the spine differ in size by
        // orders of magnitude, so *which* promoted job a thief ends up with
        // matters as much as getting one at all.
        let n = env_size("HEARTBEAT_SKEWED_N", 10_000_000);
        let expected = n * (n + 1) / 2;
        println!("== skewed: one sum of a 90/10 lopsided {n}-node tree");
        let root = Node::lopsided(1, n, 9, 10);

        for &t in &threads {
            with_scheduler(t, |worker| {
                bench(&format!("skewed {t} thr"), n, expected, || {
                    worker.scope(|s| sum(s, black_box(&root)))
                });
            });
        }
    }
}
