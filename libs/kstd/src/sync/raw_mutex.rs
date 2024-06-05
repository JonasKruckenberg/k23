use core::sync::atomic::{AtomicBool, Ordering};

pub struct RawMutex {
    lock: AtomicBool,
}

impl RawMutex {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    pub fn lock(&self) {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.is_locked() {
                core::hint::spin_loop();
            }
        }
    }

    pub fn try_lock(&self) -> bool {
        self.lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    pub fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}
