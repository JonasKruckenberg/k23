// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::sync::Closed;
use crate::sync::wake_batch::WakeBatch;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};
use core::{fmt, mem, ptr};
use mycelium_bitfield::{FromBits, bitfield, enum_from_bits};
use pin_project::{pin_project, pinned_drop};
use spin::{Mutex, MutexGuard};
use util::CachePadded;

/// A queue of waiting tasks which can be [woken in first-in, first-out
/// order][wake], or [all at once][wake_all].
///
/// This type is taken from [maitake-sync](https://github.com/hawkw/mycelium/blob/dd0020892564c77ee4c20ffbc2f7f5b046ad54c8/maitake-sync/src/wait_queue.rs#L1577).
///
/// A `WaitQueue` allows any number of tasks to [wait] asynchronously and be
/// woken when some event occurs, either [individually][wake] in first-in,
/// first-out order, or [all at once][wake_all]. This makes it a vital building
/// block of runtime services (such as timers or I/O resources), where it may be
/// used to wake a set of tasks when a timer completes or when a resource
/// becomes available. It can be equally useful for implementing higher-level
/// synchronization primitives: for example, a `WaitQueue` plus an
/// [`UnsafeCell`] is essentially an entire implementation of a fair
/// asynchronous mutex. Finally, a `WaitQueue` can be a useful
/// synchronization primitive on its own: sometimes, you just need to have a
/// bunch of tasks wait for something and then wake them all up.
///
/// # Implementation Notes
///
/// This type is currently implemented using [intrusive doubly-linked
/// list][ilist].
///
/// The *[intrusive]* aspect of this map is important, as it means that it does
/// not allocate memory. Instead, nodes in the linked list are stored in the
/// futures of tasks trying to wait for capacity. This means that it is not
/// necessary to allocate any heap memory for each task waiting to be woken.
///
/// However, the intrusive linked list introduces one new danger: because
/// futures can be *cancelled*, and the linked list nodes live within the
/// futures trying to wait on the queue, we *must* ensure that the node
/// is unlinked from the list before dropping a cancelled future. Failure to do
/// so would result in the list containing dangling pointers. Therefore, we must
/// use a *doubly-linked* list, so that nodes can edit both the previous and
/// next node when they have to remove themselves. This is kind of a bummer, as
/// it means we can't use something nice like this [intrusive queue by Dmitry
/// Vyukov][2], and there are not really practical designs for lock-free
/// doubly-linked lists that don't rely on some kind of deferred reclamation
/// scheme such as hazard pointers or QSBR.
///
/// Instead, we just stick a [`Mutex`] around the linked list, which must be
/// acquired to pop nodes from it, or for nodes to remove themselves when
/// futures are cancelled. This is a bit sad, but the critical sections for this
/// mutex are short enough that we still get pretty good performance despite it.
///
/// [intrusive]: https://fuchsia.dev/fuchsia-src/development/languages/c-cpp/fbl_containers_guide/introduction
/// [2]: https://www.1024cores.net/home/lock-free-algorithms/queues/intrusive-mpsc-node-based-queue
/// [`Waker`]: Waker
/// [wait]: WaitQueue::wait
/// [wake]: WaitQueue::wake
/// [wake_all]: WaitQueue::wake_all
/// [`UnsafeCell`]: UnsafeCell
/// [ilist]: linked_list::List
#[derive(Debug)]
pub struct WaitQueue {
    /// The wait maps's state variable.
    state: CachePadded<AtomicUsize>,
    /// The linked list of waiters.
    ///
    /// # Safety
    ///
    /// This is protected by a mutex; the mutex *must* be acquired when
    /// manipulating the linked list, OR when manipulating waiter nodes that may
    /// be linked into the list. If a node is known to not be linked, it is safe
    /// to modify that node (such as by waking the stored [`Waker`]) without
    /// holding the lock; otherwise, it may be modified through the list, so the
    /// lock must be held when modifying the
    /// node.
    queue: Mutex<linked_list::List<Waiter>>,
}

bitfield! {
    #[derive(Eq, PartialEq)]
    struct State<usize> {
        /// The queue's state.
        const INNER: StateInner;

        /// The number of times [`WaitQueue::wake_all`] has been called.
        const WAKE_ALLS = ..;
    }
}

/// The queue's current state.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
enum StateInner {
    /// No waiters are queued, and there is no pending notification.
    /// Waiting while the queue is in this state will enqueue the waiter;
    /// notifying while in this state will store a pending notification in the
    /// queue, transitioning to [`StateInner::Woken`].
    Empty = 0b00,
    /// There are one or more waiters in the queue. Waiting while
    /// the queue is in this state will not transition the state. Waking while
    /// in this state will wake the first waiter in the queue; if this empties
    /// the queue, then the queue will transition to [`StateInner::Empty`].
    Waiting = 0b01,
    /// The queue has a stored notification. Waiting while the queue
    /// is in this state will consume the pending notification *without*
    /// enqueueing the waiter and transition the queue to [`StateInner::Empty`].
    /// Waking while in this state will leave the queue in this state.
    Woken = 0b10,
    /// The queue is closed. Waiting while in this state will return
    /// [`Closed`] without transitioning the queue's state.
    ///
    /// *Note*: This *must* correspond to all state bits being set, as it's set
    /// via a [`fetch_or`].
    ///
    /// [`fetch_or`]: AtomicUsize::fetch_or
    Closed = 0b11,
}

