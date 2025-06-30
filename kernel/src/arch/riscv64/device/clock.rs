// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::device_tree::Device;
use core::ptr;
use core::time::Duration;
use kasync::time::{Clock, NANOS_PER_SEC, RawClock, RawClockVTable};
use riscv::sbi;

static CLOCK_VTABLE: RawClockVTable =
    RawClockVTable::new(clone_raw, now_raw, schedule_wakeup_raw, drop_raw);

unsafe fn clone_raw(ptr: *const ()) -> RawClock {
    tracing::trace!(
        clock.addr = ?ptr,
        "RISCV CLOCK::clone_raw"
    );
    debug_assert!(ptr.is_null());
    RawClock::new(ptr, &CLOCK_VTABLE)
}

unsafe fn now_raw(_ptr: *const ()) -> u64 {
    tracing::trace!(
        clock.addr = ?_ptr,
        "RISCV CLOCK::now_raw"
    );
    debug_assert!(_ptr.is_null());

    riscv::register::time::read64()
}

unsafe fn schedule_wakeup_raw(_ptr: *const (), at: u64) {
    tracing::trace!(
        clock.addr = ?_ptr,
        "RISCV CLOCK::schedule_wakeup_raw"
    );
    debug_assert!(_ptr.is_null());

    sbi::time::set_timer(at).unwrap();
}

unsafe fn drop_raw(_ptr: *const ()) {
    tracing::trace!(
        clock.addr = ?_ptr,
        "RISCV CLOCK::drop_raw"
    );
    debug_assert!(_ptr.is_null());
}

pub fn new(cpu_node: &Device) -> crate::Result<Clock> {
    let timebase_frequency = cpu_node
        .property("timebase-frequency")
        .or_else(|| cpu_node.parent().unwrap().property("timebase-frequency"))
        .unwrap()
        .as_u64()?;

    let tick_duration = Duration::from_nanos(NANOS_PER_SEC / timebase_frequency);

    let clock = unsafe { Clock::new(tick_duration, ptr::null(), &CLOCK_VTABLE) };

    Ok(clock.named("RISCV CLOCK"))
}
