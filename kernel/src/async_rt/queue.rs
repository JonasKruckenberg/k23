// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Fixed-capacity, local queue for tasks.
//!
//! This is conceptually a simple intrusively linked list that can be pushed to and popped from using
//! the [`Local`] handle. To complicate things we also allow "consuming" tasks from the queue through
//! the [`Steal`] handle which other workers can use for work stealing purposes as the name implies.

use super::task::TaskRef;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use core::{iter, ptr};

const LOCAL_QUEUE_CAPACITY: usize = 256;
const MASK: usize = LOCAL_QUEUE_CAPACITY - 1;

/// Producer handle. May only be used from a single thread.
#[derive(Debug)]
pub(crate) struct Local {
    inner: Arc<Inner>,
}

/// Consumer handle. May be used from many threads.
#[derive(Debug)]
pub(crate) struct Steal(Arc<Inner>);

#[derive(Debug)]
pub(crate) struct Inner {
    /// Concurrently updated by many threads.
    ///
    /// Contains two `u32` values. The `LSB` byte is the "real" head of
    /// the queue. The `u32` in the `MSB` is set by a stealer in process
    /// of stealing values. It represents the first value being stolen in the
    /// batch. The `u32` indices are intentionally wider than strictly
    /// required for buffer indexing in order to provide ABA mitigation and make
    /// it possible to distinguish between full and empty buffers.
    ///
    /// When both `u32` values are the same, there is no active
    /// stealer.
    ///
    /// Tracking an in-progress stealer prevents a wrapping scenario.
    head: AtomicU64,

    /// Only updated by producer thread but read by many threads.
    tail: AtomicU32,

    /// Elements
    buffer: Box<[UnsafeCell<MaybeUninit<TaskRef>>; LOCAL_QUEUE_CAPACITY]>,
}

unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

// Constructing the fixed size array directly is very awkward. The only way to
// do it is to repeat `UnsafeCell::new(MaybeUninit::uninit())` 256 times, as
// the contents are not Copy. The trick with defining a const doesn't work for
// generic types.
fn make_fixed_size<T>(buffer: Box<[T]>) -> Box<[T; LOCAL_QUEUE_CAPACITY]> {
    assert_eq!(buffer.len(), LOCAL_QUEUE_CAPACITY);

    // safety: We check that the length is correct.
    unsafe { Box::from_raw(Box::into_raw(buffer).cast()) }
}

pub fn new() -> (Steal, Local) {
    let mut buffer = Vec::with_capacity(LOCAL_QUEUE_CAPACITY);

    for _ in 0..LOCAL_QUEUE_CAPACITY {
        buffer.push(UnsafeCell::new(MaybeUninit::uninit()));
    }

    let inner = Arc::new(Inner {
        head: AtomicU64::new(0),
        tail: AtomicU32::new(0),
        buffer: make_fixed_size(buffer.into_boxed_slice()),
    });

    let local = Local {
        inner: inner.clone(),
    };

    let remote = Steal(inner);

    (remote, local)
}

