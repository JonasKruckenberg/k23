// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::hint::black_box;

use chaos::{Callsite, ControlPlane, site};
use criterion::{Criterion, criterion_group, criterion_main};

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("plane");
    let site = site!();
    let mut plane = ControlPlane::new(black_box(42));
    group.bench_function("decide_at", |b| {
        b.iter(|| black_box(black_box(&mut plane).decide_at(site)));
    });
    group.bench_function("new", |b| {
        b.iter(|| black_box(ControlPlane::new(black_box(42))));
    });
    group.finish();

    // Through the macro and the installed implementation, i.e. what a real call
    // site pays.
    let mut group = c.benchmark_group("macro");
    group.bench_function("decide", |b| b.iter(|| black_box(chaos::decide!())));
    group.bench_function("assert_stable", |b| {
        let x = black_box(7_u64);
        b.iter(|| chaos::assert_stable!(|| black_box(x)));
    });
    group.finish();

    let _ = Callsite::as_u64(site);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
