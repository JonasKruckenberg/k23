// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::{new_task, raw, Id, JoinHandle, LocalTaskRef, TaskRef};
use core::future::Future;
use core::marker::PhantomData;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use sync::Mutex;

#[derive(Debug)]
pub struct OwnedTasks {
    list: Mutex<linked_list::List<raw::Header>>,
    id: NonZeroU64,
    closed: AtomicBool,
}

impl OwnedTasks {
    pub(in crate::scheduler2) fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let id = NonZeroU64::new(unsafe { NEXT_ID.fetch_add(1, Ordering::Relaxed) }).unwrap();
        OwnedTasks {
            list: Mutex::new(linked_list::List::new()),
            id,
            closed: AtomicBool::new(false),
        }
    }

    pub(in crate::scheduler2) fn is_empty(&self) -> bool {
        self.list.lock().is_empty()
    }

    pub(in crate::scheduler2) fn bind<F>(
        &self,
        task: F,
        id: Id,
    ) -> (JoinHandle<F::Output>, Option<TaskRef>)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (task, join) = new_task(task, id);
        let task = unsafe { self.bind_inner(task) };
        (join, task)
    }

    pub(in crate::scheduler2) unsafe fn bind_local<F>(
        &self,
        task: F,
        id: Id,
    ) -> (JoinHandle<F::Output>, Option<TaskRef>)
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let (task, join) = super::new_task(task, id);
        let task = unsafe { self.bind_inner(task) };
        (join, task)
    }

    // The part of `bind` that's the same for every type of future.
    unsafe fn bind_inner(&self, task: TaskRef) -> Option<TaskRef> {
        unsafe {
            // safety: We just created the task, so we have exclusive access
            // to the field.
            task.header().set_owner_id(self.id);
        }

        let mut list = self.list.lock();
        // Check the closed flag in the lock for ensuring all that tasks
        // will shut down after the OwnedTasks has been closed.
        if self.closed.load(Ordering::Acquire) {
            drop(list);
            task.shutdown();
            return None;
        }
        list.push_back(task.clone());
        Some(task)
    }

    pub(in crate::scheduler2) fn close_and_shutdown_all(&self) {
        self.closed.store(true, Ordering::Release);
        let mut list = self.list.lock();

        let mut c = list.cursor_front_mut();
        while let Some(task) = c.remove() {
            task.shutdown();
        }
    }

    #[inline]
    pub(in crate::scheduler2) fn assert_owner(&self, task: TaskRef) -> LocalTaskRef {
        debug_assert_eq!(task.header().get_owner_id(), Some(self.id));
        // safety: All tasks bound to this OwnedTasks are Send, so it is safe
        // to poll it on this thread no matter what thread we are on.
        LocalTaskRef {
            task,
            _not_send: PhantomData,
        }
    }
}
