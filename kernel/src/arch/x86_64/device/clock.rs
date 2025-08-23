// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr;
use core::time::Duration;

use kasync::time::{Clock, NANOS_PER_SEC, RawClock, RawClockVTable};

static CLOCK_VTABLE: RawClockVTable =
    RawClockVTable::new(clone_raw, now_raw, schedule_wakeup_raw, drop_raw);

unsafe fn clone_raw(ptr: *const ()) -> RawClock {
    tracing::trace!(
        clock.addr = ?ptr,
        "X86_64 CLOCK::clone_raw"
    );
    debug_assert!(ptr.is_null());
    RawClock::new(ptr, &CLOCK_VTABLE)
}

unsafe fn now_raw(_ptr: *const ()) -> u64 {
    tracing::trace!(
        clock.addr = ?_ptr,
        "X86_64 CLOCK::now_raw"
    );
    debug_assert!(_ptr.is_null());

    // Read TSC (Time Stamp Counter)
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") low, out("edx") high);
    }
    ((high as u64) << 32) | (low as u64)
}

unsafe fn schedule_wakeup_raw(_ptr: *const (), at: u64) {
    tracing::trace!(
        clock.addr = ?_ptr,
        at = at,
        "X86_64 CLOCK::schedule_wakeup_raw"
    );
    debug_assert!(_ptr.is_null());

    // TODO: Implement timer interrupt scheduling for x86_64
    // This would typically use APIC timer or HPET
}

unsafe fn drop_raw(_ptr: *const ()) {
    tracing::trace!(
        clock.addr = ?_ptr,
        "X86_64 CLOCK::drop_raw"
    );
    debug_assert!(_ptr.is_null());
}

pub fn new() -> crate::Result<Clock> {
    // TODO: Get actual TSC frequency from CPUID or calibrate it
    // For now, assume 1 GHz TSC frequency as a placeholder
    // This gives us 1 nanosecond per tick which is a reasonable approximation
    // Real TSC frequencies are often higher (2-4 GHz), but we need at least 1ns resolution
    let timebase_frequency = 1_000_000_000;

    let tick_duration = Duration::from_nanos(NANOS_PER_SEC / timebase_frequency);

    // Safety: The TSC is CPU-local but generally synchronized across cores on modern x86_64
    // However, this might not be safe on older systems without invariant TSC
    // FIXME: Check for invariant TSC support and handle accordingly
    let clock = unsafe { Clock::new(tick_duration, ptr::null(), &CLOCK_VTABLE) };

    Ok(clock.named("X86_64 TSC"))
}
