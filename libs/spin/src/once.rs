// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem;

use util::loom_const_fn;

use crate::loom::{AtomicU8, Ordering};

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
    loom_const_fn! {
        #[inline]
        #[must_use]
        pub const fn new() -> Once {
            Once {
                status: AtomicU8::new(STATUS_INCOMPLETE),
            }
        }
    }

    #[inline]
    pub fn is_completed(&self) -> bool {
        self.status.load(Ordering::Acquire) == STATUS_COMPLETE
    }

    pub fn state(&mut self) -> ExclusiveState {
        self.status.with_mut(|status| match *status {
            STATUS_INCOMPLETE => ExclusiveState::Incomplete,
            STATUS_POISONED => ExclusiveState::Poisoned,
            STATUS_COMPLETE => ExclusiveState::Complete,
            _ => unreachable!("invalid Once state"),
        })
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
        #[allow(tail_expr_drop_order, reason = "")]
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

    pub fn wait(&self) {
        while !self.poll() {
            #[cfg(loom)]
            crate::loom::thread::yield_now();
            core::hint::spin_loop();
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

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        self.status.store(STATUS_POISONED, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;
    use crate::loom::thread;

    #[test]
    fn smoke_once() {
        static O: std::sync::LazyLock<Once> = std::sync::LazyLock::new(|| Once::new());
        let mut a = 0;
        O.call_once(|| a += 1);
        assert_eq!(a, 1);
        O.call_once(|| a += 1);
        assert_eq!(a, 1);
    }

    #[test]
    fn stampede_once() {
        crate::loom::model(|| {
            static O: std::sync::LazyLock<Once> = std::sync::LazyLock::new(|| Once::new());
            static mut RUN: bool = false;

            const MAX_THREADS: usize = 4;

            let (tx, rx) = channel();
            for _ in 0..MAX_THREADS {
                let tx = tx.clone();
                thread::spawn(move || {
                    // for _ in 0..2 {
                    //     thread::yield_now()
                    // }
                    unsafe {
                        O.call_once(|| {
                            assert!(!RUN);
                            RUN = true;
                        });
                        assert!(RUN);
                    }
                    tx.send(()).unwrap();
                });
            }

            unsafe {
                O.call_once(|| {
                    assert!(!RUN);
                    RUN = true;
                });
                assert!(RUN);
            }

            for _ in 0..MAX_THREADS {
                rx.recv().unwrap();
            }
        })
    }

    #[cfg(not(loom))]
    #[test]
    fn wait() {
        use crate::loom::{AtomicBool, Ordering};

        for _ in 0..50 {
            let val = AtomicBool::new(false);
            let once = Once::new();

            thread::scope(|s| {
                for _ in 0..4 {
                    s.spawn(|| {
                        once.wait();
                        assert!(val.load(Ordering::Relaxed));
                    });
                }

                once.call_once(|| val.store(true, Ordering::Relaxed));
            });
        }
    }
}
