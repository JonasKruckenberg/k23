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
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main};
use heartbeat::{Job, Park, ParkVTable, Scheduler, Scope, Worker};

/// Interval at which each worker is handed a heartbeat.
const HEARTBEAT_INTERVAL: Duration = Duration::from_micros(100);

pub struct Node {
    value: i64,
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

impl Node {
    /// Perfectly balanced tree holding the values `from..=to`.
    ///
    /// Allocates the parent *before* its children (pre-order), exactly like the
    /// Zig example's `balancedTree`. This matters: it lays the tree out nearly
    /// sequentially in memory, and the walk order then matches the allocation
    /// order. (Upstream's own Rust example builds children first, which costs it
    /// roughly 2x on tree-walk speed on this author's box.)
    pub fn make_balanced_tree(from: i64, to: i64) -> Box<Node> {
        let value = from + (to - from) / 2;
        let mut node = Box::new(Node {
            value,
            left: None,
            right: None,
        });
        if value > from {
            node.left = Some(Self::make_balanced_tree(from, value - 1));
        }
        if value < to {
            node.right = Some(Self::make_balanced_tree(value + 1, to));
        }
        node
    }

    /// Sequential baseline.
    pub fn sum(&self) -> i64 {
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
    // frame's critical path — worth ~20% on this whole benchmark.
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

/// Time the tree sum on `harts` harts: this thread plus `harts - 1` workers.
fn bench_harts(group: &mut BenchmarkGroup<'_, WallTime>, root: &Node, harts: usize, expected: i64) {
    let sched = Scheduler::new();

    // On real hardware each hart's timer ISR sets its own CPU-local heartbeat
    // flag. Here one thread plays the timer for every hart, so each worker has
    // to publish the address of its flag first.
    let flags: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    thread::scope(|scope| {
        // The idle harts, running whatever gets shared with them.
        let idle: Vec<_> = (1..harts)
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

        // The "timer interrupt". Round-robin, one worker per tick, like the Zig
        // reference's `heartbeatWorker`: every worker still sees a heartbeat
        // each `HEARTBEAT_INTERVAL`, but *staggered* — setting every flag at
        // once makes all workers promote simultaneously and stampede the
        // scheduler lock.
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

        assert_eq!(worker.scope(|s| sum(s, root)), expected);

        // `black_box` on the input: `sum` is a pure function of read-only
        // memory, and LLVM will otherwise merge the repeated calls across
        // iterations, reporting picoseconds.
        group.bench_function(BenchmarkId::new("heartbeat", harts), |b| {
            b.iter(|| worker.scope(|s| sum(s, black_box(root))));
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

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    match std::env::var(name) {
        Ok(s) => s
            .split(',')
            .map(|v| {
                v.trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("bad {name}: {v:?}"))
            })
            .collect(),
        Err(_) => default.to_vec(),
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    let sizes = env_list("HEARTBEAT_BENCH_SIZES", &[1000, 10_000_000]);
    let harts = env_list("HEARTBEAT_BENCH_HARTS", &[1, 2, 4, 8, 16, 32]);

    for &n in &sizes {
        let mut group = c.benchmark_group(format!("tree-sum-{n}"));
        group.sample_size(50);

        let root = Node::make_balanced_tree(1, n as i64);

        // Correctness check outside the timed region: 1 + 2 + ... + n.
        let expected = (n as i64) * (n as i64 + 1) / 2;
        assert_eq!(root.sum(), expected);

        group.bench_function(BenchmarkId::new("Baseline", 1), |b| {
            b.iter(|| black_box(root.as_ref()).sum());
        });

        for &harts in &harts {
            bench_harts(&mut group, &root, harts, expected);
        }

        group.finish();
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
