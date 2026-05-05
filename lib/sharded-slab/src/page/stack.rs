use core::fmt;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::cfg;

pub(super) struct TransferStack<C = cfg::DefaultConfig> {
    head: AtomicUsize,
    _cfg: PhantomData<fn(C)>,
}

impl<C: cfg::Config> TransferStack<C> {
    pub(super) fn new() -> Self {
        Self {
            head: AtomicUsize::new(super::Addr::<C>::NULL),
            _cfg: PhantomData,
        }
    }

    pub(super) fn pop_all(&self) -> Option<usize> {
        let val = self.head.swap(super::Addr::<C>::NULL, Ordering::Acquire);
        log::trace!("-> pop {:#x}", val);
        if val == super::Addr::<C>::NULL {
            None
        } else {
            Some(val)
        }
    }

    fn push(&self, new_head: usize, before: impl Fn(usize)) {
        // We loop to win the race to set the new head. The `next` variable
        // is the next slot on the stack which needs to be pointed to by the
        // new head.
        let mut next = self.head.load(Ordering::Relaxed);
        loop {
            log::trace!("-> next {:#x}", next);
            before(next);

            match self
                .head
                .compare_exchange(next, new_head, Ordering::Release, Ordering::Relaxed)
            {
                // lost the race!
                Err(actual) => {
                    log::trace!("-> retry!");
                    next = actual;
                }
                Ok(_) => {
                    log::trace!("-> successful; next={:#x}", next);
                    return;
                }
            }
        }
    }
}

impl<C: cfg::Config> super::FreeList<C> for TransferStack<C> {
    fn push<T>(&self, new_head: usize, slot: &super::Slot<T, C>) {
        self.push(new_head, |next| slot.set_next(next));
    }
}

impl<C> fmt::Debug for TransferStack<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferStack")
            .field(
                "head",
                &format_args!("{:#0x}", &self.head.load(Ordering::Relaxed)),
            )
            .finish()
    }
}
