// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Criterion port of Spice's `examples/zig-parallel-example`: build a perfectly
//! balanced binary tree and time summing all its values, sequentially
//! (baseline) and through the heartbeat scheduler at various hart counts.
//!
//! The scheduler is wired up the way it will be on real hardware: `harts - 1`
//! workers sitting in `main_loop` waiting for work to be shared with them, the
//! benchmark thread itself as the hart that asks for the sum, and one thread
//! standing in for the per-hart timer interrupt that delivers the heartbeats.
//! Reported times are per whole-tree sum; divide by the node count for ns/node.
//!
//! The 10M-node tree needs roughly 400 MB of RAM (upstream's 100M needs ~4 GB).
//! Override the defaults with env vars, e.g.:
//!
//! ```sh
//! HEARTBEAT_BENCH_SIZES=1000,100000000 HEARTBEAT_BENCH_HARTS=1,2,4 just benchmark //lib/heartbeat:heartbeat_benchmarks
//! ```

#![expect(clippy::undocumented_unsafe_blocks, reason = "its fine for benchmarks")]

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
    /// Parent-first (pre-order) allocation, like the Zig example.
    fn balanced(from: i64, to: i64) -> Box<Node> {
        let value = from + (to - from) / 2;
        let mut node = Box::new(Node {
            value,
            left: None,
            right: None,
        });
        if value > from {
            node.left = Some(Self::balanced(from, value - 1));
        }
        if value < to {
            node.right = Some(Self::balanced(value + 1, to));
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

/// The parallel sum, structured exactly like `sum` in the Zig example: fork the
/// right child, run the left inline, join (running the right inline if it was
/// never stolen).
fn sum(mut s: Scope<'_, '_>, node: &Node) -> i64 {
    // Load the value *before* forking: written after the joins, LLVM sinks
    // this load below both recursive calls, putting its latency on every
    // frame's critical path — worth ~20% on this whole benchmark. Loaded here,
    // it waits out the subtree walk in a callee-saved register (this is where
    // the Zig example gets its `ldp val, left` from).
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

/// A [`Park`] backed by `std::thread::park`. The library itself is `no_std` and
/// knows nothing about threads, so the host supplies this the same way the
/// kernel will supply a WFI-based one.
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
    unsafe { Park::new(Arc::into_raw(state).cast::<()>(), &STD_PARK_VTABLE) }
}

fn bench(name: &str, n: i64, mut f: impl FnMut() -> i64) {
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

    let expected = n * (n + 1) / 2;
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
    println!("{name:>16}:  Min: {min:.4} ns  Mean: {mean:.4} ns  Max: {max:.4} ns");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: i64 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(10_000_000);
    let threads: Vec<usize> = args.map(|a| a.parse().expect("bad thread count")).collect();
    let threads = if threads.is_empty() {
        vec![1, 2, 4, 8]
    } else {
        threads
    };

    println!("building balanced tree with {n} nodes...");
    let root = Node::balanced(1, n);

    bench("Baseline", n, || std::hint::black_box(&*root).sum());

    for &num_threads in &threads {
        let sched = Scheduler::new();

        // On real hardware each hart's timer ISR sets its own CPU-local heartbeat
        // flag. Here one thread plays the timer for every hart, so each worker has
        // to publish the address of its flag first.
        let flags: Mutex<Vec<usize>> = Mutex::new(Vec::new());

        thread::scope(|scope| {
            // The idle harts, running whatever gets shared with them.
            let idle: Vec<_> = (1..num_threads)
                .map(|_| {
                    scope.spawn(|| {
                        let stub = Job::stub();
                        let mut worker = Worker::new(&sched, std_park(), &stub);
                        flags
                            .lock()
                            .unwrap()
                            .push(&raw const *worker.heartbeat_flag() as usize);

                        worker.main_loop();
                    })
                })
                .collect();

            // The "timer interrupt". Round-robin, one worker per tick, like the
            // Zig reference's `heartbeatWorker`: every worker still sees a
            // heartbeat each `HEARTBEAT_INTERVAL`, but *staggered* — setting
            // every flag at once makes all workers promote simultaneously and
            // stampede the scheduler lock.
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

            // The hart asking for the tree sum. The others only get anything to do
            // because this one's heartbeat shares its jobs.
            let stub = Job::stub();
            let mut worker = Worker::new(&sched, std_park(), &stub);
            flags
                .lock()
                .unwrap()
                .push(&raw const *worker.heartbeat_flag() as usize);

            assert_eq!(worker.scope(|s| sum(s, &root)), root.sum());

            bench(&format!("Spice {num_threads} thr"), n, || {
                worker.scope(|s| sum(s, std::hint::black_box(&root)))
            });

            sched.stop();

            // A worker may not be dropped while another hart can still reach it: a
            // thief unparks its owner *after* publishing the result. Join everyone
            // before `worker` goes out of scope. See `Worker::new`.
            for hart in idle {
                hart.join().unwrap();
            }
            timer.join().unwrap();
        });
    }
}
