// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use cordyceps::List;

use crate::time::Ticks;
use crate::time::timer::entry::Entry;
use crate::time::timer::{Core, Deadline};

#[derive(Debug)]
pub(crate) struct Wheel {
    /// A bitmap of the slots that are occupied.
    ///
    /// The least-significant bit represents slot zero.
    ///
    /// See <https://lwn.net/Articles/646056/> for details on
    /// this strategy.
    occupied_slots: u64,
    slots: [List<Entry>; Wheel::SLOTS],
    /// This wheel's level.
    level: usize,
    /// The number of ticks represented by a single slot in this wheel.
    ticks_per_slot: Ticks,
    /// The number of ticks represented by this entire wheel.
    ticks_per_wheel: Ticks,
    /// A bitmask for masking out all lower wheels' indices from a `now` timestamp.
    wheel_mask: u64,
}

impl Wheel {
    /// The number of slots per timer wheel is fixed at 64 slots.
    ///
    /// This is because we can use a 64-bit bitmap for each wheel to store which
    /// slots are occupied.
    #[cfg(not(loom))]
    const SLOTS: usize = 64;
    // loom is very "stack overflow happy" reducing the number of
    // slots I found helps this a lot.
    // Besides, for testing we won't have many timer entries anyway
    #[cfg(loom)]
    const SLOTS: usize = 4;

    pub(crate) const BITS: usize = Self::SLOTS.trailing_zeros() as usize;

    #[allow(
        clippy::cast_possible_truncation,
        reason = "slot index can be at most 64"
    )]
    #[inline]
    pub(crate) const fn new(level: usize) -> Self {
        // how many ticks does a single slot represent in a wheel of this level?
        let ticks_per_slot = Ticks(Self::SLOTS.pow(level as u32) as u64);
        let ticks_per_wheel = Ticks(ticks_per_slot.0 * Self::SLOTS as u64);

        debug_assert!(ticks_per_slot.0.is_power_of_two());
        debug_assert!(ticks_per_wheel.0.is_power_of_two());

        // because `ticks_per_wheel` is a power of two, we can calculate a
        // bitmask for masking out the indices in all lower wheels from a `now`
        // timestamp.
        let wheel_mask = !(ticks_per_wheel.0 - 1);
        let slots = [const { List::new() }; Self::SLOTS];

        Self {
            level,
            ticks_per_slot,
            ticks_per_wheel,
            wheel_mask,
            occupied_slots: 0,
            slots,
        }
    }

    pub(crate) fn insert(&mut self, deadline: Ticks, ptr: NonNull<Entry>) {
        let slot = self.slot_index(deadline);
        // insert the sleep entry into the appropriate linked list.
        self.slots[slot].push_front(ptr);
        // toggle the occupied bit for that slot.
        self.fill_slot(slot);
    }

    pub(crate) fn remove(&mut self, deadline: Ticks, entry: Pin<&mut Entry>) {
        let slot = self.slot_index(deadline);
        // safety: we will not use the `NonNull` to violate pinning
        // invariants; it's used only to insert the sleep into the intrusive
        // list. It's safe to remove the sleep from the linked list because
        // we know it's in this list (provided the rest of the timer wheel
        // is like...working...)
        unsafe {
            let entry = NonNull::from(Pin::into_inner_unchecked(entry));
            if let Some(entry) = self.slots[slot].remove(entry) {
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

    pub(crate) fn next_deadline(&self, now: Ticks) -> Option<Deadline> {
        let distance = self.next_slot_distance(now)?;

        let slot = distance % Self::SLOTS;
        // does the next slot wrap this wheel around from the now slot?
        let skipped = distance.saturating_sub(Self::SLOTS);

        debug_assert!(
            distance < Self::SLOTS * 2,
            "distance must be less than 2*{}, but found {distance}",
            Self::SLOTS
        );
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

    pub(crate) fn take_slot(&mut self, slot: usize) -> List<Entry> {
        debug_assert!(
            self.occupied_slots & (1 << slot) != 0,
            "taking an unoccupied slot!"
        );
        let list = self.slots[slot].split_off(0);
        debug_assert!(
            !list.is_empty(),
            "if a slot is occupied, its list must not be empty"
        );
        self.clear_slot(slot);
        list
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
        let next_dist = next_set_bit(self.occupied_slots, now_slot)? % Self::SLOTS;
        tracing::trace!(now_slot, next_dist);

        Some(next_dist)
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "slot index can be at most 64"
    )]
    fn clear_slot(&mut self, slot_index: usize) {
        debug_assert!(slot_index < Self::SLOTS);
        self.occupied_slots &= !(1 << slot_index);
        debug_assert!(self.occupied_slots.count_ones() <= Self::SLOTS as u32);
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "slot index can be at most 64"
    )]
    fn fill_slot(&mut self, slot_index: usize) {
        debug_assert!(slot_index < Self::SLOTS);
        self.occupied_slots |= 1 << slot_index;
        debug_assert!(self.occupied_slots.count_ones() <= Self::SLOTS as u32);
    }

    /// Given a duration, returns the slot into which an entry for that duration
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