#[derive(Debug)]
#[pin_project(PinnedDrop)]
#[must_use = "futures do nothing unless `.await`ed or `poll`ed"]
pub struct Wait<'a> {
    /// The [`WaitQueue`] being waited on.
    queue: &'a WaitQueue,
    /// Entry in the wait queue linked list.
    #[pin]
    waiter: Waiter,
}

/// Future returned from [`WaitQueue::wait_owned()`].
///
/// This is identical to the [`Wait`] future, except that it takes an
/// [`Arc`] reference to the [`WaitQueue`], allowing the returned future to
/// live for the `'static` lifetime.
///
/// This future is fused, so once it has completed, any future calls to poll
/// will immediately return [`Poll::Ready`].
///
/// # Notes
///
/// This future is `!Unpin`, as it is unsafe to [`core::mem::forget`] a
/// `WaitOwned`  future once it has been polled. For instance, the following
/// code must not compile:
///
///```compile_fail
/// use maitake_sync::wait_queue::WaitOwned;
///
/// // Calls to this function should only compile if `T` is `Unpin`.
/// fn assert_unpin<T: Unpin>() {}
///
/// assert_unpin::<WaitOwned<'_>>();
/// ```
#[derive(Debug)]
#[pin_project(PinnedDrop)]
pub struct WaitOwned {
    /// The `WaitQueue` being waited on.
    queue: Arc<WaitQueue>,
    /// Entry in the wait queue.
    #[pin]
    waiter: Waiter,
}

/// A waiter node which may be linked into a wait queue.
#[repr(C)]
#[pin_project]
struct Waiter {
    /// The intrusive linked list node.
    ///
    /// This *must* be the first field in the struct in order for the `Linked`
    /// implementation to be sound.
    #[pin]
    node: UnsafeCell<WaiterInner>,
    /// The future's state.
    state: WaitState,
}

struct WaiterInner {
    /// Intrusive linked list pointers.
    links: linked_list::Links<Waiter>,
    /// The node's waker
    waker: Wakeup,
    // This type is !Unpin due to the heuristic from:
    // <https://github.com/rust-lang/rust/pull/82834>
    _pin: PhantomPinned,
}

bitfield! {
    #[derive(Eq, PartialEq)]
    struct WaitState<usize> {
        /// The waiter's state.
        const INNER: WaitStateInner;
        /// The number of times [`WaitQueue::wake_all`] has been called.
        const WAKE_ALLS = ..;
    }
}

enum_from_bits! {
    /// The state of a [`Waiter`] node in a [`WaitQueue`].
    #[derive(Debug, Eq, PartialEq)]
    enum WaitStateInner<u8> {
        /// The waiter has not yet been enqueued.
        ///
        /// The number of times [`WaitQueue::wake_all`] has been called is stored
        /// when the node is created, in order to determine whether it was woken by
        /// a stored wakeup when enqueueing.
        ///
        /// When in this state, the node is **not** part of the linked list, and
        /// can be dropped without removing it from the list.
        Start = 0b00,

        /// The waiter is waiting.
        ///
        /// When in this state, the node **is** part of the linked list. If the
        /// node is dropped in this state, it **must** be removed from the list
        /// before dropping it. Failure to ensure this will result in dangling
        /// pointers in the linked list!
        Waiting = 0b01,

        /// The waiter has been woken.
        ///
        /// When in this state, the node is **not** part of the linked list, and
        /// can be dropped without removing it from the list.
        Woken = 0b10,
    }
}

#[derive(Clone, Debug)]
enum Wakeup {
    Empty,
    Waiting(Waker),
    One,
    All,
    // Closed,
}

