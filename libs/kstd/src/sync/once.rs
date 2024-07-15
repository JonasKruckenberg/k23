use core::cell::UnsafeCell;
use core::mem;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

const STATUS_INCOMPLETE: u8 = 0;
const STATUS_RUNNING: u8 = 1;
const STATUS_COMPLETE: u8 = 2;
const STATUS_POISONED: u8 = 3;

pub struct Once<T = ()> {
    status: AtomicU8,
    data: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T: Send + Sync> Sync for Once<T> {}
unsafe impl<T: Send> Send for Once<T> {}

impl<T> Once<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            status: AtomicU8::new(STATUS_INCOMPLETE),
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// # Panics
    ///
    /// Panics if the closure errors or panics.
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        self.get_or_try_init::<_, ()>(|| Ok(f()))
            .expect("get_or_init failed")
    }

    /// # Errors
    ///
    /// Returns an error if the given closure errors.
    ///
    /// # Panics
    ///
    /// Panics if the closure panics.
    pub fn get_or_try_init<F, E>(&self, f: F) -> Result<&T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        if let Some(value) = self.get() {
            Ok(value)
        } else {
            self.get_or_try_init_slow(f)
        }
    }

    fn get_or_try_init_slow<F: FnOnce() -> Result<T, E>, E>(&self, f: F) -> Result<&T, E> {
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

                    let value = match f() {
                        Ok(val) => val,
                        Err(err) => {
                            // If an error occurs, clean up everything and leave.
                            mem::forget(panic_guard);
                            self.status.store(STATUS_INCOMPLETE, Ordering::Release);
                            return Err(err);
                        }
                    };

                    unsafe {
                        (*self.data.get()).as_mut_ptr().write(value);
                    }

                    mem::forget(panic_guard);

                    self.status.store(STATUS_COMPLETE, Ordering::Release);

                    return unsafe { Ok(self.force_get()) };
                }
                Err(STATUS_RUNNING) => match self.poll() {
                    Some(v) => return Ok(v),
                    None => continue,
                },
                Err(STATUS_COMPLETE) => return Ok(unsafe { self.force_get() }),
                Err(STATUS_POISONED) => panic!("Once poisoned by panic"),
                _ => unreachable!(),
            }
        }
    }

    fn poll(&self) -> Option<&T> {
        loop {
            match self.status.load(Ordering::Acquire) {
                STATUS_INCOMPLETE => return None,
                STATUS_RUNNING => core::hint::spin_loop(),
                STATUS_COMPLETE => return Some(unsafe { self.force_get() }),
                STATUS_POISONED => panic!("Once poisoned by panic"),
                _ => unreachable!(),
            }
        }
    }

    pub fn wait(&self) -> &T {
        loop {
            match self.poll() {
                Some(x) => break x,
                None => core::hint::spin_loop(),
            }
        }
    }

    pub fn get(&self) -> Option<&T> {
        match self.status.load(Ordering::Acquire) {
            STATUS_COMPLETE => Some(unsafe { self.force_get() }),
            _ => None,
        }
    }
    pub fn get_mut(&mut self) -> Option<&mut T> {
        match self.status.load(Ordering::Acquire) {
            STATUS_COMPLETE => Some(unsafe { self.force_get_mut() }),
            _ => None,
        }
    }

    unsafe fn force_get(&self) -> &T {
        // SAFETY:
        // * `UnsafeCell`/inner deref: data never changes again
        // * `MaybeUninit`/outer deref: data was initialized
        &*(*self.data.get()).as_ptr()
    }

    unsafe fn force_get_mut(&mut self) -> &mut T {
        // SAFETY:
        // * `UnsafeCell`/inner deref: data never changes again
        // * `MaybeUninit`/outer deref: data was initialized
        &mut *(*self.data.get()).as_mut_ptr()
    }
}

impl<T> Default for Once<T> {
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
