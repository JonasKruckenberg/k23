// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use async_exec::executor::{Executor, Worker};
use async_exec::new_executor;
use async_exec::park::StdPark;
use criterion::{Criterion, criterion_group, criterion_main};
use fastrand::FastRand;

fn ping_ping_10k_single_threaded(c: &mut Criterion) {
    static EXEC: Executor<StdPark> = new_executor!(1);
    let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

    const PINGS: usize = 10_000;

    c.bench_function("ping_ping_10k_single_threaded", |b| {
        b.iter(|| {
            let h = EXEC
                .try_spawn(async {
                    for _ in 0..PINGS {
                        async_exec::task::yield_now().await;
                    }
                })
                .unwrap();
            worker.block_on(h).unwrap();
        });
    });
}

fn ping_pong_10k_single_threaded(c: &mut Criterion) {
    static EXEC: Executor<StdPark> = new_executor!(1);
    let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

    const PINGS: usize = 10_000;

    c.bench_function("ping_pong_10k_single_threaded", |b| {
        b.iter(|| {
            let h1 = EXEC
                .try_spawn(async {
                    for _ in 0..PINGS {
                        async_exec::task::yield_now().await;
                    }
                })
                .unwrap();

            let h2 = EXEC
                .try_spawn(async {
                    for _ in 0..PINGS {
                        async_exec::task::yield_now().await;
                    }
                })
                .unwrap();

            worker.block_on(futures::future::try_join(h1, h2)).unwrap();
        });
    });
}

fn ping_ping_10k_multi_threaded(c: &mut Criterion) {
    static EXEC: Executor<StdPark> = new_executor!(1);
    let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

    let h = std::thread::spawn(|| {
        let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
        worker.run();
    });

    const PINGS: usize = 10_000;

    c.bench_function("ping_ping_10k_multi_threaded", |b| {
        b.iter(|| {
            let h = EXEC
                .try_spawn(async {
                    for _ in 0..PINGS {
                        async_exec::task::yield_now().await;
                    }
                })
                .unwrap();
            worker.block_on(h).unwrap();
        });
    });

    EXEC.stop();
    h.join().unwrap();
}

fn ping_pong_10k_multi_threaded(c: &mut Criterion) {
    static EXEC: Executor<StdPark> = new_executor!(1);
    let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

    let h = std::thread::spawn(|| {
        let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
        worker.run();
    });

    const PINGS: usize = 10_000;

    c.bench_function("ping_pong_10k_multi_threaded", |b| {
        b.iter(|| {
            let h1 = EXEC
                .try_spawn(async {
                    for _ in 0..PINGS {
                        async_exec::task::yield_now().await;
                    }
                })
                .unwrap();

            let h2 = EXEC
                .try_spawn(async {
                    for _ in 0..PINGS {
                        async_exec::task::yield_now().await;
                    }
                })
                .unwrap();

            worker.block_on(futures::future::try_join(h1, h2)).unwrap();
        });
    });

    EXEC.stop();
    h.join().unwrap();
}

criterion_group!(
    ping_pong,
    ping_ping_10k_single_threaded,
    ping_pong_10k_single_threaded,
    ping_ping_10k_multi_threaded,
    ping_pong_10k_multi_threaded,
);
criterion_main!(ping_pong);
