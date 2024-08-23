use core::{
    mem,
    sync::atomic::{AtomicU8, Ordering},
};

/// No initialization has run yet, and no thread is currently using the Once.
const STATUS_INCOMPLETE: u8 = 0;
/// Some thread has previously attempted to initialize the Once, but it panicked,
/// so the Once is now poisoned. There are no other threads currently accessing
/// this Once.
const STATUS_POISONED: u8 = 1;
/// Some thread is currently attempting to run initialization. It may succeed,
/// so all future threads need to wait for it to finish.
const STATUS_RUNNING: u8 = 2;
/// Initialization has completed and all future calls should finish immediately.
const STATUS_COMPLETE: u8 = 4;

pub enum ExclusiveState {
    Incomplete,
    Poisoned,
    Complete,
}

/// A synchronization primitive for running one-time global initialization.
pub struct Once {
    status: AtomicU8,
}

impl Once {
    #[inline]
    #[must_use]
    pub const fn new() -> Once {
        Once {
            status: AtomicU8::new(STATUS_INCOMPLETE),
        }
    }

    #[inline]
    pub fn is_completed(&self) -> bool {
        self.status.load(Ordering::Acquire) == STATUS_COMPLETE
    }

    pub fn state(&mut self) -> ExclusiveState {
        match *self.status.get_mut() {
            STATUS_INCOMPLETE => ExclusiveState::Incomplete,
            STATUS_POISONED => ExclusiveState::Poisoned,
            STATUS_COMPLETE => ExclusiveState::Complete,
            _ => unreachable!("invalid Once state"),
        }
    }

    /// # Panics
    ///
    /// Panics if the closure panics.
    #[inline]
    #[track_caller]
    pub fn call_once<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        // Fast path check
        if self.is_completed() {
            return;
        }

        let mut f = Some(f);
        self.call(&mut || f.take().unwrap()());
    }

    #[cold]
    #[track_caller]
    fn call(&self, f: &mut impl FnMut()) {
        loop {
            let xchg = self.status.compare_exchange(
                STATUS_INCOMPLETE,
                STATUS_RUNNING,
                Ordering::Acquire,
                Ordering::Acquire,
            );

            match xchg {
                Ok(_) => {
                    let panic_guard = PanicGuard {
                        status: &self.status,
                    };

                    f();

                    mem::forget(panic_guard);

                    self.status.store(STATUS_COMPLETE, Ordering::Release);

                    return;
                }
                Err(STATUS_COMPLETE) => return,
                Err(STATUS_RUNNING) => self.wait(),
                Err(STATUS_POISONED) => {
                    // Panic to propagate the poison.
                    panic!("Once instance has previously been poisoned");
                }
                _ => unreachable!("state is never set to invalid values"),
            }
        }
    }

    fn poll(&self) -> bool {
        match self.status.load(Ordering::Acquire) {
            STATUS_INCOMPLETE | STATUS_RUNNING => false,
            STATUS_COMPLETE => true,
            STATUS_POISONED => panic!("Once poisoned by panic"),
            _ => unreachable!(),
        }
    }

    fn wait(&self) {
        loop {
            if !self.poll() {
                core::hint::spin_loop();
            }
        }
    }
}

impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

struct PanicGuard<'a> {
    status: &'a AtomicU8,
}

impl<'a> Drop for PanicGuard<'a> {
    fn drop(&mut self) {
        self.status.store(STATUS_POISONED, Ordering::Relaxed);
    }
}
