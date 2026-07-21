// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::ptr;
use core::ptr::NonNull;

use cordyceps::{list, Linked};
use util::loom_const_fn;

use super::QsbrDomain;
use crate::loom::sync::atomic::{fence, AtomicU64, Ordering};

pub(crate) const IDLE_STATE: u64 = 0;

pub struct QsbrReader {
    pub(crate) state: AtomicU64,
    links: list::Links<Self>,
    _not_send_or_sync: PhantomData<*const ()>,
}

impl QsbrReader {
    loom_const_fn! {
        pub const fn new() -> Self {
            Self {
                state: AtomicU64::new(IDLE_STATE),
                links: list::Links::new(),
                _not_send_or_sync: PhantomData,
            }
        }
    }

    #[inline(always)]
    pub fn read<R>(&self, f: impl FnOnce(&Guard) -> R) -> R {
        debug_assert_ne!(
            self.state.load(Ordering::Relaxed),
            IDLE_STATE,
            "qsbr: read() on an idle or unregistered QsbrCpu"
        );

        f(&Guard {
            _marker: PhantomData,
        })
    }

    pub unsafe fn register(&'static self, domain: &'static QsbrDomain) {
        assert!(!self.links.is_linked(), "qsbr: QsbrCpu registered twice");
        domain.cpus.lock().push_back(NonNull::from(self));

        unsafe {
            self.exit_idle(domain);
        }
    }

    pub unsafe fn quiescent(&self, domain: &QsbrDomain) {
        let state = self.state.load(Ordering::Relaxed);
        debug_assert_ne!(state, IDLE_STATE, "qsbr: quiescent() while idle");

        // Acquire: pairs with the Release epoch increments in
        // `advance`/`retire`; entering epoch E makes every unlink that
        // ended an earlier epoch visible to our subsequent critical
        // sections.
        let epoch = domain.global_epoch.load(Ordering::Acquire);
        if state != epoch {
            // Release: pairs with the Acquire load in `min_active_epoch`;
            // orders this CPU's preceding reads (its last uses of retired
            // objects) before the reclaimer's frees.
            self.state.store(epoch, Ordering::Release);
        }
    }

    #[inline]
    pub unsafe fn enter_idle(&self) {
        self.state.store(IDLE_STATE, Ordering::Release);
    }

    #[inline]
    pub unsafe fn exit_idle(&self, domain: &QsbrDomain) {
        let epoch = domain.global_epoch.load(Ordering::Relaxed);
        self.state.store(epoch, Ordering::Relaxed);

        // Store→load barrier, paired with the SeqCst fence in
        // `min_active_epoch`: either the reclaimer sees our epoch store, or
        // we see every unlink it made before scanning — so it can never
        // free something our upcoming critical sections can still reach.
        fence(Ordering::SeqCst);
    }
}

unsafe impl Linked<list::Links<QsbrReader>> for QsbrReader {
    type Handle = NonNull<QsbrReader>;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(target: NonNull<Self>) -> NonNull<list::Links<QsbrReader>> {
        // SAFETY: raw field projection; no intermediate reference formed.
        unsafe { NonNull::new_unchecked(ptr::addr_of_mut!((*target.as_ptr()).links)) }
    }
}

pub struct Guard {
    /// `!Send + !Sync`: the guard proves things about the *current* CPU's
    /// quiescent-state reporting and must not cross to another context.
    _marker: PhantomData<*const ()>,
}