// === impl WaitQueue ===

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            state: CachePadded(AtomicUsize::new(StateInner::Empty.into_usize())),
            queue: Mutex::new(linked_list::List::new()),
        }
    }

    /// Wake the next task in the queue.
    ///
    /// If the queue is empty, a wakeup is stored in the `WaitQueue`, and the
    /// **next** call to [`wait().await`] will complete immediately. If one or more
    /// tasks are currently in the queue, the first task in the queue is woken.
    ///
    /// At most one wakeup will be stored in the queue at any time. If `wake()`
    /// is called many times while there are no tasks in the queue, only a
    /// single wakeup is stored.
    ///
    /// [`wait().await`]: Self::wait()
    #[inline]
    pub fn wake(&self) {
        // snapshot the queue's current state.
        let mut state = self.load();

        // check if any tasks are currently waiting on this queue. if there are
        // no waiting tasks, store the wakeup to be consumed by the next call to
        // `wait`.
        loop {
            match state.get(State::INNER) {
                // if the queue is closed, bail.
                StateInner::Closed => return,
                // if there are waiting tasks, break out of the loop and wake one.
                StateInner::Waiting => break,
                _ => {}
            }

            let next = state.with_inner(StateInner::Woken);
            // advance the state to `Woken`, and return (if we did so
            // successfully)
            match self.compare_exchange(state, next) {
                Ok(_) => return,
                Err(actual) => state = actual,
            }
        }

        // okay, there are tasks waiting on the queue; we must acquire the lock
        // on the linked list and wake the next task from the queue.
        let mut queue = self.queue.lock();

        // the queue's state may have changed while we were waiting to acquire
        // the lock, so we need to acquire a new snapshot before we take the
        // waker.
        state = self.load();
        let waker = self.wake_locked(&mut queue, state);
        drop(queue);

        //now that we've released the lock, wake the waiting task (if we
        //actually deuqueued one).
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Wake *all* tasks currently in the queue.
    ///
    /// All tasks currently waiting on the queue are woken. Unlike [`wake()`], a
    /// wakeup is *not* stored in the queue to wake the next call to [`wait()`]
    /// if the queue is empty. Instead, this method only wakes all currently
    /// registered waiters. Registering a task to be woken is done by `await`ing
    /// the [`Future`] returned by the [`wait()`] method on this queue.
    ///
    /// [`wake()`]: Self::wake
    /// [`wait()`]: Self::wait
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn wake_all(&self) {
        let mut batch = WakeBatch::new();
        let mut waiters_remaining = true;

        let mut queue = self.queue.lock();
        let state = self.load();

        match state.get(State::INNER) {
            StateInner::Woken | StateInner::Empty => {
                self.state
                    .0
                    .fetch_add(State::ONE_WAKE_ALL, Ordering::SeqCst);
                return;
            }
            StateInner::Closed => return,
            StateInner::Waiting => {
                let next_state = State::new()
                    .with_inner(StateInner::Empty)
                    .with(State::WAKE_ALLS, state.get(State::WAKE_ALLS) + 1);
                self.compare_exchange(state, next_state)
                    .expect("state should not have transitioned while locked");
            }
        }

        // As long as there are waiters remaining to wake, lock the queue, drain
        // another batch, release the lock, and wake them.
        while waiters_remaining {
            waiters_remaining = Self::drain_to_wake_batch(&mut batch, &mut queue, Wakeup::All);
            MutexGuard::unlocked(&mut queue, || batch.wake_all());
        }
    }

    /// Wait to be woken up by this queue.
    ///
    /// Equivalent to:
    ///
    /// ```ignore
    /// async fn wait(&self);
    /// ```
    ///
    /// This returns a [`Wait`] [`Future`] that will complete when the task is
    /// woken by a call to [`wake()`] or [`wake_all()`], or when the `WaitQueue`
    /// is dropped.
    ///
    /// Each `WaitQueue` holds a single wakeup. If [`wake()`] was previously
    /// called while no tasks were waiting on the queue, then `wait().await`
    /// will complete immediately, consuming the stored wakeup. Otherwise,
    /// `wait().await` waits to be woken by the next call to [`wake()`] or
    /// [`wake_all()`].
    ///
    /// The [`Wait`] future is not guaranteed to receive wakeups from calls to
    /// [`wake()`] if it has not yet been polled. See the documentation for the
    /// [`Wait::subscribe()`] method for details on receiving wakeups from the
    /// queue prior to polling the `Wait` future for the first time.
    ///
    /// A `Wait` future **is** is guaranteed to receive wakeups from calls to
    /// [`wake_all()`] as soon as it is created, even if it has not yet been
    /// polled.
    ///
    /// # Returns
    ///
    /// The [`Future`] returned by this method completes with one of the
    /// following [outputs](Future::Output):
    ///
    /// - [`Ok`]`(())` if the task was woken by a call to [`wake()`] or
    ///   [`wake_all()`].
    /// - [`Err`]`(`[`Closed`]`)` if the task was woken by the `WaitQueue` being
    ///   [`close`d](WaitQueue::close).
    ///
    /// # Cancellation
    ///
    /// A `WaitQueue` fairly distributes wakeups to waiting tasks in the order
    /// that they started to wait. If a [`Wait`] future is dropped, the task
    /// will forfeit its position in the queue.
    ///
    /// [`wake()`]: Self::wake
    /// [`wake_all()`]: Self::wake_all
    pub fn wait(&self) -> Wait<'_> {
        Wait {
            queue: self,
            waiter: self.waiter(),
        }
    }

    /// Wait to be woken up by this queue, returning a future that's valid
    /// for the `'static` lifetime.
    ///
    /// This returns a [`WaitOwned`] future that will complete when the task
    /// is woken by a call to [`wake()`] or [`wake_all()`], or when the
    /// `WaitQueue` is [closed].
    ///
    /// This is identical to the [`wait()`] method, except that it takes a
    /// [`Arc`] reference to the [`WaitQueue`], allowing the returned future
    /// to live for the `'static` lifetime. See the documentation for
    /// [`wait()`] for details on how to use the future returned by this
    /// method.
    ///
    /// # Returns
    ///
    /// The [`Future`] returned by this method completes with one of the
    /// following [outputs](Future::Output):
    ///
    /// - [`Ok`]`(())` if the task was woken by a call to [`wake()`] or
    ///   [`wake_all()`].
    /// - [`Err`]`(`[`Closed`]`)` if the task was woken by the `WaitQueue`
    ///   being [closed].
    ///
    /// # Cancellation
    ///
    /// A `WaitQueue` fairly distributes wakeups to waiting tasks in the
    /// order that they started to wait. If a [`WaitOwned`] future is
    /// dropped, the task will forfeit its position in the queue.
    ///
    /// [`wake()`]: Self::wake
    /// [`wake_all()`]: Self::wake_all
    /// [`wait()`]: Self::wait
    /// [closed]: Self::close
    pub fn wait_owned(self: &Arc<Self>) -> WaitOwned {
        let waiter = self.waiter();
        let queue = self.clone();
        WaitOwned { queue, waiter }
    }

    /// Asynchronously poll the given function `f` until a condition occurs,
    /// using the [`WaitQueue`] to only re-poll when notified.
    ///
    /// This can be used to implement a "wait loop", turning a "try" function
    /// (e.g. "try_recv" or "try_send") into an asynchronous function (e.g.
    /// "recv" or "send").
    ///
    /// In particular, this function correctly *registers* interest in the [`WaitQueue`]
    /// prior to polling the function, ensuring that there is not a chance of a race
    /// where the condition occurs AFTER checking but BEFORE registering interest
    /// in the [`WaitQueue`], which could lead to deadlock.
    ///
    /// This is intended to have similar behavior to `Condvar` in the standard library,
    /// but asynchronous, and not requiring operating system intervention (or existence).
    ///
    /// In particular, this can be used in cases where interrupts or events are used
    /// to signify readiness or completion of some task, such as the completion of a
    /// DMA transfer, or reception of an ethernet frame. In cases like this, the interrupt
    /// can wake the queue, allowing the polling function to check status fields for
    /// partial progress or completion.
    ///
    /// Consider using [`Self::wait_for_value()`] if your function does return a value.
    ///
    /// Consider using [`WaitCell::wait_for()`](super::wait_cell::WaitCell::wait_for)
    /// if you do not need multiple waiters.
    ///
    /// # Errors
    ///
    /// Returns [`Err`]`(`[`Closed`]`)` if the [`WaitQueue`] is closed.
    ///
    pub async fn wait_for<F: FnMut() -> bool>(&self, mut f: F) -> Result<(), Closed> {
        loop {
            let wait = self.wait();
            let mut wait = core::pin::pin!(wait);
            let _ = wait.as_mut().subscribe()?;
            if f() {
                return Ok(());
            }
            wait.await?;
        }
    }

    /// Asynchronously poll the given function `f` until a condition occurs,
    /// using the [`WaitQueue`] to only re-poll when notified.
    ///
    /// This can be used to implement a "wait loop", turning a "try" function
    /// (e.g. "try_recv" or "try_send") into an asynchronous function (e.g.
    /// "recv" or "send").
    ///
    /// In particular, this function correctly *registers* interest in the [`WaitQueue`]
    /// prior to polling the function, ensuring that there is not a chance of a race
    /// where the condition occurs AFTER checking but BEFORE registering interest
    /// in the [`WaitQueue`], which could lead to deadlock.
    ///
    /// This is intended to have similar behavior to `Condvar` in the standard library,
    /// but asynchronous, and not requiring operating system intervention (or existence).
    ///
    /// In particular, this can be used in cases where interrupts or events are used
    /// to signify readiness or completion of some task, such as the completion of a
    /// DMA transfer, or reception of an ethernet frame. In cases like this, the interrupt
    /// can wake the queue, allowing the polling function to check status fields for
    /// partial progress or completion, and also return the status flags at the same time.
    ///
    /// Consider using [`Self::wait_for()`] if your function does not return a value.
    ///
    /// Consider using [`WaitCell::wait_for_value()`](super::wait_cell::WaitCell::wait_for_value)
    /// if you do not need multiple waiters.
    ///
    /// # Errors
    ///
    /// Returns [`Err`]`(`[`Closed`]`)` if the [`WaitQueue`] is closed.
    ///
    pub async fn wait_for_value<T, F: FnMut() -> Option<T>>(&self, mut f: F) -> Result<T, Closed> {
        loop {
            let wait = self.wait();
            let mut wait = core::pin::pin!(wait);
            match wait.as_mut().subscribe() {
                Poll::Ready(wr) => wr?,
                Poll::Pending => {}
            }
            if let Some(t) = f() {
                return Ok(t);
            }
            wait.await?;
        }
    }

    /// Close the queue, indicating that it may no longer be used.
    ///
    /// Once a queue is closed, all [`wait()`] calls (current or future) will
    /// return an error.
    ///
    /// This method is generally used when implementing higher-level
    /// synchronization primitives or resources: when an event makes a resource
    /// permanently unavailable, the queue can be closed.
    ///
    /// [`wait()`]: Self::wait
    pub fn close(&self) {
        let state = self
            .state
            .0
            .fetch_or(StateInner::Closed.into_bits(), Ordering::SeqCst);
        let state = State::from_bits(state);
        if state.get(State::INNER) != StateInner::Waiting {
            return;
        }

        let mut queue = self.queue.lock();
        let mut batch = WakeBatch::new();
        let mut waiters_remaining = true;

        while waiters_remaining {
            waiters_remaining = Self::drain_to_wake_batch(&mut batch, &mut queue, Wakeup::All);
            MutexGuard::unlocked(&mut queue, || batch.wake_all());
        }
    }

    /// Returns `true` if this `WaitQueue` is [closed](Self::close).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.load().get(State::INNER) == StateInner::Closed
    }

    /// Returns a [`Waiter`] entry in this queue.
    ///
    /// This is factored out into a separate function because it's used by both
    /// [`WaitQueue::wait`] and [`WaitQueue::wait_owned`].
    fn waiter(&self) -> Waiter {
        // how many times has `wake_all` been called when this waiter is created?
        let current_wake_alls = self.load().get(State::WAKE_ALLS);
        let state = WaitState::new()
            .with(WaitState::WAKE_ALLS, current_wake_alls)
            .with(WaitState::INNER, WaitStateInner::Start);
        Waiter {
            state,
            node: UnsafeCell::new(WaiterInner {
                links: linked_list::Links::new(),
                waker: Wakeup::Empty,
                _pin: PhantomPinned,
            }),
        }
    }

    // fn try_wait(&self) -> Poll<Result<(), Closed>> {
    //     let mut state = self.load();
    //     let initial_wake_alls = state.get(State::WAKE_ALLS);
    //     while state.get(State::INNER) == StateInner::Woken {
    //         match self.compare_exchange(state, state.with_inner(StateInner::Empty)) {
    //             Ok(_) => return Poll::Ready(Ok(())),
    //             Err(actual) => state = actual,
    //         }
    //     }
    //
    //     match state.get(State::INNER) {
    //         StateInner::Closed => Poll::Ready(Err(Closed(()))),
    //         _ if state.get(State::WAKE_ALLS) > initial_wake_alls => Poll::Ready(Ok(())),
    //         StateInner::Empty | StateInner::Waiting => Poll::Pending,
    //         StateInner::Woken => Poll::Ready(Ok(())),
    //     }
    // }

    #[cold]
    #[inline(never)]
    fn wake_locked(&self, queue: &mut linked_list::List<Waiter>, curr: State) -> Option<Waker> {
        let inner = curr.get(State::INNER);

        // is the queue still in the `Waiting` state? it is possible that we
        // transitioned to a different state while locking the queue.
        if inner != StateInner::Waiting {
            // if there are no longer any queued tasks, try to store the
            // wakeup in the queue and bail.
            if let Err(actual) = self.compare_exchange(curr, curr.with_inner(StateInner::Woken)) {
                debug_assert!(actual.get(State::INNER) != StateInner::Waiting);
                self.store(actual.with_inner(StateInner::Woken));
            }

            return None;
        }

        // otherwise, we have to dequeue a task and wake it.
        let node = queue
            .pop_back()
            .expect("if we are in the Waiting state, there must be waiters in the queue");
        let waker = Waiter::wake(node, queue, Wakeup::One);

        // if we took the final waiter currently in the queue, transition to the
        // `Empty` state.
        if queue.is_empty() {
            self.store(curr.with_inner(StateInner::Empty));
        }

        waker
    }

    /// Drain waiters from `queue` and add them to `batch`. Returns `true` if
    /// the batch was filled while more waiters remain in the queue, indicating
    /// that this function must be called again to wake all waiters.
    fn drain_to_wake_batch(
        batch: &mut WakeBatch,
        queue: &mut linked_list::List<Waiter>,
        wakeup: Wakeup,
    ) -> bool {
        while let Some(node) = queue.pop_back() {
            let Some(waker) = Waiter::wake(node, queue, wakeup.clone()) else {
                // this waiter was enqueued by `Wait::register` and doesn't have
                // a waker, just keep going.
                continue;
            };

            if batch.add_waker(waker) {
                // there's still room in the wake set, just keep adding to it.
                continue;
            }

            // wake set is full, drop the lock and wake everyone!
            break;
        }

        !queue.is_empty()
    }

    fn load(&self) -> State {
        State::from_bits(self.state.0.load(Ordering::SeqCst))
    }

    fn store(&self, state: State) {
        self.state.0.store(state.0, Ordering::SeqCst);
    }

    fn compare_exchange(&self, current: State, new: State) -> Result<State, State> {
        self.state
            .0
            .compare_exchange(current.0, new.0, Ordering::SeqCst, Ordering::SeqCst)
            .map(State::from_bits)
            .map_err(State::from_bits)
    }
}

