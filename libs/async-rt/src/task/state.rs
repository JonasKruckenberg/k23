// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![expect(
    impl_trait_overcaptures,
    reason = "mycelium_bitfield is not updated to edition 2024 yet"
)]

use crate::loom::sync::atomic::{self, AtomicUsize, Ordering};
use crate::task::PollResult;
use core::fmt;
use util::loom_const_fn;

/// Task state. The task stores its state in an atomic `usize` with various bitfields for the
/// necessary information. The state has the following layout:
///
/// ```text
/// | 63     7 | 6        5 | 4             4 | 3      3 | 2   2 | 1       0 |
/// | refcount | join waker | has join handle | cancelled | woken | lifecycle |
/// ```
///
/// The rest of the bits are used for the ref-count.
pub(crate) struct State {
    val: AtomicUsize,
}

mycelium_bitfield::bitfield! {
    /// A snapshot of a task's current state.
    #[derive(PartialEq, Eq)]
    pub(crate) struct Snapshot<usize> {
        /// If set, this task is currently being polled.
        pub const POLLING: bool;
        /// If set, this task's `Future` has completed (i.e., it has returned
        /// `Poll::Ready`).
        pub const COMPLETE: bool;
        /// If set, this task's `Waker` has been woken.
        pub(crate) const WOKEN: bool;
        /// If set, this task has been canceled.
        pub const CANCELLED: bool;
        /// If set, this task has a `JoinHandle` awaiting its completion.
        ///
        /// If the `JoinHandle` is dropped, this flag is unset.
        ///
        /// This flag does *not* indicate the presence of a `Waker` in the
        /// `join_waker` slot; it only indicates that a `JoinHandle` for this
        /// task *exists*. The join waker may not currently be registered if
        /// this flag is set.
        pub const HAS_JOIN_HANDLE: bool;
        /// The state of the task's `JoinHandle` `Waker`.
        const JOIN_WAKER: JoinWakerState;
         /// If set, this task has output ready to be taken by a `JoinHandle`.
        const HAS_OUTPUT: bool;
        /// The number of currently live references to this task.
        ///
        /// When this is 0, the task may be deallocated.
        const REFS = ..;
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
enum JoinWakerState {
    /// There is no join waker; the slot is uninitialized.
    Empty = 0b00,
    /// A join waker is *being* registered.
    Registering = 0b01,
    /// A join waker is registered, the slot is initialized.
    Waiting = 0b10,
    /// The join waker has been woken.
    Woken = 0b11,
}

#[must_use]
pub(super) enum StartPollAction {
    /// Successful transition, it's okay to poll the task.
    Poll,
    /// Transition failed for some reason - most likely it is already running on another thread
    /// (which shouldn't happen) - doesn't matter though we shouldn't poll the task.
    DontPoll,
    /// Transition failed because the task was cancelled and its `JoinHandle` waker may need to be woken.
    Cancelled {
        /// If `true`, the task's join waker must be woken.
        wake_join_waker: bool,
    },
}

#[must_use]
pub enum JoinAction {
    /// It's safe to take the task's output!
    TakeOutput,

    /// The task was canceled, it cannot be joined.
    Canceled {
        /// If `true`, the task completed successfully before it was cancelled.
        completed: bool,
    },

    /// Register the *first* join waker; there is no previous join waker and the
    /// slot is not initialized.
    Register,

    /// The task is not ready to read the output, but a previous join waker is
    /// registered.
    Reregister,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum WakeByRefAction {
    /// The task should be enqueued.
    Enqueue,
    /// The task does not need to be enqueued.
    None,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum WakeByValAction {
    /// The task should be enqueued.
    Enqueue,
    /// The task does not need to be enqueued.
    None,
    /// The task should be deallocated.
    Drop,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ShutdownAction {
    /// The task does not need to be enqueued.
    None,
    /// The task should be deallocated.
    Drop,
}

const REF_ONE: usize = Snapshot::REFS.first_bit();
const REF_MAX: usize = Snapshot::REFS.raw_mask();

impl State {
    loom_const_fn! {
        /// Returns a task's initial state.
        pub(super) const fn new() -> State {
            // The raw task returned by this method has a ref-count of three. See
            // the comment on INITIAL_STATE for more.
            State {
                val: AtomicUsize::new(REF_ONE),
            }
        }
    }

    pub(super) fn load(&self, ordering: Ordering) -> Snapshot {
        Snapshot(self.val.load(ordering))
    }

    /// Attempt to transition the task from `IDLE` to `POLLING`, the returned enum indicates what
    /// to the with the task.
    ///
    /// This method should always be followed by a call to [`Self::end_poll`] after the actual poll
    /// is completed.
    pub(super) fn start_poll(&self) -> StartPollAction {
        let mut should_wait_for_join_waker = false;
        let action = self.transition(|s| {
            // cannot start polling a task which is being polled on another
            // thread, or a task which has completed
            if s.get(Snapshot::POLLING) || s.get(Snapshot::COMPLETE) {
                return StartPollAction::DontPoll;
            }

            // if the task has been canceled, don't poll it.
            if s.get(Snapshot::CANCELLED) {
                let wake_join_waker = s.has_join_waker(&mut should_wait_for_join_waker);
                return StartPollAction::Cancelled { wake_join_waker };
            }

            s
                // the task is now being polled.
                .set(Snapshot::POLLING, true)
                // if the task was woken, consume the wakeup.
                .set(Snapshot::WOKEN, false);

            StartPollAction::Poll
        });

        if should_wait_for_join_waker {
            debug_assert!(matches!(action, StartPollAction::Cancelled { .. }));
            todo!("wait for join waker")
            // self.wait_for_join_waker(self.load(Ordering::Acquire));
        }

        action
    }

    /// Transition the task from `POLLING` to `IDLE`, the returned enum indicates what to do with task.
    /// The `completed` argument should be set to true if the polled future returned a `Poll::Ready`
    /// indicating the task is completed and should not be rescheduled.
    pub(super) fn end_poll(&self, completed: bool) -> PollResult {
        // tracing::trace!(completed, "State::end_poll");

        let mut should_wait_for_join_waker = false;
        let action = self.transition(|s| {
            // Cannot end a poll if a task is not being polled!
            debug_assert!(s.get(Snapshot::POLLING));
            debug_assert!(!s.get(Snapshot::COMPLETE));
            debug_assert!(
                s.ref_count() > 0,
                "cannot poll a task that has zero references, what is happening!"
            );

            s.set(Snapshot::POLLING, false)
                .set(Snapshot::COMPLETE, completed);

            // Was the task woken during the poll?
            if !completed && s.get(Snapshot::WOKEN) {
                return PollResult::PendingSchedule;
            }

            let had_join_waker = if completed {
                // set the output flag so that the JoinHandle knows it is now
                // safe to read the task's output.
                s.set(Snapshot::HAS_OUTPUT, true);
                s.has_join_waker(&mut should_wait_for_join_waker)
            } else {
                false
            };

            if had_join_waker {
                PollResult::ReadyJoined
            } else if completed {
                PollResult::Ready
            } else {
                PollResult::Pending
            }
        });

        if should_wait_for_join_waker {
            debug_assert_eq!(action, PollResult::ReadyJoined);
            todo!("wait for join waker")
            // self.wait_for_join_waker(self.load(Ordering::Acquire));
        }

        action
    }

    pub(super) fn try_join(&self) -> JoinAction {
        // tracing::trace!("State::try_join");

        fn should_register(s: &mut Snapshot) -> JoinAction {
            let action = match s.get(Snapshot::JOIN_WAKER) {
                JoinWakerState::Empty => JoinAction::Register,
                x => {
                    debug_assert_eq!(x, JoinWakerState::Waiting);
                    JoinAction::Reregister
                }
            };
            s.set(Snapshot::JOIN_WAKER, JoinWakerState::Registering);

            action
        }

        self.transition(|s| {
            let has_output = s.get(Snapshot::HAS_OUTPUT);

            if s.get(Snapshot::CANCELLED) {
                return JoinAction::Canceled {
                    completed: has_output,
                };
            }

            // If the task has not completed, we can't take its join output.
            if !s.get(Snapshot::COMPLETE) {
                return should_register(s);
            }

            // If the task does not have output, we cannot take it.
            if !has_output {
                return should_register(s);
            }

            *s = s.with(Snapshot::HAS_OUTPUT, false);
            JoinAction::TakeOutput
        })
    }

    pub(super) fn join_waker_registered(&self) {
        // tracing::trace!("State::join_waker_registered");

        self.transition(|s| {
            debug_assert_eq!(s.get(Snapshot::JOIN_WAKER), JoinWakerState::Registering);
            s.set(Snapshot::HAS_JOIN_HANDLE, true)
                .set(Snapshot::JOIN_WAKER, JoinWakerState::Waiting);
        });
    }

    pub(super) fn wake_by_val(&self) -> WakeByValAction {
        // tracing::trace!("State::wake_by_val");

        self.transition(|s| {
            // If the task was woken *during* a poll, it will be re-queued by the
            // scheduler at the end of the poll if needed, so don't enqueue it now.
            if s.get(Snapshot::POLLING) {
                *s = s.with(Snapshot::WOKEN, true).drop_ref();
                assert!(s.ref_count() > 0);

                return WakeByValAction::None;
            }

            // If the task is already completed or woken, we don't need to
            // requeue it, but decrement the ref count for the waker that was
            // used for this wakeup.
            if s.get(Snapshot::COMPLETE) || s.get(Snapshot::WOKEN) {
                let new_state = s.drop_ref();
                *s = new_state;
                return if new_state.ref_count() == 0 {
                    WakeByValAction::Drop
                } else {
                    WakeByValAction::None
                };
            }

            // Otherwise, transition to the woken state and enqueue the task.
            *s = s.with(Snapshot::WOKEN, true).clone_ref();
            WakeByValAction::Enqueue
        })
    }

    pub(super) fn wake_by_ref(&self) -> WakeByRefAction {
        // tracing::trace!("State::wake_by_ref");

        self.transition(|state| {
            if state.get(Snapshot::COMPLETE) || state.get(Snapshot::WOKEN) {
                return WakeByRefAction::None;
            }

            if state.get(Snapshot::POLLING) {
                state.set(Snapshot::WOKEN, true);
                return WakeByRefAction::None;
            }

            // Otherwise, transition to the woken state and enqueue the task.
            *state = state.with(Snapshot::WOKEN, true).clone_ref();
            WakeByRefAction::Enqueue
        })
    }

    pub(super) fn clone_ref(&self) {
        // tracing::trace!("State::clone_ref");

        // Using a relaxed ordering is alright here, as knowledge of the
        // original reference prevents other threads from erroneously deleting
        // the object.
        //
        // As explained in the [Boost documentation][1], Increasing the
        // reference counter can always be done with memory_order_relaxed: New
        // references to an object can only be formed from an existing
        // reference, and passing an existing reference from one thread to
        // another must already provide any required synchronization.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        let old_refs = self.val.fetch_add(REF_ONE, Ordering::Relaxed);
        Snapshot::REFS.unpack(old_refs);

        // However we need to guard against massive refcounts in case someone
        // is `mem::forget`ing tasks. If we don't do this the count can overflow
        // and users will use-after free. We racily saturate to `isize::MAX` on
        // the assumption that there aren't ~2 billion threads incrementing
        // the reference count at once. This branch will never be taken in
        // any realistic program.
        //
        // We abort because such a program is incredibly degenerate, and we
        // don't care to support it.
        assert!(old_refs < REF_MAX, "task reference count overflow");
    }

    pub(super) fn drop_ref(&self) -> bool {
        // tracing::trace!("State::drop_ref");

        // We do not need to synchronize with other cores unless we are going to
        // delete the task.
        let old_refs = self.val.fetch_sub(REF_ONE, Ordering::Release);
        let old_refs = Snapshot::REFS.unpack(old_refs);

        // Did we drop the last ref?
        if old_refs > 1 {
            return false;
        }

        atomic::fence(Ordering::Acquire);
        true
    }

    /// Cancel the task.
    ///
    /// Returns `true` if the task was successfully canceled.
    pub(super) fn cancel(&self) -> bool {
        tracing::trace!("State::cancel");

        self.transition(|s| {
            // you can't cancel a task that has already been canceled, that doesn't make sense.
            if s.get(Snapshot::CANCELLED) {
                return false;
            }

            s.set(Snapshot::CANCELLED, true).set(Snapshot::WOKEN, true);

            true
        })
    }

    pub(super) fn create_join_handle(&self) {
        tracing::trace!("State::create_join_handle");

        self.transition(|s| {
            debug_assert!(
                !s.get(Snapshot::HAS_JOIN_HANDLE),
                "task already has a join handle, cannot create a new one! state={s:?}"
            );

            *s = s.with(Snapshot::HAS_JOIN_HANDLE, true);
        });
    }

    pub(super) fn drop_join_handle(&self) {
        tracing::trace!("State::drop_join_handle");

        const MASK: usize = !Snapshot::HAS_JOIN_HANDLE.raw_mask();
        let _prev = self.val.fetch_and(MASK, Ordering::Release);
        tracing::trace!(
            "drop_join_handle; prev_state:\n{}\nstate:\n{}",
            Snapshot::from_bits(_prev),
            self.load(Ordering::Acquire),
        );
        debug_assert!(
            Snapshot(_prev).get(Snapshot::HAS_JOIN_HANDLE),
            "tried to drop a join handle when the task did not have a join handle!\nstate: {:#?}",
            Snapshot(_prev),
        );
    }

    fn transition<T>(&self, mut transition: impl FnMut(&mut Snapshot) -> T) -> T {
        let mut current = self.load(Ordering::Acquire);
        loop {
            tracing::trace!("State::transition; current:\n{}", current);
            let mut next = current;
            // Run the transition function.
            let res = transition(&mut next);

            if current.0 == next.0 {
                return res;
            }

            tracing::trace!("State::transition; next:\n{}", next);
            match self.val.compare_exchange_weak(
                current.0,
                next.0,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return res,
                Err(actual) => current = Snapshot(actual),
            }
        }
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.load(Ordering::Relaxed).fmt(f)
    }
}

impl Snapshot {
    pub fn ref_count(self) -> usize {
        Snapshot::REFS.unpack(self.0)
    }

    fn drop_ref(self) -> Self {
        Self(self.0 - REF_ONE)
    }

    fn clone_ref(self) -> Self {
        Self(self.0 + REF_ONE)
    }

    fn has_join_waker(&mut self, should_wait: &mut bool) -> bool {
        match self.get(Snapshot::JOIN_WAKER) {
            JoinWakerState::Empty => false,
            JoinWakerState::Registering => {
                *should_wait = true;
                debug_assert!(
                    self.get(Snapshot::HAS_JOIN_HANDLE),
                    "a task cannot register a join waker if it does not have a join handle!",
                );
                true
            }
            JoinWakerState::Waiting => {
                debug_assert!(
                    self.get(Snapshot::HAS_JOIN_HANDLE),
                    "a task cannot have a join waker if it does not have a join handle!",
                );
                *should_wait = false;
                self.set(Snapshot::JOIN_WAKER, JoinWakerState::Empty);
                true
            }
            JoinWakerState::Woken => {
                debug_assert!(
                    false,
                    "join waker should not be woken until task has completed, wtf"
                );
                false
            }
        }
    }
}

impl mycelium_bitfield::FromBits<usize> for JoinWakerState {
    type Error = core::convert::Infallible;

    /// The number of bits required to represent a value of this type.
    const BITS: u32 = 2;

    #[inline]
    fn try_from_bits(bits: usize) -> Result<Self, Self::Error> {
        match bits {
            b if b == Self::Registering as usize => Ok(Self::Registering),
            b if b == Self::Waiting as usize => Ok(Self::Waiting),
            b if b == Self::Empty as usize => Ok(Self::Empty),
            b if b == Self::Woken as usize => Ok(Self::Woken),
            _ => {
                // this should never happen unless the bitpacking code is broken
                unreachable!("invalid join waker state {bits:#b}")
            }
        }
    }

    #[inline]
    fn into_bits(self) -> usize {
        self as u8 as usize
    }
}