impl Local {
    /// Returns the number of entries in the queue
    pub(crate) fn len(&self) -> usize {
        self.inner.len() as usize
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many tasks can be pushed into the queue
    pub(crate) fn remaining_slots(&self) -> usize {
        self.inner.remaining_slots()
    }

    pub(crate) fn max_capacity(&self) -> usize {
        LOCAL_QUEUE_CAPACITY
    }

    /// Returns false if there are any entries in the queue
    pub(crate) fn has_tasks(&self) -> bool {
        !self.inner.is_empty()
    }

    pub(crate) fn can_steal(&self) -> bool {
        self.remaining_slots() >= self.max_capacity() - self.max_capacity() / 2
    }

    /// Pops a task from the local queue.
    pub(crate) fn pop(&mut self) -> Option<TaskRef> {
        let mut head = self.inner.head.load(Ordering::Acquire);

        let idx = loop {
            let (steal, real) = unpack(head);

            // safety: this is the **only** thread that updates this cell.
            let tail = unsafe { ptr::read(self.inner.tail.as_ptr()) };

            if real == tail {
                // queue is empty
                return None;
            }

            let next_real = real.wrapping_add(1);

            // If `steal == real` there are no concurrent stealers. Both `steal`
            // and `real` are updated.
            let next = if steal == real {
                pack(next_real, next_real)
            } else {
                assert_ne!(steal, next_real);
                pack(steal, next_real)
            };

            // Attempt to claim a task.
            let res =
                self.inner
                    .head
                    .compare_exchange(head, next, Ordering::AcqRel, Ordering::Acquire);

            match res {
                Ok(_) => break real as usize & MASK,
                Err(actual) => head = actual,
            }
        };

        Some(unsafe { ptr::read(self.inner.buffer[idx].get()).assume_init() })
    }

    /// Pushes a batch of tasks to the back of the queue. All tasks must fit in
    /// the local queue.
    ///
    /// # Panics
    ///
    /// The method panics if there is not enough capacity to fit in the queue.
    pub(crate) unsafe fn push_back_unchecked(&mut self, tasks: impl Iterator<Item = TaskRef>) {
        // safety: this is the **only** thread that updates this cell.
        let mut tail = unsafe { ptr::read(self.inner.tail.as_ptr()) };

        for task in tasks {
            let idx = tail as usize & MASK;

            // Write the task to the slot
            //
            // Safety: There is only one producer and the above `if`
            // condition ensures we don't touch a cell if there is a
            // value, thus no consumer.
            unsafe {
                ptr::write((*self.inner.buffer[idx].get()).as_mut_ptr(), task);
            }

            tail = tail.wrapping_add(1);
        }

        self.inner.tail.store(tail, Ordering::Release);
    }

    /// Pushes a task to the back of the local queue, if there is not enough
    /// capacity in the queue, this triggers the overflow operation.
    ///
    /// When the queue overflows, half of the current contents of the queue is
    /// moved to the given Injection queue. This frees up capacity for more
    /// tasks to be pushed into the local queue.
    pub(crate) fn push_back_or_overflow<O: Overflow>(
        &mut self,
        mut task: TaskRef,
        overflow: &O,
        // stats: &mut Stats,
    ) {
        let tail = loop {
            let head = self.inner.head.load(Ordering::Acquire);
            let (steal, real) = unpack(head);

            // safety: this is the **only** thread that updates this cell.
            let tail = unsafe { ptr::read(self.inner.tail.as_ptr()) };

            if tail.wrapping_sub(steal) < LOCAL_QUEUE_CAPACITY as u32 {
                // There is capacity for the task
                break tail;
            } else if steal != real {
                // Concurrently stealing, this will free up capacity, so only
                // push the task onto the inject queue
                overflow.push(task);
                return;
            } else {
                // Push the current task and half of the queue into the
                // inject queue.
                match self.push_overflow(task, real, tail, overflow) {
                    Ok(_) => return,
                    // Lost the race, try again
                    Err(v) => {
                        task = v;
                    }
                }
            }
        };

        self.push_back_finish(task, tail);
    }

    // Second half of `push_back`
    fn push_back_finish(&self, task: TaskRef, tail: u32) {
        // Map the position to a slot index.
        let idx = tail as usize & MASK;

        // Write the task to the slot
        //
        // Safety: There is only one producer and the above `if`
        // condition ensures we don't touch a cell if there is a
        // value, thus no consumer.
        unsafe {
            ptr::write((*self.inner.buffer[idx].get()).as_mut_ptr(), task);
        }

        // Make the task available. Synchronizes with a load in
        // `steal_into2`.
        self.inner
            .tail
            .store(tail.wrapping_add(1), Ordering::Release);
    }

    /// Moves a batch of tasks into the global queue.
    ///
    /// This will temporarily make some of the tasks unavailable to stealers.
    /// Once `push_overflow` is done, a notification is sent out, so if other
    /// workers "missed" some of the tasks during a steal, they will get
    /// another opportunity.
    #[inline(never)]
    fn push_overflow<O: Overflow>(
        &mut self,
        task: TaskRef,
        head: u32,
        tail: u32,
        overflow: &O,
        // stats: &mut Stats,
    ) -> Result<(), TaskRef> {
        /// How many elements are we taking from the local queue.
        ///
        /// This is one less than the number of tasks pushed to the inject
        /// queue as we are also inserting the `task` argument.
        const NUM_TASKS_TAKEN: u32 = (LOCAL_QUEUE_CAPACITY / 2) as u32;

        assert_eq!(
            tail.wrapping_sub(head) as usize,
            LOCAL_QUEUE_CAPACITY,
            "queue is not full; tail = {tail}; head = {head}"
        );

        let prev = pack(head, head);

        // Claim a bunch of tasks
        //
        // We are claiming the tasks **before** reading them out of the buffer.
        // This is safe because only the **current** thread is able to push new
        // tasks.
        //
        // There isn't really any need for memory ordering... Relaxed would
        // work. This is because all tasks are pushed into the queue from the
        // current thread (or memory has been acquired if the local queue handle
        // moved).
        if self
            .inner
            .head
            .compare_exchange(
                prev,
                pack(
                    head.wrapping_add(NUM_TASKS_TAKEN),
                    head.wrapping_add(NUM_TASKS_TAKEN),
                ),
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_err()
        {
            // We failed to claim the tasks, losing the race. Return out of
            // this function and try the full `push` routine again. The queue
            // may not be full anymore.
            return Err(task);
        }

        /// An iterator that takes elements out of the run queue.
        struct BatchTaskIter<'a> {
            buffer: &'a [UnsafeCell<MaybeUninit<TaskRef>>; LOCAL_QUEUE_CAPACITY],
            head: u64,
            i: u64,
        }
        impl<'a> Iterator for BatchTaskIter<'a> {
            type Item = TaskRef;

            #[inline]
            fn next(&mut self) -> Option<TaskRef> {
                if self.i == u64::from(NUM_TASKS_TAKEN) {
                    None
                } else {
                    let i_idx = self.i.wrapping_add(self.head) as usize & MASK;
                    let slot = &self.buffer[i_idx];

                    // safety: Our CAS from before has assumed exclusive ownership
                    // of the task pointers in this range.
                    let task = unsafe { ptr::read((*slot.get()).as_ptr()) };

                    self.i += 1;
                    Some(task)
                }
            }
        }

        // safety: The CAS above ensures that no consumer will look at these
        // values again, and we are the only producer.
        let batch_iter = BatchTaskIter {
            buffer: &self.inner.buffer,
            head: head as u64,
            i: 0,
        };
        overflow.push_batch(batch_iter.chain(iter::once(task)));

        // // Add 1 to factor in the task currently being scheduled.
        // stats.incr_overflow_count();

        Ok(())
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        if !crate::panic::panicking() {
            assert!(self.pop().is_none(), "queue not empty");
        }
    }
}

impl Steal {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Steals half the tasks from self and place them into `dst`.
    pub(crate) fn steal_into(
        &self,
        dst: &mut Local,
        // dst_stats: &mut Stats,
    ) -> Option<TaskRef> {
        // Safety: the caller is the only thread that mutates `dst.tail` and
        // holds a mutable reference.
        let dst_tail = unsafe { ptr::read(dst.inner.tail.as_ptr()) };

        // To the caller, `dst` may **look** empty but still have values
        // contained in the buffer. If another thread is concurrently stealing
        // from `dst` there may not be enough capacity to steal.
        let (steal, _) = unpack(dst.inner.head.load(Ordering::Acquire));

        if dst_tail.wrapping_sub(steal) > LOCAL_QUEUE_CAPACITY as u32 / 2 {
            // we *could* try to steal less here, but for simplicity, we're just
            // going to abort.
            return None;
        }

        // Steal the tasks into `dst`'s buffer. This does not yet expose the
        // tasks in `dst`.
        let mut n = self.steal_into2(dst, dst_tail);

        if n == 0 {
            // No tasks were stolen
            return None;
        }

        // dst_stats.incr_steal_count(n as u16);
        // dst_stats.incr_steal_operations();

        // We are returning a task here
        n -= 1;

        let ret_pos = dst_tail.wrapping_add(n);
        let ret_idx = ret_pos as usize & MASK;

        // safety: the value was written as part of `steal_into2` and not
        // exposed to stealers, so no other thread can access it.
        let ret = unsafe { ptr::read((*dst.inner.buffer[ret_idx].get()).as_ptr()) };

        if n == 0 {
            // The `dst` queue is empty, but a single task was stolen
            return Some(ret);
        }

        // Make the stolen items available to consumers
        dst.inner
            .tail
            .store(dst_tail.wrapping_add(n), Ordering::Release);

        Some(ret)
    }