// === impl State & StateInner ===

impl State {
    const ONE_WAKE_ALL: usize = Self::WAKE_ALLS.first_bit();

    fn with_inner(self, inner: StateInner) -> Self {
        self.with(Self::INNER, inner)
    }
}
impl StateInner {
    const fn into_usize(self) -> usize {
        self as u8 as usize
    }
}

impl FromBits<usize> for StateInner {
    const BITS: u32 = 2;
    type Error = core::convert::Infallible;

    fn try_from_bits(bits: usize) -> Result<Self, Self::Error> {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "`bits` will only have 3 bits set"
        )]
        Ok(match bits as u8 {
            bits if bits == Self::Empty as u8 => Self::Empty,
            bits if bits == Self::Waiting as u8 => Self::Waiting,
            bits if bits == Self::Woken as u8 => Self::Woken,
            bits if bits == Self::Closed as u8 => Self::Closed,
            _ => {
                unreachable!();
            }
        })
    }

    fn into_bits(self) -> usize {
        self as usize
    }
}

// === impl Waiter ===

impl fmt::Debug for Waiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Waiter")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Waiter {
    /// Returns the [`Waker`] for the task that owns this `Waiter`.
    ///
    /// # Safety
    ///
    /// This is only safe to call while the list is locked. The `list`
    /// parameter ensures this method is only called while holding the lock, so
    /// this can be safe.
    ///
    /// Of course, that must be the *same* list that this waiter is a member of,
    /// and currently, there is no way to ensure that...
    #[inline(always)]
    fn wake(
        this: NonNull<Self>,
        list: &mut linked_list::List<Self>,
        wakeup: Wakeup,
    ) -> Option<Waker> {
        Waiter::with_inner(this, list, |node| {
            let waker = mem::replace(&mut node.waker, wakeup);
            match waker {
                // the node has a registered waker, so wake the task.
                Wakeup::Waiting(waker) => Some(waker),
                // do nothing: the node was registered by `Wait::register`
                // without a waker, so the future will already be woken when it is
                // actually polled.
                Wakeup::Empty => None,
                // the node was already woken? this should not happen and
                // probably indicates a race!
                _ => unreachable!("tried to wake a waiter in the {:?} state!", waker),
            }
        })
    }

    /// # Safety
    ///
    /// This is only safe to call while the list is locked. The dummy `_list`
    /// parameter ensures this method is only called while holding the lock, so
    /// this can be safe.
    ///
    /// Of course, that must be the *same* list that this waiter is a member of,
    /// and currently, there is no way to ensure that...
    #[inline(always)]
    fn with_inner<T>(
        mut this: NonNull<Self>,
        _list: &mut linked_list::List<Self>,
        f: impl FnOnce(&mut WaiterInner) -> T,
    ) -> T {
        // safety: this is only called while holding the lock on the queue,
        // so it's safe to mutate the waiter.
        unsafe { f(&mut *this.as_mut().node.get()) }
    }

    fn poll_wait(
        mut self: Pin<&mut Self>,
        queue: &WaitQueue,
        waker: Option<&Waker>,
    ) -> Poll<Result<(), Closed>> {
        // Safety: we never move out of `ptr` below, only mutate its fields
        let ptr = unsafe { NonNull::from(Pin::into_inner_unchecked(self.as_mut())) };
        let mut this = self.as_mut().project();

        match this.state.get(WaitState::INNER) {
            WaitStateInner::Start => {
                let queue_state = queue.load();

                // can we consume a pending wakeup?
                if queue
                    .compare_exchange(
                        queue_state.with_inner(StateInner::Woken),
                        queue_state.with_inner(StateInner::Empty),
                    )
                    .is_ok()
                {
                    this.state.set(WaitState::INNER, WaitStateInner::Woken);
                    return Poll::Ready(Ok(()));
                }

                // okay, no pending wakeups. try to wait...

                let mut waiters = queue.queue.lock();
                let mut queue_state = queue.load();

                // the whole queue was woken while we were trying to acquire
                // the lock!
                if queue_state.get(State::WAKE_ALLS) != this.state.get(WaitState::WAKE_ALLS) {
                    this.state.set(WaitState::INNER, WaitStateInner::Woken);
                    return Poll::Ready(Ok(()));
                }

                // transition the queue to the waiting state
                'to_waiting: loop {
                    match queue_state.get(State::INNER) {
                        // the queue is `Empty`, transition to `Waiting`
                        StateInner::Empty => {
                            match queue.compare_exchange(
                                queue_state,
                                queue_state.with_inner(StateInner::Waiting),
                            ) {
                                Ok(_) => break 'to_waiting,
                                Err(actual) => queue_state = actual,
                            }
                        }
                        // the queue is already `Waiting`
                        StateInner::Waiting => break 'to_waiting,
                        // the queue was woken, consume the wakeup.
                        StateInner::Woken => {
                            match queue.compare_exchange(
                                queue_state,
                                queue_state.with_inner(StateInner::Empty),
                            ) {
                                Ok(_) => {
                                    this.state.set(WaitState::INNER, WaitStateInner::Woken);
                                    return Poll::Ready(Ok(()));
                                }
                                Err(actual) => queue_state = actual,
                            }
                        }
                        StateInner::Closed => return Poll::Ready(Err(Closed(()))),
                    }
                }

                // enqueue the node
                this.state.set(WaitState::INNER, WaitStateInner::Waiting);
                if let Some(waker) = waker {
                    // safety: we may mutate the inner state because we are
                    // holding the lock.
                    unsafe {
                        let node = this.node.as_mut().get();
                        debug_assert!(matches!((*node).waker, Wakeup::Empty));
                        (*node).waker = Wakeup::Waiting(waker.clone());
                    }
                }

                waiters.push_front(ptr);

                Poll::Pending
            }
            WaitStateInner::Waiting => {
                let _waiters = queue.queue.lock();
                // safety: we may mutate the inner state because we are
                // holding the lock.
                unsafe {
                    let node = &mut *this.node.get();
                    match node.waker {
                        Wakeup::Waiting(ref mut curr_waker) => {
                            match waker {
                                Some(waker) if !curr_waker.will_wake(waker) => {
                                    *curr_waker = waker.clone();
                                }
                                _ => {}
                            }
                            Poll::Pending
                        }
                        Wakeup::All | Wakeup::One => {
                            this.state.set(WaitState::INNER, WaitStateInner::Woken);
                            Poll::Ready(Ok(()))
                        }
                        // Wakeup::Closed => {
                        //     this.state.set(WaitState::INNER, WaitStateInner::Woken);
                        //     Poll::Ready(Err(Closed(())))
                        // }
                        Wakeup::Empty => {
                            if let Some(waker) = waker {
                                node.waker = Wakeup::Waiting(waker.clone());
                            }

                            Poll::Pending
                        }
                    }
                }
            }
            WaitStateInner::Woken => Poll::Ready(Ok(())),
        }
    }

    /// Release this `Waiter` from the queue.
    ///
    /// This is called from the `drop` implementation for the [`Wait`] and
    /// [`WaitOwned`] futures.
    fn release(mut self: Pin<&mut Self>, queue: &WaitQueue) {
        let state = *(self.as_mut().project().state);
        // Safety: we never move out of `ptr` below, only mutate its fields
        let ptr = NonNull::from(unsafe { Pin::into_inner_unchecked(self) });

        // if we're not enqueued, we don't have to do anything else.
        if state.get(WaitState::INNER) != WaitStateInner::Waiting {
            return;
        }

        let mut waiters = queue.queue.lock();
        let state = queue.load();
        // remove the node
        // safety: we have the lock on the queue, so this is safe.
        unsafe {
            waiters.cursor_from_ptr_mut(ptr).remove();
        };

        // if we removed the last waiter from the queue, transition the state to
        // `Empty`.
        if waiters.is_empty() && state.get(State::INNER) == StateInner::Waiting {
            queue.store(state.with_inner(StateInner::Empty));
        }

        // if the node has an unconsumed wakeup, it must be assigned to the next
        // node in the queue.
        let next_waiter =
            if Waiter::with_inner(ptr, &mut waiters, |node| matches!(&node.waker, Wakeup::One)) {
                queue.wake_locked(&mut waiters, state)
            } else {
                None
            };

        drop(waiters);

        if let Some(next) = next_waiter {
            next.wake();
        }
    }
}

