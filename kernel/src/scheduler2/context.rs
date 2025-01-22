// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::scheduler2::scheduler;
use crate::scheduler2::scheduler::Handle;
use core::cell::Cell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};
use thread_local::thread_local;

thread_local! {
    static CONTEXT: Context = const {
        Context {
            running: AtomicBool::new(false),
            scheduler: Scoped::new()
        }
    };
}

struct Context {
    running: AtomicBool,
    scheduler: Scoped<scheduler::Context>,
}

#[track_caller]
#[allow(tail_expr_drop_order)]
pub(super) fn with_scheduler<R>(f: impl FnOnce(Option<&scheduler::Context>) -> R) -> R {
    let mut f = Some(f);
    CONTEXT
        .try_with(|c| {
            let f = f.take().unwrap();
            if c.running.load(Ordering::SeqCst) {
                c.scheduler.with(f)
            } else {
                f(None)
            }
        })
        .unwrap_or_else(|_| (f.take().unwrap())(None))
}

/// Scoped thread-local storage
pub(super) struct Scoped<T> {
    pub(super) inner: Cell<*const T>,
}

impl<T> Scoped<T> {
    pub(super) const fn new() -> Scoped<T> {
        Scoped {
            inner: Cell::new(ptr::null()),
        }
    }

    /// Inserts a value into the scoped cell for the duration of the closure
    pub(super) fn set<F, R>(&self, t: &T, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        struct Reset<'a, T> {
            cell: &'a Cell<*const T>,
            prev: *const T,
        }

        impl<T> Drop for Reset<'_, T> {
            fn drop(&mut self) {
                self.cell.set(self.prev);
            }
        }

        let prev = self.inner.get();
        self.inner.set(t as *const _);

        let _reset = Reset {
            cell: &self.inner,
            prev,
        };

        f()
    }

    /// Gets the value out of the scoped cell;
    pub(super) fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Option<&T>) -> R,
    {
        let val = self.inner.get();

        if val.is_null() {
            f(None)
        } else {
            unsafe { f(Some(&*val)) }
        }
    }
}

pub(crate) fn enter<F, R>(_handle: &Handle, f: F) -> R
where
    F: FnOnce() -> R,
{
    CONTEXT.with(|c| c.running.store(true, Ordering::SeqCst));
    // TODO unset on drop

    f()
}

pub(super) fn set_scheduler<R>(v: &scheduler::Context, f: impl FnOnce() -> R) -> R {
    #[allow(tail_expr_drop_order)]
    CONTEXT.with(|c| c.scheduler.set(v, f))
}
