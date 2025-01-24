// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::{raw, Id, JoinHandle, Schedule, TaskRef};
use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};
use sync::Mutex;

#[derive(Debug)]
pub struct OwnedTasks {
    list: Mutex<linked_list::List<raw::Header>>,
    closed: AtomicBool,
}

impl OwnedTasks {
    pub(in crate::async_rt) fn new() -> Self {
        OwnedTasks {
            list: Mutex::new(linked_list::List::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub(in crate::async_rt) fn is_empty(&self) -> bool {
        self.list.lock().is_empty()
    }

    pub(in crate::async_rt) fn bind<F, S>(
        &self,
        task: F,
        scheduler: S,
        id: Id,
    ) -> (JoinHandle<F::Output>, Option<TaskRef>)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
        S: Schedule + 'static,
    {
        let (for_owner, for_scheduler, join) = super::new_task(task, scheduler, id);
        let task = unsafe { self.bind_inner(for_owner, for_scheduler) };
        (join, task)
    }

    pub(in crate::async_rt) unsafe fn bind_local<F, S>(
        &self,
        task: F,
        scheduler: S,
        id: Id,
    ) -> (JoinHandle<F::Output>, Option<TaskRef>)
    where
        F: Future + 'static,
        F::Output: 'static,
        S: Schedule + 'static,
    {
        let (for_owner, for_scheduler, join) = super::new_task(task, scheduler, id);
        let task = unsafe { self.bind_inner(for_owner, for_scheduler) };
        (join, task)
    }

    // The part of `bind` that's the same for every type of future.
    unsafe fn bind_inner(&self, task: TaskRef, for_scheduler: TaskRef) -> Option<TaskRef> {
        let mut list = self.list.lock();
        // Check the closed flag in the lock for ensuring all that tasks
        // will shut down after the OwnedTasks has been closed.
        if self.closed.load(Ordering::Acquire) {
            drop(list);
            task.shutdown();
            return None;
        }
        list.push_back(task);
        Some(for_scheduler)
    }

    pub(in crate::async_rt) fn close_and_shutdown_all(&self) {
        self.closed.store(true, Ordering::Release);
        let mut list = self.list.lock();

        let mut c = list.cursor_front_mut();
        while let Some(task) = c.remove() {
            task.shutdown();
        }
    }

    pub(in crate::async_rt) fn remove(&self, task: &TaskRef) -> Option<TaskRef> {
        let mut list = self.list.lock();
        // Check the closed flag in the lock for ensuring all that tasks
        // will shut down after the OwnedTasks has been closed.
        if self.closed.load(Ordering::Acquire) {
            drop(list);
            task.shutdown();
            return None;
        }

        unsafe { list.cursor_from_ptr_mut(task.header_ptr()).remove() }
    }
}
