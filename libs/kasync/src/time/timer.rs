// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod entry;
mod wheel;

use crate::time::{Clock, Instant, Ticks};
use cordyceps::List;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::Poll;
use spin::Mutex;
use wheel::Wheel;

pub(in crate::time) use entry::Entry;
use util::loom_const_fn;

#[derive(Copy, Clone, Debug)]
pub struct Deadline {
    pub ticks: Ticks,
    slot: usize,
    wheel: usize,
}

#[derive(Debug)]
pub struct Timer {
    pub(in crate::time) clock: Clock,
    pub(in crate::time) core: Mutex<Core>,
}

#[derive(Debug)]
pub(super) struct Core {
    /// The ticks that have elapsed since the wheel started.
    now: Ticks,
    /// Timer wheels
    ///
    /// Each timer has 6 wheels with 64 slots, giving each wheel a precision multiplier
    /// of `64^x` where `x` is the wheel level:
    ///
    /// Levels:
    /// - 64^0 slots
    /// - 64^1 slots
    /// - 64^2 slots
    /// - 64^3 slots
    /// - 64^4 slots
    /// - 64^5 slots
    ///
    /// For example, a timer constructed from the default QEMU RISC-V CPU timebase frequency of `10_000_000 hz`
    /// and therefore a `tick_duration` of `100 ns` (10000000 hz => 1/10000000 s => 0,0000001 s => 100ns)
    /// will have the following wheel configuration:
    ///
    /// | wheel | multiplier |                               |
    /// |-------|------------|-------------------------------|
    /// | 0     | 64^0       | 100 ns slots / ~6 µs range    |
    /// | 1     | 64^1       | ~6 µs slots / ~4 ms range     |
    /// | 2     | 64^2       | ~4 ms slots / ~262 ms range   |
    /// | 3     | 64^3       | ~262 ms slots / ~17 sec range |
    /// | 4     | 64^4       | 17 sec range / ~18 min range  |
    /// | 5     | 64^5       | 18 min range / ~19 hr range   |
    ///
    /// As you can see, such a timer configuration can track time up to 19 hours into the future with
    /// a precision of 100 nanoseconds. Quite high precision at the cost of a quite maximum timeout
    /// duration.
    ///
    /// Here is another example of a "lower precision" clock configuration where the `tick_duration`
    /// is `1 ms`:
    ///
    /// | wheel | multiplier |                               |
    /// |-------|------------|-------------------------------|
    /// | 0     | 64^0       | 1 ms slots / 64 ms range      |
    /// | 1     | 64^1       | 64 ms slots / ~ 4 sec range   |
    /// | 2     | 64^2       | ~ 4 sec slots / ~ 4 min range |
    /// | 3     | 64^3       | ~ 4 min slots / ~ 4 hr range  |
    /// | 4     | 64^4       | ~ 4 hr slots / ~ 12 day range |
    /// | 5     | 64^5       | ~ 12 day slots / ~ 2 yr range |
    ///
    /// As you can see with this configuration we are able to track time up to 2 years into the future
    /// with a precision of 1 millisecond which should be an acceptable tradeoff for most applications.
    wheels: [Wheel; Core::WHEELS],
}

// === impl Deadline ===

impl Deadline {
    pub fn as_ticks(&self) -> Ticks {
        self.ticks
    }

    pub fn as_instant(&self, clock: &Clock) -> Instant {
        Instant::from_ticks(clock, self.ticks)
    }
}

// === impl Timer ===

impl Timer {
    loom_const_fn! {
        pub const fn new(clock: Clock) -> Self {
            Self {
                clock,
                core: Mutex::new(Core::new()),
            }
        }
    }

    #[inline]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    #[inline]
    pub fn try_turn(&self) -> Option<(usize, Option<Deadline>)> {
        let mut lock = self.core.try_lock()?;
        Some(self.turn_locked(&mut lock))
    }

    #[inline]
    pub fn turn(&self) -> (usize, Option<Deadline>) {
        let mut lock = self.core.lock();
        self.turn_locked(&mut lock)
    }

    pub(super) fn turn_locked(&self, core: &mut Core) -> (usize, Option<Deadline>) {
        let mut now = self.clock.now_ticks();

        if now < core.now {
            tracing::warn!("time went backwards!");
            now = core.now;
        }

        let mut expired = 0;
        loop {
            let (_expired, next_deadline) = core.poll(now);
            expired += _expired;
            if let Some(next) = next_deadline {
                now = self.clock.now_ticks();
                if now >= next.ticks {
                    // we've advanced past the next deadline, so we need to
                    // advance again.
                    continue;
                }
            }

            return (expired, next_deadline);
        }
    }
}

// === impl Core ===

impl Core {
    const WHEELS: usize = Wheel::BITS;
    const MAX_SLEEP_TICKS: u64 = (1 << (Wheel::BITS * Self::WHEELS)) - 1;

