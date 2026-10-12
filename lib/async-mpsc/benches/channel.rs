// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! FIFO against unordered, over the same message counts.
//!
//! Messages come from a pool leaked once per benchmark, so no allocator traffic lands inside a
//! timed region: what is measured is the queue and its wakeups, nothing else. Every message is
//! drained before an iteration ends, so the pool can be re-sent.

use std::hint::black_box;
use std::ptr::NonNull;
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

use async_mpsc::{fifo, unordered};
use cordyceps::{mpsc_queue, stack, Linked};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

/// Messages per throughput iteration.
const MSGS: usize = 100_000;

/// Messages per drain iteration.
const DRAIN: usize = 10_000;

/// Thread counts sampled, capped at the machine's parallelism.
const THREAD_COUNTS: [usize; 4] = [1, 2, 4, 8];

macro_rules! message {
    ($name:ident, $links:path) => {
        struct $name {
            links: $links,
            value: u32,
        }

        // Safety: handles are `&'static` references to leaked messages, which are never freed and
        // never moved, and `links` is a live field of every one of them.
        unsafe impl Linked<$links> for $name {
            type Handle = &'static Self;

            fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
                NonNull::from(handle)
            }

            unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
                // Safety: every pointer in a queue came from `into_ptr`, so it points at a leaked
                // message that outlives the channel.
                unsafe { &*ptr.as_ptr() }
            }

            unsafe fn links(target: NonNull<Self>) -> NonNull<$links> {
                // Safety: `target` points at a live message, so the projection is in bounds.
                unsafe { NonNull::new_unchecked(&raw mut (*target.as_ptr()).links) }
            }
        }

        /// Leaks `count` messages, so sending one never touches the allocator.
        fn $name(count: usize) -> Vec<&'static $name> {
            (0..count)
                .map(|i| {
                    &*Box::leak(Box::new($name {
                        links: <$links>::new(),
                        value: u32::try_from(i).expect("pool fits in u32"),
                    }))
                })
                .collect()
        }
    };
}

message!(FifoMsg, mpsc_queue::Links<FifoMsg>);
message!(StackMsg, stack::Links<StackMsg>);

static FIFO_STUB: FifoMsg = FifoMsg {
    links: mpsc_queue::Links::new_stub(),
    value: 0,
};

fn fifo_channel() -> fifo::Channel<FifoMsg> {
    // Safety: the stub is used by one channel at a time — benchmarks run one after another — is
    // never sent, and is `'static`.
    unsafe { fifo::Channel::new_with_static_stub(&FIFO_STUB) }
}

// -- throughput -----------------------------------------------------------------------------------

