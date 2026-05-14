// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fastrand::FastRand;

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut rng = FastRand::from_seed(42);
    c.bench_function("fastrand", |b| b.iter(|| black_box(rng.fastrand())));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
