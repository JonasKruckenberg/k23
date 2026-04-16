// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr;
use core::time::Duration;

use kasync::time::{Clock, NANOS_PER_SEC, RawClock, RawClockVTable};
use riscv::sbi;

use crate::device_tree::Device;

static CLOCK_VTABLE: RawClockVTable =
    RawClockVTable::new(clone_raw, now_raw, schedule_wakeup_raw, drop_raw);

unsafe fn clone_raw(ptr: *const ()) -> RawClock {
    debug_assert!(ptr.is_null());
    RawClock::new(ptr, &CLOCK_VTABLE)
}

unsafe fn now_raw(_ptr: *const ()) -> u64 {
    debug_assert!(_ptr.is_null());

    riscv::register::time::read64()
}

unsafe fn schedule_wakeup_raw(_ptr: *const (), at: u64) {
    debug_assert!(_ptr.is_null());

    sbi::time::set_timer(at).unwrap();
}

unsafe fn drop_raw(_ptr: *const ()) {
    debug_assert!(_ptr.is_null());
}

pub fn new(cpu_node: &Device) -> crate::Result<Clock> {
    let timebase_frequency = cpu_node
        .property("timebase-frequency")
        .or_else(|| cpu_node.parent().unwrap().property("timebase-frequency"))
        .unwrap()
        .as_u64()?;

    let tick_duration = Duration::from_nanos(NANOS_PER_SEC / timebase_frequency);

    // Safety: HAHA, actually this ISN'T SAFE! Technically both `now_raw` and `now_schedule_wakeup`
    // access HART-local registers for the time & timeout, so in the scenario that different HARTs in
    // the system disagree about what time it is, we're in real big trouble. Unfortunately, I can't
    // think of a great way to solve this that doesn't involve incrementing a global on a timer or
    // choosing one hart that maintains & drives timers.
    // *Fortunately* this situation is probably quite rare, so we're going to ignore this for now.
    // FIXME: <https://github.com/JonasKruckenberg/k23/issues/490>
    let clock = unsafe { Clock::new(tick_duration, ptr::null(), &CLOCK_VTABLE) };

    Ok(clock.named("RISCV CLOCK"))
}