    // Steal tasks from `self`, placing them into `dst`. Returns the number of
    // tasks that were stolen.
    fn steal_into2(&self, dst: &mut Local, dst_tail: u32) -> u32 {
        let mut prev_packed = self.0.head.load(Ordering::Acquire);
        let mut next_packed;

        let n = loop {
            let (src_head_steal, src_head_real) = unpack(prev_packed);
            let src_tail = self.0.tail.load(Ordering::Acquire);

            // If these two do not match, another thread is concurrently
            // stealing from the queue.
            if src_head_steal != src_head_real {
                return 0;
            }

            // Number of available tasks to steal
            let n = src_tail.wrapping_sub(src_head_real);
            let n = n - n / 2;

            if n == 0 {
                // No tasks available to steal
                return 0;
            }

            // Update the real head index to acquire the tasks.
            let steal_to = src_head_real.wrapping_add(n);
            assert_ne!(src_head_steal, steal_to);
            next_packed = pack(src_head_steal, steal_to);

            // Claim all those tasks. This is done by incrementing the "real"
            // head but not the steal. By doing this, no other thread is able to
            // steal from this queue until the current thread completes.
            let res = self.0.head.compare_exchange(
                prev_packed,
                next_packed,
                Ordering::AcqRel,
                Ordering::Acquire,
            );

            match res {
                Ok(_) => break n,
                Err(actual) => prev_packed = actual,
            }
        };

        assert!(n <= LOCAL_QUEUE_CAPACITY as u32 / 2, "actual = {n}");

        let (first, _) = unpack(next_packed);

        // Take all the tasks
        for i in 0..n {
            // Compute the positions
            let src_pos = first.wrapping_add(i);
            let dst_pos = dst_tail.wrapping_add(i);

            // Map to slots
            let src_idx = src_pos as usize & MASK;
            let dst_idx = dst_pos as usize & MASK;

            // Read the task
            //
            // safety: We acquired the task with the atomic exchange above.
            let task = unsafe { ptr::read((*self.0.buffer[src_idx].get()).as_ptr()) };

            // Write the task to the new slot
            //
            // safety: `dst` queue is empty, and we are the only producer to
            // this queue.
            unsafe { ptr::write((*dst.inner.buffer[dst_idx].get()).as_mut_ptr(), task) };
        }

        let mut prev_packed = next_packed;

        // Update `src_head_steal` to match `src_head_real` signalling that the
        // stealing routine is complete.
        loop {
            let head = unpack(prev_packed).1;
            next_packed = pack(head, head);

            let res = self.0.head.compare_exchange(
                prev_packed,
                next_packed,
                Ordering::AcqRel,
                Ordering::Acquire,
            );

            match res {
                Ok(_) => return n,
                Err(actual) => {
                    let (actual_steal, actual_real) = unpack(actual);

                    assert_ne!(actual_steal, actual_real);

                    prev_packed = actual;
                }
            }
        }
    }
}

impl Clone for Steal {
    fn clone(&self) -> Steal {
        Steal(self.0.clone())
    }
}

impl Inner {
    fn remaining_slots(&self) -> usize {
        let (steal, _) = unpack(self.head.load(Ordering::Acquire));
        let tail = self.tail.load(Ordering::Acquire);

        LOCAL_QUEUE_CAPACITY - (tail.wrapping_sub(steal) as usize)
    }

    fn len(&self) -> u32 {
        let (_, head) = unpack(self.head.load(Ordering::Acquire));
        let tail = self.tail.load(Ordering::Acquire);

        tail.wrapping_sub(head)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub(crate) trait Overflow {
    fn push(&self, task: TaskRef);

    fn push_batch<I>(&self, iter: I)
    where
        I: Iterator<Item = TaskRef>;
}

/// Split the head value into the real head and the index a stealer is working
/// on.
fn unpack(n: u64) -> (u32, u32) {
    let real = n & u32::MAX as u64;
    let steal = n >> (size_of::<u32>() * 8);

    (steal as u32, real as u32)
}

/// Join the two head values
fn pack(steal: u32, real: u32) -> u64 {
    (real as u64) | ((steal as u64) << (size_of::<u32>() * 8))
}
