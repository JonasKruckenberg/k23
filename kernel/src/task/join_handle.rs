// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::raw::Header;
use crate::task::TaskRef;
use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::pin::Pin;
use core::task::{Context, Poll};

pub struct JoinHandle<T> {
    raw: TaskRef,
    _p: PhantomData<T>,
}
static_assertions::assert_impl_all!(JoinHandle<()>: Send, Sync);

impl<T> UnwindSafe for JoinHandle<T> {}

impl<T> RefUnwindSafe for JoinHandle<T> {}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if self.raw.state().drop_join_handle_fast().is_ok() {
            return;
        }

        self.raw.drop_join_handle_slow();
    }
}

impl<T> fmt::Debug for JoinHandle<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Safety: The header pointer is valid.
        unsafe {
            let id_ptr = Header::get_id_ptr(self.raw.header_ptr());
            let id = id_ptr.as_ref();
            fmt.debug_struct("JoinHandle").field("id", id).finish()
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = super::Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // ready!(crate::trace::trace_leaf(cx));
        let mut ret = Poll::Pending;

        // // Keep track of task budget
        // let coop = ready!(crate::runtime::coop::poll_proceed(cx));

        // Try to read the task output. If the task is not yet complete, the
        // waker is stored and is notified once the task does complete.
        //
        // The function must go via the vtable, which requires erasing generic
        // types. To do this, the function "return" is placed on the stack
        // **before** calling the function and is passed into the function using
        // `*mut ()`.
        //
        // Safety:
        //
        // The type of `T` must match the task's output type.
        unsafe {
            self.raw
                .try_read_output(core::ptr::from_mut(&mut ret).cast::<()>(), cx.waker());
        }

        // if ret.is_ready() {
        //     coop.made_progress();
        // }

        ret
    }
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(raw: TaskRef) -> Self {
        Self {
            raw,
            _p: PhantomData,
        }
    }

    pub fn abort(&self) {
        self.raw.remote_abort();
    }

    pub fn is_finished(&self) -> bool {
        let state = self.raw.header().state.load();
        state.is_complete()
    }
}
