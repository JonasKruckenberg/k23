// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::{Schedule, TaskRef};
use crate::task;
use crate::task::id::Id;
use crate::task::join_handle::JoinHandle;
use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};
use sync::Mutex;

#[derive(Debug)]
pub struct OwnedTasks {
    list: Mutex<linked_list::List<task::Header>>,
    closed: AtomicBool,
}

impl OwnedTasks {
    pub(crate) const fn new() -> Self {
        OwnedTasks {
            list: Mutex::new(linked_list::List::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.lock().is_empty()
    }

    pub(crate) fn bind<F, S>(
        &self,
        future: F,
        scheduler: S,
        id: Id,
    ) -> (JoinHandle<F::Output>, Option<TaskRef>)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
        S: Schedule + 'static,
    {
        let task = TaskRef::try_new_in(future, scheduler, id, alloc::alloc::Global).unwrap();
        let join = JoinHandle::new(task.clone());

        let task = self.bind_inner(task);
        (join, task)
    }

    pub(crate) fn bind_local<F, S>(
        &self,
        future: F,
        scheduler: S,
        id: Id,
    ) -> (JoinHandle<F::Output>, Option<TaskRef>)
    where
        F: Future + 'static,
        F::Output: 'static,
        S: Schedule + 'static,
    {
        let task = TaskRef::try_new_in(future, scheduler, id, alloc::alloc::Global).unwrap();
        let join = JoinHandle::new(task.clone());

        let task = self.bind_inner(task);
        (join, task)
    }

    // The part of `bind` that's the same for every type of future.
    fn bind_inner(&self, task: TaskRef) -> Option<TaskRef> {
        let mut list = self.list.lock();
        // Check the closed flag in the lock for ensuring all that tasks
        // will shut down after the OwnedTasks has been closed.
        if self.closed.load(Ordering::Acquire) {
            drop(list);
            return None;
        }
        list.push_back(task.clone());
        Some(task)
    }

    pub(crate) fn close_and_shutdown_all(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            log::trace!("closing OwnedTasks");
            let mut list = self.list.lock();

            let mut c = list.cursor_front_mut();
            while let Some(task) = c.remove() {
                drop(task);
            }

            debug_assert!(list.is_empty(), "{list:?}");
        }
    }

    // pub(crate) fn remove(&self, task: &TaskRef) -> Option<TaskRef> {
    //     let mut list = self.list.lock();
    //     // Check the closed flag in the lock for ensuring all that tasks
    //     // will shut down after the OwnedTasks has been closed.
    //     if self.closed.load(Ordering::Acquire) {
    //         drop(list);
    //         task.shutdown();
    //         return None;
    //     }
    //
    //     log::trace!("removing task from owned tasks");
    //
    //     // Safety: `OwnedTasks::bind`/`OwnedTasks::bind_local` are called during task creation
    //     // so every task is necessarily in our list until this point
    //     unsafe { list.cursor_from_ptr_mut(task.header_ptr()).remove() }
    // }
}