    loom_const_fn! {
        const fn new() -> Self {
            Self {
                now: Ticks(0),
                wheels: [
                    Wheel::new(0),
                    Wheel::new(1),
                    Wheel::new(2),
                    Wheel::new(3),
                    Wheel::new(4),
                    Wheel::new(5),
                ],
            }
        }
    }
    fn poll(&mut self, now: Ticks) -> (usize, Option<Deadline>) {
        // sleeps that need to be rescheduled on lower-level wheels need to be
        // processed after we have finished turning the wheel, to avoid looping
        // infinitely.
        let mut pending_reschedule = List::<Entry>::new();

        let mut expired = 0;

        // we will stop looping if the next deadline is after `now`, but we
        // still need to be able to return it.
        let mut next_deadline = self.next_deadline();
        while let Some(deadline) = next_deadline {
            // if the deadline is in the future we don't need to continue
            if deadline.ticks > now {
                break;
            }

            // Note that we need to take _all_ of the entries off the list before
            // processing any of them. This is important because it's possible that
            // those entries might need to be reinserted into the same slot.
            //
            // This happens only on the highest level, when an entry is inserted
            // more than MAX_DURATION into the future. When this happens, we wrap
            // around, and process some entries a multiple of MAX_DURATION before
            // they actually need to be dropped down a level. We then reinsert them
            // back into the same position; we must make sure we don't then process
            // those entries again, or we'll end up in an infinite loop.
            let entries = self.wheels[deadline.wheel].take_slot(deadline.slot);
            for entry in entries {
                // Safety: upon registering the caller promised the entry is valid
                let entry_deadline = unsafe { entry.as_ref().deadline };

                if entry_deadline > now {
                    // this timer was on the top-level wheel and needs to be
                    // rescheduled on a lower-level wheel, rather than firing now.
                    debug_assert_ne!(
                        deadline.wheel, 0,
                        "if a timer is being rescheduled, it must not have been on the lowest-level wheel"
                    );
                    tracing::trace!(
                        "rescheduling entry {entry:?} because deadline {entry_deadline:?} is later than now {now:?}"
                    );
                    // this timer will need to be rescheduled.
                    pending_reschedule.push_front(entry);
                } else {
                    // otherwise, fire the timer
                    // Safety: upon registering the caller promised the entry is valid
                    unsafe {
                        expired += 1;
                        entry.as_ref().fire();
                    }
                }
            }

            self.now = deadline.ticks;
            next_deadline = self.next_deadline();
        }

        self.now = now;

        let any = !pending_reschedule.is_empty();

        for entry in pending_reschedule {
            // Safety: upon registering the caller promised the entry is valid
            let entry_deadline = unsafe { entry.as_ref().deadline };

            debug_assert!(entry_deadline > self.now);
            debug_assert_ne!(entry_deadline, Ticks(0));
            self.insert_at(entry_deadline, entry);
        }

        if any {
            next_deadline = self.next_deadline();
        }

        (expired, next_deadline)
    }

    fn next_deadline(&self) -> Option<Deadline> {
        self.wheels.iter().find_map(|wheel| {
            let next_deadline = wheel.next_deadline(self.now)?;
            Some(next_deadline)
        })
    }

    pub(super) fn cancel(&mut self, entry: Pin<&mut Entry>) {
        let deadline = entry.deadline;
        tracing::trace!("canceling entry={entry:?};now={:?}", self.now);
        let wheel = self.wheel_index(deadline);
        self.wheels[wheel].remove(deadline, entry);
    }

    pub(super) unsafe fn register(&mut self, ptr: NonNull<Entry>) -> Poll<()> {
        let deadline = {
            // Safety: callers responsibility
            let entry = unsafe { ptr.as_ref() };

            tracing::trace!("registering entry={entry:?};now={:?}", self.now);

            if entry.deadline <= self.now {
                tracing::trace!("timer already completed, firing immediately");
                entry.fire();
                return Poll::Ready(());
            }

            let _did_link = entry.is_registered.compare_exchange(
                false,
                true,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            debug_assert!(
                _did_link.is_ok(),
                "tried to register a sleep that was already registered"
            );

            entry.deadline
        };

        self.insert_at(deadline, ptr);
        Poll::Pending
    }

    fn insert_at(&mut self, deadline: Ticks, entry: NonNull<Entry>) {
        let wheel = self.wheel_index(deadline);
        tracing::trace!("inserting entry={entry:?};deadline={deadline:?}");
        self.wheels[wheel].insert(deadline, entry);
    }

    #[inline]
    fn wheel_index(&self, ticks: Ticks) -> usize {
        wheel_index(self.now, ticks)
    }
}

fn wheel_index(now: Ticks, ticks: Ticks) -> usize {
    const WHEEL_MASK: u64 = (1 << Wheel::BITS) - 1;

    // mask out the bits representing the index in the wheel
    let mut wheel_indices = now.0 ^ ticks.0 | WHEEL_MASK;

    // put sleeps over the max duration in the top level wheel
    if wheel_indices >= Core::MAX_SLEEP_TICKS {
        wheel_indices = Core::MAX_SLEEP_TICKS - 1;
    }

    let zeros = wheel_indices.leading_zeros();
    let rest = u64::BITS - 1 - zeros;

    rest as usize / Core::WHEELS
}
