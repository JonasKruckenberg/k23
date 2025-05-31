mod entry;
mod wheel;

use crate::time::clock::PhysTicks;
use crate::time::{Clock, Instant, TimeError};
use cordyceps::List;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::Poll;
use core::time::Duration;
use spin::Mutex;
use util::loom_const_fn;
use wheel::Wheel;

pub(in crate::time) use entry::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtTicks(pub u64);

#[derive(Copy, Clone, Debug)]
pub struct Deadline {
    pub ticks: VirtTicks,
    slot: usize,
    wheel: usize,
}

#[derive(Debug)]
pub struct Timer {
    clock: Clock,
    tick_scale: u64,
    pub(in crate::time) core: Mutex<Core>,
}

#[derive(Debug)]
pub(super) struct Core {
    /// The ticks that have elapsed since the wheel started.
    now: VirtTicks,
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
    pub fn as_ticks(&self) -> VirtTicks {
        self.ticks
    }

    pub fn as_instant(&self, timer: &Timer) -> Instant {
        Instant::from_ticks(timer, self.ticks)
    }
}

// === impl Timer ===

impl Timer {
    loom_const_fn! {
        pub const fn new(tick_duration: Duration, clock: Clock) -> Self {
            debug_assert!(tick_duration.as_nanos() >= clock.tick_duration().as_nanos());

            let tick_scale = tick_duration.as_nanos() / clock.tick_duration().as_nanos();
            debug_assert!(tick_scale <= u64::MAX as u128);

            Self {
                clock,
                #[expect(clippy::cast_possible_truncation, reason = "the assertion above checked that this is fine")]
                tick_scale: tick_scale as u64,
                core: Mutex::new(Core::new())
            }
        }
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    pub fn now_ticks(&self) -> VirtTicks {
        self.phys_to_virt(self.clock.now_ticks())
    }

    /// Convert the given raw [`VirtTicks`] into a [`Duration`] using this timers
    /// internal tick duration.
    ///
    /// # Panics
    ///
    /// This method panics if the conversion would overflow.
    pub fn ticks_to_duration(&self, ticks: VirtTicks) -> Duration {
        self.clock.ticks_to_duration(self.virt_to_phys(ticks))
    }

    /// Convert the given [`Duration`] into a raw [`VirtTicks`] using this timers
    /// internal tick duration.
    ///
    /// # Errors
    ///
    /// Returns a [`TimeError`] if the duration doesn't fit into the ticks u64 representation.
    pub fn duration_to_ticks(&self, duration: Duration) -> Result<VirtTicks, TimeError> {
        Ok(self.phys_to_virt(self.clock.duration_to_ticks(duration)?))
    }

    fn virt_to_phys(&self, virt_ticks: VirtTicks) -> PhysTicks {
        PhysTicks(virt_ticks.0 * self.tick_scale)
    }

    fn phys_to_virt(&self, phys_ticks: PhysTicks) -> VirtTicks {
        VirtTicks(phys_ticks.0 / self.tick_scale)
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
        let mut now = self.now_ticks();

        if now < core.now {
            tracing::warn!("time went backwards!");
            now = core.now;
        }

        let mut expired = 0;
        loop {
            let (_expired, next_deadline) = core.poll(now);
            expired += _expired;
            if let Some(next) = next_deadline {
                now = self.now_ticks();
                if now >= next.ticks {
                    // we've advanced past the next deadline, so we need to
                    // advance again.
                    continue;
                }
            }

            tracing::trace!(expired, ?next_deadline, "turn_locked");
            return (expired, next_deadline);
        }
    }
}

// === impl Core ===

impl Core {
    const WHEELS: usize = Wheel::BITS;
    const MAX_SLEEP_TICKS: u64 = (1 << (Wheel::BITS * Self::WHEELS)) - 1;

    #[inline]
    const fn new() -> Self {
        Self {
            now: VirtTicks(0),
            #[cfg(not(loom))]
            wheels: [
                Wheel::new(0),
                Wheel::new(1),
                Wheel::new(2),
                Wheel::new(3),
                Wheel::new(4),
                Wheel::new(5),
            ],
            #[cfg(loom)]
            wheels: [Wheel::new(0), Wheel::new(1)],
        }
    }

    fn poll(&mut self, now: VirtTicks) -> (usize, Option<Deadline>) {
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
            debug_assert_ne!(entry_deadline, VirtTicks(0));
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

    fn insert_at(&mut self, deadline: VirtTicks, entry: NonNull<Entry>) {
        let wheel = self.wheel_index(deadline);
        tracing::trace!("inserting entry={entry:?};deadline={deadline:?}");
        self.wheels[wheel].insert(deadline, entry);
    }

    #[inline]
    fn wheel_index(&self, ticks: VirtTicks) -> usize {
        wheel_index(self.now, ticks)
    }
}

fn wheel_index(now: VirtTicks, ticks: VirtTicks) -> usize {
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

#[cfg(test)]
mod tests {
    use crate::loom;
    use crate::test_util::clock_100ns;
    use crate::time::Timer;
    use core::time::Duration;

    #[test]
    fn map_ticks_roundtrip() {
        loom::model(move || {
            let timer = Timer::new(Duration::from_millis(1), clock_100ns());

            let dur = Duration::from_secs(42);

            let virt_ticks = timer.duration_to_ticks(dur).unwrap();
            let phys_ticks = timer.virt_to_phys(virt_ticks);

            let virt_ticks = timer.phys_to_virt(phys_ticks);
            let dur2 = timer.ticks_to_duration(virt_ticks);

            assert_eq!(dur, dur2);
        });
    }

    #[test]
    fn tick_precision() {
        loom::model(move || {
            let timer = Timer::new(Duration::from_millis(500), clock_100ns());

            let start_timer = timer.now_ticks();
            let start_clock = timer.clock.now_ticks();

            std::thread::sleep(Duration::from_millis(250));
            let halfway_timer = timer.now_ticks();
            let halfway_clock = timer.clock.now_ticks();

            std::thread::sleep(Duration::from_millis(250));
            let end_timer = timer.now_ticks();
            let end_clock = timer.clock.now_ticks();

            // we expect both the start and halfway timestamp to be 0
            // since 250ms < 500ms they fall into the same "bucket"
            assert_eq!(start_timer.0, 0);
            assert_eq!(halfway_timer.0, 0);

            // but we expect the end timestamp to be 1
            // since 500ms is the timer precision
            assert_eq!(end_timer.0, 1);

            // But we do expect the clock to tick up all the time
            const NANOS_PER_MS: u64 = 1_000_000;
            const TICKS_PER_MS: u64 = NANOS_PER_MS / 100;

            // the start timestamp should definitely smaller than the next timestamp
            assert!(start_clock.0 < halfway_clock.0);

            // the halfway timestamp should be 250ms or more, but definitely smaller than the next timestamp
            assert!(halfway_clock.0 / TICKS_PER_MS >= 250);
            assert!(halfway_clock.0 < end_clock.0);

            // the end timestamp should be 500ms or more
            assert!(end_clock.0 / TICKS_PER_MS >= 500);
        });
    }
}