fn fifo_throughput(producers: usize, pool: &[Vec<&'static FifoMsg>]) -> Duration {
    let channel = fifo_channel();
    let mut rx = channel.receiver().expect("first receiver");
    let barrier = Barrier::new(producers + 2);
    let (channel, barrier) = (&channel, &barrier);

    thread::scope(|scope| {
        for msgs in pool.iter().take(producers) {
            scope.spawn(move || {
                barrier.wait();
                for msg in msgs {
                    channel.send(msg);
                }
            });
        }

        let total = pool.iter().take(producers).map(Vec::len).sum::<usize>();
        scope.spawn(move || {
            barrier.wait();
            let mut count = 0;
            while count < total {
                if let Ok(msg) = rx.try_recv() {
                    black_box(msg.value);
                    count += 1;
                }
            }
        });

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn unordered_throughput(producers: usize, pool: &[Vec<&'static StackMsg>]) -> Duration {
    let channel = unordered::Channel::<StackMsg>::new();
    let mut rx = channel.receiver().expect("first receiver");
    let barrier = Barrier::new(producers + 2);
    let (channel, barrier) = (&channel, &barrier);

    thread::scope(|scope| {
        for msgs in pool.iter().take(producers) {
            scope.spawn(move || {
                let tx = channel.sender();
                barrier.wait();
                for msg in msgs {
                    tx.send(msg);
                }
            });
        }

        let total = pool.iter().take(producers).map(Vec::len).sum::<usize>();
        scope.spawn(move || {
            barrier.wait();
            let mut count = 0;
            while count < total {
                if let Ok(msg) = rx.try_recv() {
                    black_box(msg.value);
                    count += 1;
                }
            }
        });

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn bench_throughput(c: &mut Criterion) {
    let mut g = c.benchmark_group("throughput");
    g.throughput(Throughput::Elements(MSGS as u64));

    // Producers and the consumer should land on distinct cores, so cap at half the parallelism.
    let max = thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .div_ceil(2)
        .max(1);

    for n in THREAD_COUNTS {
        if n > max {
            break;
        }

        let fifo_pool: Vec<_> = (0..n).map(|_| FifoMsg(MSGS / n)).collect();
        let stack_pool: Vec<_> = (0..n).map(|_| StackMsg(MSGS / n)).collect();

        g.bench_function(format!("fifo/{n}p1c"), |b| {
            b.iter_custom(|iters| (0..iters).map(|_| fifo_throughput(n, &fifo_pool)).sum());
        });
        g.bench_function(format!("unordered/{n}p1c"), |b| {
            b.iter_custom(|iters| {
                (0..iters)
                    .map(|_| unordered_throughput(n, &stack_pool))
                    .sum()
            });
        });
    }

    g.finish();
}

// -- uncontended send -----------------------------------------------------------------------------
//
// One thread, no receiver draining: isolates the send path, including the wake that `fifo` pays on
// every message and `unordered` pays only when the channel was empty.

fn bench_send(c: &mut Criterion) {
    let mut g = c.benchmark_group("send");
    g.throughput(Throughput::Elements(DRAIN as u64));

    let fifo_pool = FifoMsg(DRAIN);
    g.bench_function("fifo", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    let channel = fifo_channel();
                    let mut rx = channel.receiver().expect("first receiver");
                    let start = Instant::now();
                    for msg in &fifo_pool {
                        channel.send(msg);
                    }
                    let elapsed = start.elapsed();
                    while rx.try_recv().is_ok() {}
                    elapsed
                })
                .sum()
        });
    });

    let stack_pool = StackMsg(DRAIN);
    g.bench_function("unordered", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    let channel = unordered::Channel::<StackMsg>::new();
                    let mut rx = channel.receiver().expect("first receiver");
                    let tx = channel.sender();
                    let start = Instant::now();
                    for msg in &stack_pool {
                        tx.send(msg);
                    }
                    let elapsed = start.elapsed();
                    while rx.try_recv().is_ok() {}
                    elapsed
                })
                .sum()
        });
    });

    g.finish();
}

// -- drain ----------------------------------------------------------------------------------------
//
// Everything queued up front, then drained by one thread: isolates the receive path, where
// `unordered` takes the whole queue in one swap and `fifo` walks it a node at a time.

fn bench_drain(c: &mut Criterion) {
    let mut g = c.benchmark_group("drain");
    g.throughput(Throughput::Elements(DRAIN as u64));

    let fifo_pool = FifoMsg(DRAIN);
    g.bench_function("fifo", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    let channel = fifo_channel();
                    let mut rx = channel.receiver().expect("first receiver");
                    for msg in &fifo_pool {
                        channel.send(msg);
                    }

                    let start = Instant::now();
                    while let Ok(msg) = rx.try_recv() {
                        black_box(msg.value);
                    }
                    start.elapsed()
                })
                .sum()
        });
    });

    let stack_pool = StackMsg(DRAIN);
    g.bench_function("unordered", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    let channel = unordered::Channel::<StackMsg>::new();
                    let mut rx = channel.receiver().expect("first receiver");
                    let tx = channel.sender();
                    for msg in &stack_pool {
                        tx.send(msg);
                    }

                    let start = Instant::now();
                    while let Ok(msg) = rx.try_recv() {
                        black_box(msg.value);
                    }
                    start.elapsed()
                })
                .sum()
        });
    });

    g.bench_function("unordered/batch", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| {
                    let channel = unordered::Channel::<StackMsg>::new();
                    let mut rx = channel.receiver().expect("first receiver");
                    let tx = channel.sender();
                    for msg in &stack_pool {
                        tx.send(msg);
                    }

                    let start = Instant::now();
                    for msg in rx.try_recv_all() {
                        black_box(msg.value);
                    }
                    start.elapsed()
                })
                .sum()
        });
    });

    g.finish();
}

criterion_group!(benches, bench_throughput, bench_send, bench_drain);
criterion_main!(benches);