// Safety: TODO
unsafe impl linked_list::Linked for Waiter {
    type Handle = NonNull<Waiter>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(target: NonNull<Self>) -> NonNull<linked_list::Links<Waiter>> {
        // Safety: ensured by caller
        unsafe {
            // Safety: using `ptr::addr_of!` avoids creating a temporary
            // reference, which stacked borrows dislikes.
            let node = &*ptr::addr_of!((*target.as_ptr()).node);
            let links = ptr::addr_of_mut!((*node.get()).links);
            // Safety: since the `target` pointer is `NonNull`, we can assume
            // that pointers to its members are also not null, making this use
            // of `new_unchecked` fine.
            NonNull::new_unchecked(links)
        }
    }
}

// === impl Wait ===

impl Wait<'_> {
    /// Returns `true` if this `WaitOwned` future is waiting for a
    /// notification from the provided [`WaitQueue`].
    #[inline]
    #[must_use]
    pub fn waits_on(&self, queue: &WaitQueue) -> bool {
        ptr::eq(self.queue, queue)
    }

    /// Returns `true` if `self` and `other` are waiting on a notification
    /// from the same [`WaitQueue`].
    #[inline]
    #[must_use]
    pub fn same_queue(&self, other: &Wait<'_>) -> bool {
        ptr::eq(self.queue, other.queue)
    }

    /// Eagerly subscribe this future to wakeups from [`WaitQueue::wake()`].
    ///
    /// Polling a `Wait` future adds that future to the list of waiters that may
    /// receive a wakeup from a `WaitQueue`. However, in some cases, it is
    /// desirable to subscribe to wakeups *prior* to actually waiting for one.
    /// This method should be used when it is necessary to ensure a `Wait`
    /// future is in the list of waiters before the future is `poll`ed for the
    /// first time.
    ///
    /// In general, this method is used in cases where a [`WaitQueue`] must
    /// synchronize with some additional state, such as an `AtomicBool` or
    /// counter. If a task first checks that state, and then chooses whether or
    /// not to wait on the `WaitQueue` based on that state, then a race
    /// condition may occur where the `WaitQueue` wakes waiters *between* when
    /// the task checked the external state and when it first polled its `Wait`
    /// future to wait on the queue. This method allows registering the `Wait`
    /// future with the queue *prior* to checking the external state, without
    /// actually sleeping, so that when the task does wait for the `Wait` future
    /// to complete, it will have received any wakeup that was sent between when
    /// the external state was checked and the `Wait` future was first polled.
    ///
    /// # Returns
    ///
    /// This method returns a [`Poll`]`<`[`Result<(), Closed>`]`>` which is `Ready` a wakeup was
    /// already received. This method returns [`Poll::Ready`] in the following
    /// cases:
    ///
    ///  1. The [`WaitQueue::wake()`] method was called between the creation of the
    ///     `Wait` and the call to this method.
    ///  2. This is the first call to `subscribe` or `poll` on this future, and the
    ///     `WaitQueue` was holding a stored wakeup from a previous call to
    ///     [`wake()`]. This method consumes the wakeup in that case.
    ///  3. The future has previously been `subscribe`d or polled, and it has since
    ///     then been marked ready by either consuming a wakeup from the
    ///     `WaitQueue`, or by a call to [`wake()`] or [`wake_all()`] that
    ///     removed it from the list of futures ready to receive wakeups.
    ///  4. The `WaitQueue` has been [`close`d](WaitQueue::close), in which case
    ///     this method returns `Poll::Ready(Err(Closed))`.
    ///
    /// If this method returns [`Poll::Ready`], any subsequent `poll`s of this
    /// `Wait` future will also immediately return [`Poll::Ready`].
    ///
    /// If the [`Wait`] future subscribed to wakeups from the queue, and
    /// has not been woken, this method returns [`Poll::Pending`].
    ///
    /// [`wake()`]: WaitQueue::wake
    /// [`wake_all()`]: WaitQueue::wake_all
    pub fn subscribe(self: Pin<&mut Self>) -> Poll<Result<(), Closed>> {
        let this = self.project();
        this.waiter.poll_wait(this.queue, None)
    }
}

