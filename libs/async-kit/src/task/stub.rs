// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::state::State;
use crate::task::{Header, Id, JoinError, PollResult, VTable};
use core::ptr::NonNull;
use core::task::{Context, Poll};
use util::loom_const_fn;

/// A stub task required by many `const` constructors in this crate. You should rarely need to use
/// this directly, instead look for the safe construction macros provided.
#[derive(Debug)]
pub struct TaskStub {
    pub(crate) header: Header,
}

impl Default for TaskStub {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStub {
    const STATIC_STUB_VTABLE: VTable = VTable {
        poll: stub_poll,
        poll_join: stub_poll_join,
        deallocate: stub_deallocate,
        wake_by_ref: stub_wake_by_ref,
    };

    loom_const_fn! {
        pub const fn new() -> Self {
            Self {
                header: Header {
                    state: State::new(),
                    vtable: &Self::STATIC_STUB_VTABLE,
                    id: Id::stub(),
                    run_queue_links: mpsc_queue::Links::new_stub(),
                    span: tracing::Span::none(),
                    #[cfg(debug_assertions)]
                    scheduler_type: None
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe fn stub_poll(ptr: NonNull<Header>) -> PollResult {
    // Safety: this method should never be called
    unsafe {
        debug_assert!(ptr.as_ref().id.is_stub());
        unreachable!("stub task ({ptr:?}) should never be polled!");
    }
}

#[unsafe(no_mangle)]
pub unsafe fn stub_poll_join(
    ptr: NonNull<Header>,
    _outptr: NonNull<()>,
    _cx: &mut Context<'_>,
) -> Poll<Result<(), JoinError<()>>> {
    // Safety: this method should never be called
    unsafe {
        debug_assert!(ptr.as_ref().id.is_stub());
        unreachable!("stub task ({ptr:?}) should never be polled!");
    }
}

#[unsafe(no_mangle)]
unsafe fn stub_deallocate(ptr: NonNull<Header>) {
    // Safety: this method should never be called
    unsafe {
        debug_assert!(ptr.as_ref().id.is_stub());
        unreachable!("stub task ({ptr:p}) should never be deallocated!");
    }
}

#[unsafe(no_mangle)]
pub unsafe fn stub_wake_by_ref(ptr: *const ()) {
    unreachable!("stub task ({ptr:p}) has no waker and should never be woken!");
}
