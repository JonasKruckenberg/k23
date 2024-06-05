use super::raw_mutex::RawMutex;
use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

pub struct Mutex<T: ?Sized> {
    raw: RawMutex,
    data: UnsafeCell<T>,
}

#[must_use = "if unused the Mutex will immediately unlock"]
#[clippy::has_significant_drop]
pub struct MutexGuard<'a, T: ?Sized> {
    lock: &'a Mutex<T>,
    // This marker ensures the guard is !Send
    _m: PhantomData<*mut ()>,
}

unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Mutex {
            raw: RawMutex::new(),
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.raw.lock();

        MutexGuard {
            lock: self,
            _m: PhantomData,
        }
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self.raw.try_lock() {
            Some(MutexGuard {
                lock: self,
                _m: PhantomData,
            })
        } else {
            None
        }
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.raw.unlock();
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Mutex");
        match self.try_lock() {
            Some(guard) => {
                d.field("data", &&*guard);
            }
            None => {
                d.field("data", &format_args!("<locked>"));
            }
        }
        d.finish_non_exhaustive()
    }
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
        assert_not_send!(MutexGuard<()>);
    }

    #[test]
    fn is_mutex() {
        let m = Arc::new(Mutex::new(RefCell::new(0)));

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
        let m = Arc::new(Mutex::new(()));
        let g = m.try_lock();
        assert!(g.is_some());
        assert!(m.is_locked());

        assert!(m.try_lock().is_none());

        let m2 = m.clone();
        thread::spawn(move || {
            let lock = m2.try_lock();
            assert!(lock.is_none());
        })
        .join()
        .unwrap();
    }
}
