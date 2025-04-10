// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::device::cpu::with_cpu;
use crate::time::Clock;
use crate::time::clock::Ticks;
use crate::time::sleep::Entry;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::Poll;
use core::time::Duration;
use spin::Mutex;

#[derive(Copy, Clone, Debug)]
pub struct Deadline {
    pub ticks: Ticks,
    slot: usize,
    wheel: usize,
}

pub struct Timer {
    pub(super) core: Mutex<Core>,
}

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

struct Wheel {
    /// A bitmap of the slots that are occupied.
    ///
    /// The least-significant bit represents slot zero.
    ///
    /// See <https://lwn.net/Articles/646056/> for details on
    /// this strategy.
    occupied_slots: u64,
    slots: [linked_list::List<Entry>; Wheel::SLOTS],
    /// This wheel's level.
    level: usize,
    /// The number of ticks represented by a single slot in this wheel.
    ticks_per_slot: Ticks,
    /// The number of ticks represented by this entire wheel.
    ticks_per_wheel: Ticks,
    /// A bitmask for masking out all lower wheels' indices from a `now` timestamp.
    wheel_mask: u64,
}

impl Timer {
    pub(crate) fn new() -> Self {
        Self {
            core: Mutex::new(Core::new()),
        }
    }

    pub fn turn(&self) -> (usize, Option<Deadline>) {
        let mut lock = self.core.lock();
        self.turn_locked(&mut lock)
    }

    pub(super) fn turn_locked(&self, core: &mut Core) -> (usize, Option<Deadline>) {
        let mut now = with_cpu(|cpu| cpu.clock.now_ticks());

        if now < core.now {
            tracing::warn!("time went backwards!");
            now = core.now;
        }

        let mut expired = 0;
        loop {
            let (_expired, next_deadline) = core.poll(now);
            expired += _expired;
            if let Some(next) = next_deadline {
                now = with_cpu(|cpu| cpu.clock.now_ticks());
                if now >= next.ticks {
                    // we've advanced past the next deadline, so we need to
                    // advance again.
                    continue;
                }
            }

            return (expired, next_deadline);
        }
    }

    fn cancel(&self, entry: Pin<&mut Entry>) {
        let mut lock = self.core.lock();
        lock.cancel(entry);
    }

    unsafe fn register(&self, ptr: NonNull<Entry>) -> Poll<()> {
        let mut lock = self.core.lock();
        // Safety: callers responsibility
        unsafe { lock.register(ptr) }
    }
}

impl Core {
    const WHEELS: usize = Wheel::BITS;
    const MAX_SLEEP_TICKS: u64 = (1 << (Wheel::BITS * Self::WHEELS)) - 1;

