// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use criterion::measurement::Measurement;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use fiber::Fiber;
use fiber::stack::DefaultFiberStack;

fn fiber_switch<M: Measurement + 'static>(name: &str, c: &mut Criterion<M>) {
    let stack = DefaultFiberStack::default();
    let mut identity = Fiber::with_stack(stack, |mut input, yielder| {
        loop {
            input = yielder.suspend(input)
        }
    });

    c.bench_function(name, |b| b.iter(|| identity.resume(black_box(0usize))));

    // Forcibly reset the fiber so that this benchmarks works even when the
    // unwind feature is disabled.
    unsafe {
        identity.force_reset();
    }
}

fn fiber_call<M: Measurement + 'static>(name: &str, c: &mut Criterion<M>) {
    // Don't count time spent allocating a stack.
    let mut stack = DefaultFiberStack::default();

    c.bench_function(name, move |b| {
        b.iter(|| {
            let mut identity =
                Fiber::<usize, (), usize, _, _>::with_stack(&mut stack, |input, _yielder| input);
            identity.resume(black_box(0usize))
        })
    });
}

fn fiber_switch_time(c: &mut Criterion) {
    fiber_switch("fiber_switch_time", c);
}
fn fiber_call_time(c: &mut Criterion) {
    fiber_call("fiber_call_time", c);
}

criterion_group!(
    name = time;
    config = Criterion::default();
    targets = fiber_switch_time, fiber_call_time
);

cfg_if::cfg_if! {
    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        use criterion_cycles_per_byte::CyclesPerByte;

        fn fiber_switch_cycles(c: &mut Criterion<CyclesPerByte>) {
            fiber_switch("fiber_switch_cycles", c);
        }
        fn fiber_call_cycles(c: &mut Criterion<CyclesPerByte>) {
            fiber_call("fiber_call_cycles", c);
        }

        criterion_group!(
            name = cycles;
            config = Criterion::default().with_measurement(CyclesPerByte);
            targets = fiber_switch_cycles, fiber_call_cycles
        );

        criterion_main!(cycles, time);
    } else {
        criterion_main!(time);
    }
}
