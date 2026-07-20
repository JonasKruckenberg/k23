// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::hint::black_box;
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ringbuf::RingBuf;

/// Queue capacity.
const CAP: usize = 1024;

/// Messages per throughput iteration.
const MSGS: u32 = 100_000;

/// Thread counts sampled, capped at the machine's parallelism.
const THREAD_COUNTS: [usize; 4] = [1, 2, 4, 8];

/// Ping-pong capacity and round count.
const PING_PONG_CAP: usize = 8;
const PING_PONG_ROUNDS: u32 = 10_000;

/// Thread and capacity counts are `usize` at the call sites but the message counters are `u32`.
fn fits_u32(n: usize) -> u32 {
    u32::try_from(n).expect("count fits in u32")
}

// -- throughput -----------------------------------------------------------------------------------

fn mpmc_throughput(producers: usize, consumers: usize) -> Duration {
    let q = RingBuf::<CAP, u32>::new();
    let (np, nc) = (fits_u32(producers), fits_u32(consumers));
    let per_producer = MSGS / np;
    let per_consumer = per_producer * np / nc;
    let barrier = Barrier::new(producers + consumers + 1);

    thread::scope(|s| {
        for _ in 0..producers {
            let (q, b) = (&q, &barrier);
            s.spawn(move || {
                b.wait();
                for i in 0..per_producer {
                    q.push(i);
                }
            });
        }
        for _ in 0..consumers {
            let (q, b) = (&q, &barrier);
            s.spawn(move || {
                b.wait();
                let mut sum = 0u64;
                for _ in 0..per_consumer {
                    sum = sum.wrapping_add(u64::from(q.pop()));
                }
                black_box(sum);
            });
        }

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn mpsc_throughput(producers: usize) -> Duration {
    let mut q = RingBuf::<CAP, u32>::new();
    let (tx, rx) = q.split();
    let np = fits_u32(producers);
    let per_producer = MSGS / np;
    let total = per_producer * np;
    let barrier = Barrier::new(producers + 2);

    thread::scope(|s| {
        for _ in 0..producers {
            let (tx, b) = (tx.clone(), &barrier);
            s.spawn(move || {
                b.wait();
                for i in 0..per_producer {
                    tx.push(i);
                }
            });
        }

        let b = &barrier;
        s.spawn(move || {
            b.wait();
            let mut sum = 0u64;
            for _ in 0..total {
                sum = sum.wrapping_add(u64::from(rx.pop()));
            }
            black_box(sum);
        });

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn spsc_throughput() -> Duration {
    let mut q = RingBuf::<CAP, u32, true>::new();
    let (tx, rx) = q.split();
    let barrier = Barrier::new(3);

    thread::scope(|s| {
        let b = &barrier;
        s.spawn(move || {
            b.wait();
            for i in 0..MSGS {
                tx.push(i);
            }
        });

        let b = &barrier;
        s.spawn(move || {
            b.wait();
            let mut sum = 0u64;
            for _ in 0..MSGS {
                sum = sum.wrapping_add(u64::from(rx.pop()));
            }
            black_box(sum);
        });

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn bench_throughput(c: &mut Criterion) {
    let mut g = c.benchmark_group("throughput");
    g.throughput(Throughput::Elements(u64::from(MSGS)));

    // Producers and consumers should land on distinct cores, so cap at half the parallelism.
    let max = thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .div_ceil(2)
        .max(1);

    g.bench_function("spsc/1p1c", |b| {
        b.iter_custom(|iters| (0..iters).map(|_| spsc_throughput()).sum());
    });

    for n in THREAD_COUNTS {
        if n > max {
            break;
        }

        g.bench_function(format!("mpmc/{n}p{n}c"), |b| {
            b.iter_custom(|iters| (0..iters).map(|_| mpmc_throughput(n, n)).sum());
        });
        g.bench_function(format!("mpsc/{n}p1c"), |b| {
            b.iter_custom(|iters| (0..iters).map(|_| mpsc_throughput(n)).sum());
        });
        // `mpmc/1p1c` is already covered by the `NpNc` entry above.
        if n > 1 {
            g.bench_function(format!("mpmc/{n}p1c"), |b| {
                b.iter_custom(|iters| (0..iters).map(|_| mpmc_throughput(n, 1)).sum());
            });
        }
    }

    g.finish();
}

// -- ping-pong latency ----------------------------------------------------------------------------

fn mpmc_ping_pong() -> Duration {
    let there = RingBuf::<PING_PONG_CAP, u32>::new();
    let back = RingBuf::<PING_PONG_CAP, u32>::new();
    let barrier = Barrier::new(3);
    // Borrowed once here rather than per closure: re-binding inside the scope would shadow these
    // with references that do not outlive the spawned threads.
    let (there, back, barrier) = (&there, &back, &barrier);

    thread::scope(|s| {
        s.spawn(move || {
            barrier.wait();
            for _ in 0..PING_PONG_ROUNDS {
                let n = there.pop();
                back.push(n);
            }
        });

        s.spawn(move || {
            barrier.wait();
            for i in 0..PING_PONG_ROUNDS {
                there.push(i);
                black_box(back.pop());
            }
        });

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn spsc_ping_pong() -> Duration {
    let mut there = RingBuf::<PING_PONG_CAP, u32, true>::new();
    let mut back = RingBuf::<PING_PONG_CAP, u32, true>::new();
    let (there_tx, there_rx) = there.split();
    let (back_tx, back_rx) = back.split();
    let barrier = Barrier::new(3);

    thread::scope(|s| {
        let b = &barrier;
        s.spawn(move || {
            b.wait();
            for _ in 0..PING_PONG_ROUNDS {
                let n = there_rx.pop();
                back_tx.push(n);
            }
        });

        let b = &barrier;
        s.spawn(move || {
            b.wait();
            for i in 0..PING_PONG_ROUNDS {
                there_tx.push(i);
                black_box(back_rx.pop());
            }
        });

        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn bench_ping_pong(c: &mut Criterion) {
    let mut g = c.benchmark_group("ping_pong");
    g.throughput(Throughput::Elements(u64::from(PING_PONG_ROUNDS)));

    g.bench_function("spsc", |b| {
        b.iter_custom(|iters| (0..iters).map(|_| spsc_ping_pong()).sum());
    });
    g.bench_function("mpmc", |b| {
        b.iter_custom(|iters| (0..iters).map(|_| mpmc_ping_pong()).sum());
    });

    g.finish();
}

// -- uncontended baseline -------------------------------------------------------------------------

fn bench_uncontended(c: &mut Criterion) {
    let mut g = c.benchmark_group("uncontended");

    g.throughput(Throughput::Elements(1));
    g.bench_function("push_pop", |b| {
        let q = RingBuf::<CAP, u32>::new();
        b.iter(|| {
            q.push(black_box(1));
            black_box(q.pop());
        });
    });

    // Fills and drains the whole queue, so consecutive slots are touched in sequence.
    g.throughput(Throughput::Elements(CAP as u64));
    g.bench_function("fill_drain", |b| {
        let q = RingBuf::<CAP, u32>::new();
        b.iter(|| {
            for i in 0..fits_u32(CAP) {
                q.push(black_box(i));
            }
            for _ in 0..CAP {
                black_box(q.pop());
            }
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_throughput,
    bench_ping_pong,
    bench_uncontended
);
criterion_main!(benches);