    fn new() -> Self {
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

    fn poll(&mut self, now: Ticks) -> (usize, Option<Deadline>) {
        // sleeps that need to be rescheduled on lower-level wheels need to be
        // processed after we have finished turning the wheel, to avoid looping
        // infinitely.
        let mut pending_reschedule = linked_list::List::<Entry>::new();
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

impl Wheel {
    /// The number of slots per timer wheel is fixed at 64 slots.
    ///
    /// This is because we can use a 64-bit bitmap for each wheel to store which
    /// slots are occupied.
    const SLOTS: usize = 64;
    const BITS: usize = Self::SLOTS.trailing_zeros() as usize;

    #[allow(
        clippy::cast_possible_truncation,
        reason = "slot index can be at most 64"
    )]
    fn new(level: usize) -> Self {
        // how many ticks does a single slot represent in a wheel of this level?
        let ticks_per_slot = Ticks(Self::SLOTS.pow(level as u32) as u64);
        let ticks_per_wheel = Ticks(ticks_per_slot.0 * Self::SLOTS as u64);

        debug_assert!(ticks_per_slot.0.is_power_of_two());
        debug_assert!(ticks_per_wheel.0.is_power_of_two());

        // because `ticks_per_wheel` is a power of two, we can calculate a
        // bitmask for masking out the indices in all lower wheels from a `now`
        // timestamp.
        let wheel_mask = !(ticks_per_wheel.0 - 1);
        let slots = [const { linked_list::List::new() }; Self::SLOTS];

        Self {
            level,
            ticks_per_slot,
            ticks_per_wheel,
            wheel_mask,
            occupied_slots: 0,
            slots,
        }
    }

    fn insert(&mut self, deadline: Ticks, entry: NonNull<Entry>) {
        let slot = self.slot_index(deadline);
        // insert the sleep entry into the appropriate linked list.
        self.slots[slot].push_front(entry);
        // toggle the occupied bit for that slot.
        self.fill_slot(slot);
    }

    fn remove(&mut self, deadline: Ticks, entry: Pin<&mut Entry>) {
        let slot = self.slot_index(deadline);
        // safety: we will not use the `NonNull` to violate pinning
        // invariants; it's used only to insert the sleep into the intrusive
        // list. It's safe to remove the sleep from the linked list because
        // we know it's in this list (provided the rest of the timer wheel
        // is like...working...)
        unsafe {
            let ptr = NonNull::from(Pin::into_inner_unchecked(entry));
            if let Some(entry) = self.slots[slot].cursor_from_ptr_mut(ptr).remove() {
                let _did_unlink = entry.as_ref().is_registered.compare_exchange(
                    true,
                    false,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                debug_assert!(
                    _did_unlink.is_ok(),
                    "removed a sleep whose linked bit was already unset, this is potentially real bad"
                );
            }
        };

        if self.slots[slot].is_empty() {
            // if that was the only sleep in that slot's linked list, clear the
            // corresponding occupied bit.
            self.clear_slot(slot);
        }
    }

    fn next_deadline(&self, now: Ticks) -> Option<Deadline> {
        let distance = self.next_slot_distance(now)?;

        let slot = distance % Self::SLOTS;
        // does the next slot wrap this wheel around from the now slot?
        let skipped = distance.saturating_sub(Self::SLOTS);

        debug_assert!(distance < Self::SLOTS * 2);
        debug_assert!(
            skipped == 0 || self.level == Core::WHEELS - 1,
            "if the next expiring slot wraps around, we must be on the top level wheel\
            \n    dist: {distance}\
            \n    slot: {slot}\
            \n skipped: {skipped}\
            \n   level: {}",
            self.level,
        );

        // when did the current rotation of this wheel begin? since all wheels
        // represent a power-of-two number of ticks, we can determine the
        // beginning of this rotation by masking out the bits for all lower wheels.
        let rotation_start = now.0 & self.wheel_mask;
        // the next deadline is the start of the current rotation, plus the next
        // slot's value.
        let ticks = {
            let skipped_ticks = skipped as u64 * self.ticks_per_wheel.0;
            Ticks(rotation_start + (slot as u64 * self.ticks_per_slot.0) + skipped_ticks)
        };

        let deadline = Deadline {
            ticks,
            slot,
            wheel: self.level,
        };

        Some(deadline)
    }

    /// Returns the slot index of the next firing timer.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "slot index can be at most 64"
    )]
    fn next_slot_distance(&self, now: Ticks) -> Option<usize> {
        if self.occupied_slots == 0 {
            return None;
        }

        // which slot is indexed by the `now` timestamp?
        let now_slot = (now.0 / self.ticks_per_slot.0) as u32 % Self::SLOTS as u32;
        let next_dist = next_set_bit(self.occupied_slots, now_slot)?;

        Some(next_dist)
    }

    fn take_slot(&mut self, slot: usize) -> linked_list::List<Entry> {
        debug_assert!(
            self.occupied_slots & (1 << slot) != 0,
            "taking an unoccupied slot!"
        );
        let list = self.slots[slot].take();
        debug_assert!(
            !list.is_empty(),
            "if a slot is occupied, its list must not be empty"
        );
        self.clear_slot(slot);
        list
    }

    fn clear_slot(&mut self, slot_index: usize) {
        debug_assert!(slot_index < Self::SLOTS);
        self.occupied_slots &= !(1 << slot_index);
    }

    fn fill_slot(&mut self, slot_index: usize) {
        debug_assert!(slot_index < Self::SLOTS);
        self.occupied_slots |= 1 << slot_index;
    }

    /// Given a duration, returns the slot into which an entry for that duratio
    /// would be inserted.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "slot index can be at most 64"
    )]
    const fn slot_index(&self, ticks: Ticks) -> usize {
        let shift = self.level * Self::BITS;
        ((ticks.0 >> shift) % Self::SLOTS as u64) as usize
    }
}

/// Finds the index of the next set bit in `bitmap` after the `offset`th` bit.
/// If the `offset`th bit is set, returns `offset`.
///
/// Based on
/// <https://github.com/torvalds/linux/blob/d0e60d46bc03252b8d4ffaaaa0b371970ac16cda/include/linux/find.h#L21-L45>
fn next_set_bit(bitmap: u64, offset: u32) -> Option<usize> {
    debug_assert!(offset < 64, "offset: {offset}");
    if bitmap == 0 {
        return None;
    }
    let shifted = bitmap >> offset;
    let zeros = if shifted == 0 {
        bitmap.rotate_right(offset).trailing_zeros()
    } else {
        shifted.trailing_zeros()
    };
    Some(zeros as usize + offset as usize)
}
