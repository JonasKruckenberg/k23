use super::raw_mutex::RawMutex;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::ops::Deref;
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::ptr::addr_of;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct ReentrantMutex<T> {
    raw: RawMutex,
    owner: AtomicUsize,
    lock_count: UnsafeCell<u32>,
    data: T,
}

unsafe impl<T: Send> Send for ReentrantMutex<T> {}
unsafe impl<T: Send> Sync for ReentrantMutex<T> {}

impl<T> UnwindSafe for ReentrantMutex<T> {}
impl<T> RefUnwindSafe for ReentrantMutex<T> {}

#[must_use = "if unused the ReentrantMutex will immediately unlock"]
pub struct ReentrantMutexGuard<'a, T: 'a> {
    lock: &'a ReentrantMutex<T>,
    // This marker ensures the guard is !Send
    _m: PhantomData<*mut ()>,
}

impl<T> ReentrantMutex<T> {
    /// Creates a new reentrant mutex in an unlocked state.
    pub const fn new(data: T) -> ReentrantMutex<T> {
        ReentrantMutex {
            raw: RawMutex::new(),
            owner: AtomicUsize::new(0),
            lock_count: UnsafeCell::new(0),
            data,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    pub fn is_owned_by_current_thread(&self) -> bool {
        let local_id = local_thread_id().get();
        self.owner.load(Ordering::Relaxed) == local_id
    }

    pub fn lock(&self) -> ReentrantMutexGuard<'_, T> {
        let local_id = local_thread_id().get();

        if self.owner.load(Ordering::Relaxed) == local_id {
            unsafe {
                self.increment_lock_count();
            }
        } else {
            self.raw.lock();

            self.owner.store(local_id, Ordering::Relaxed);

            unsafe {
                debug_assert_eq!(*self.lock_count.get(), 0);
                *self.lock_count.get() = 1;
            }
        }

        ReentrantMutexGuard {
            lock: self,
            _m: PhantomData,
        }
    }

    pub fn try_lock(&self) -> Option<ReentrantMutexGuard<'_, T>> {
        let local_id = local_thread_id().get();

        if self.owner.load(Ordering::Relaxed) == local_id {
            unsafe {
                self.increment_lock_count();
            }

            Some(ReentrantMutexGuard {
                lock: self,
                _m: PhantomData,
            })
        } else if self.raw.try_lock() {
            self.owner.store(local_id, Ordering::Relaxed);

            unsafe {
                debug_assert_eq!(*self.lock_count.get(), 0);
                *self.lock_count.get() = 1;
            }

            Some(ReentrantMutexGuard {
                lock: self,
                _m: PhantomData,
            })
        } else {
            None
        }
    }

    unsafe fn increment_lock_count(&self) {
        *self.lock_count.get() = (*self.lock_count.get())
            .checked_add(1)
            .expect("lock count overflow in reentrant mutex");
    }
}

impl<T> Deref for ReentrantMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.lock.data
    }
}

impl<T> Drop for ReentrantMutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            *self.lock.lock_count.get() -= 1;
            if *self.lock.lock_count.get() == 0 {
                self.lock.owner.store(0, Ordering::Relaxed);
                self.lock.raw.unlock();
            }
        }
    }
}

pub fn local_thread_id() -> NonZeroUsize {
    // The address of a thread-local variable is guaranteed to be unique t<o the
    // current thread, and is also guaranteed to be non-zero. The variable has to have a
    // non-zero size to guarantee it has a unique address for each thread.>
    #[thread_local]
    static X: u8 = 0;
    NonZeroUsize::new(addr_of!(X) as usize).expect("thread ID was zero")
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::sync::assert_not_send;
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn assert_mutex_guard_is_not_send() {
        assert_not_send!(ReentrantMutexGuard<()>);
    }

    #[test]
    fn is_mutex() {
        let m = Arc::new(ReentrantMutex::new(RefCell::new(0)));

        let m2 = m.clone();
        // This thread will be blocked
        let child = thread::spawn(move || {
            let g = m2.lock();
            assert_eq!(*g.borrow(), 1);
        });

        let g = m.lock();
        *g.borrow_mut() = 1;
        drop(g);
        child.join().unwrap();
    }

    #[test]
    fn try_lock() {
        let m = Arc::new(ReentrantMutex::new(()));
        let g = m.try_lock();
        assert!(g.is_some());
        assert!(m.is_locked());
        assert!(m.is_owned_by_current_thread());

        let _g2 = m.try_lock();

        let m2 = m.clone();
        thread::spawn(move || {
            let lock = m2.try_lock();
            assert!(lock.is_none());
        })
        .join()
        .unwrap();

        let _g3 = m.try_lock();
    }
}