impl Future for Wait<'_> {
    type Output = Result<(), Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.waiter.poll_wait(this.queue, Some(cx.waker()))
    }
}

#[pinned_drop]
impl PinnedDrop for Wait<'_> {
    fn drop(mut self: Pin<&mut Self>) {
        let this = self.project();
        this.waiter.release(this.queue);
    }
}

// === impl WaitOwned ===

impl WaitOwned {
    /// Returns `true` if this `WaitOwned` future is waiting for a
    /// notification from the provided [`WaitQueue`].
    #[inline]
    #[must_use]
    pub fn waits_on(&self, queue: &WaitQueue) -> bool {
        ptr::eq(&*self.queue, queue)
    }

    /// Returns `true` if `self` and `other` are waiting on a notification
    /// from the same [`WaitQueue`].
    #[inline]
    #[must_use]
    pub fn same_queue(&self, other: &Wait<'_>) -> bool {
        ptr::eq(&*self.queue, other.queue)
    }

    /// Eagerly subscribe this future to wakeups from [`WaitQueue::wake()`].
    ///
    /// Polling a `WaitOwned` future adds that future to the list of waiters
    /// that may receive a wakeup from a `WaitQueue`. However, in some
    /// cases, it is desirable to subscribe to wakeups *prior* to actually
    /// waiting for one. This method should be used when it is necessary to
    /// ensure a `WaitOwned` future is in the list of waiters before the
    /// future is `poll`ed for the rst time.
    ///
    /// In general, this method is used in cases where a [`WaitQueue`] must
    /// synchronize with some additional state, such as an `AtomicBool` or
    /// counter. If a task first checks that state, and then chooses whether or
    /// not to wait on the `WaitQueue` based on that state, then a race
    /// condition may occur where the `WaitQueue` wakes waiters *between* when
    /// the task checked the external state and when it first polled its
    /// `WaitOwned` future to wait on the queue. This method allows
    /// registering the `WaitOwned`  future with the queue *prior* to
    /// checking the external state, without actually sleeping, so that when
    /// the task does wait for the `WaitOwned` future to complete, it will
    /// have received any wakeup that was sent between when the external
    /// state was checked and the `WaitOwned` future was first polled.
    ///
    /// # Returns
    ///
    /// This method returns a [`Poll`]`<`[`Result<(), Closed>`]`>` which is `Ready`
    /// a wakeup was already received. This method returns [`Poll::Ready`]
    /// in the following cases:
    ///
    ///  1. The [`WaitQueue::wake()`] method was called between the creation
    ///     of the `WaitOwned` future and the call to this method.
    ///  2. This is the first call to `subscribe` or `poll` on this future,
    ///     and the `WaitQueue` was holding a stored wakeup from a previous
    ///     call to [`wake()`]. This method consumes the wakeup in that case.
    ///  3. The future has previously been `subscribe`d or polled, and it
    ///     has since then been marked ready by either consuming a wakeup
    ///     from the `WaitQueue`, or by a call to [`wake()`] or
    ///     [`wake_all()`] that removed it from the list of futures ready to
    ///     receive wakeups.
    ///  4. The `WaitQueue` has been [`close`d](WaitQueue::close), in which
    ///     case this method returns `Poll::Ready(Err(Closed))`.
    ///
    /// If this method returns [`Poll::Ready`], any subsequent `poll`s of
    /// this `Wait` future will also immediately return [`Poll::Ready`].
    ///
    /// If the [`WaitOwned`] future subscribed to wakeups from the queue,
    /// and has not been woken, this method returns [`Poll::Pending`].
    ///
    /// [`wake()`]: WaitQueue::wake
    /// [`wake_all()`]: WaitQueue::wake_all
    pub fn subscribe(self: Pin<&mut Self>) -> Poll<Result<(), Closed>> {
        let this = self.project();
        this.waiter.poll_wait(this.queue, None)
    }
}

impl Future for WaitOwned {
    type Output = Result<(), Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.waiter.poll_wait(&*this.queue, Some(cx.waker()))
    }
}

#[pinned_drop]
impl PinnedDrop for WaitOwned {
    fn drop(mut self: Pin<&mut Self>) {
        let this = self.project();
        this.waiter.release(&*this.queue);
    }
}
