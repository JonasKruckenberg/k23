use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use k23_fastrand::FastRand;

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut rng = FastRand::from_seed(42);
    c.bench_function("fastrand", |b| b.iter(|| black_box(rng.fastrand())));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
