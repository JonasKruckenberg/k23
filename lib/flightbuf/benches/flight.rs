// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Prices the three claims the ring is built on:
//!
//! - `append` — the hot path, which is supposed to hold no atomics at all.
//! - `seal` — the cold path, divided by records-per-chunk to get its amortized share.
//! - `read` — the per-chunk cost, which under an overwrite ring is a whole-chunk copy and so scales
//!   with the chunk size rather than being free. That is the price of never holding the producer up.
//!
//! `append` hands the clock back only between batches, so no seal lands inside a measurement.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flightbuf::FlightBuf;

/// Record sizes swept by `append`.
const SIZES: [usize; 5] = [8, 16, 32, 64, 256];

/// Deliberately oversized: a batch has to be long relative to the clock, and 64 KiB holds at least
/// 256 records at every size swept.
const BIG_CHUNK: usize = 64 * 1024;

/// Small chunks, so a record fills one exactly and every reserve has to seal.
const CHUNKS: usize = 256;
const CHUNK: usize = 64;

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).expect("count fits in u64")
}

fn bench_append(c: &mut Criterion) {
    let mut g = c.benchmark_group("append");

    for size in SIZES {
        g.throughput(Throughput::Bytes(as_u64(size)));
        g.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            let ring = Box::new(FlightBuf::<2, BIG_CHUNK>::new());
            // Safety: the only producer in this benchmark.
            let mut producer = unsafe { ring.producer() };
            let record = vec![0xa5u8; size];

            b.iter_custom(|iters| {
                // A batch fills the open chunk exactly, so no seal lands inside a timed region.
                let per_chunk = as_u64(BIG_CHUNK / size);
                let mut elapsed = Duration::ZERO;
                let mut left = iters;

                while left > 0 {
                    let batch = left.min(per_chunk);

                    let start = Instant::now();
                    for _ in 0..batch {
                        producer.reserve(size, |bytes| bytes.copy_from_slice(black_box(&record)));
                    }
                    elapsed += start.elapsed();

                    left -= batch;
                    // Untimed: empty the open chunk so the next batch starts with a whole one.
                    producer.flush();
                }

                elapsed
            });
        });
    }

    g.finish();
}

/// One iteration is one sealed chunk. Divide by the records a real chunk holds for the share a
/// single record carries. No reader runs, which is also the steady state a lapped drainer leaves.
fn bench_seal(c: &mut Criterion) {
    let mut g = c.benchmark_group("seal");
    g.throughput(Throughput::Elements(1));

    g.bench_function("reserve", |b| {
        let ring = Box::new(FlightBuf::<CHUNKS, CHUNK>::new());
        // Safety: the only producer in this benchmark.
        let mut producer = unsafe { ring.producer() };
        let record = [0xa5u8; CHUNK];

        // The open chunk is left full by the previous iteration, so every reserve seals.
        b.iter(|| producer.reserve(CHUNK, |bytes| bytes.copy_from_slice(black_box(&record))));
    });

    g.finish();
}

/// One iteration is one chunk copied out and checked, so unlike a queue's zero-copy handoff this
/// scales with the chunk size.
fn bench_read(c: &mut Criterion) {
    let mut g = c.benchmark_group("read");
    g.throughput(Throughput::Bytes(as_u64(CHUNK)));

    g.bench_function("chunk", |b| {
        let ring = Box::new(FlightBuf::<CHUNKS, CHUNK>::new());
        // Safety: the only producer in this benchmark.
        let mut producer = unsafe { ring.producer() };
        let mut reader = ring.reader();
        let mut out = [0u8; CHUNK];
        let record = [0xa5u8; CHUNK];

        b.iter_custom(|iters| {
            // Every chunk but the open one holds history, so that is how long a batch can be
            // before the producer overwrites what the timed region is about to read.
            let per_run = as_u64(CHUNKS - 1);
            let mut elapsed = Duration::ZERO;
            let mut left = iters;

            while left > 0 {
                let batch = left.min(per_run);

                // Untimed: fill the chunks the timed region then reads back.
                for _ in 0..batch {
                    producer.reserve(CHUNK, |bytes| bytes.copy_from_slice(&record));
                }
                producer.flush();

                let start = Instant::now();
                for _ in 0..batch {
                    black_box(reader.read(&mut out).expect("filled above"));
                }
                elapsed += start.elapsed();

                left -= batch;
                while reader.read(&mut out).is_some() {}
            }

            elapsed
        });
    });

    g.finish();
}

criterion_group!(benches, bench_append, bench_seal, bench_read);
criterion_main!(benches);
