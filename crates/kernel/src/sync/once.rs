use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

const STATUS_INIT: u8 = 0;
const STATUS_RUNNING: u8 = 1;
const STATUS_COMPLETE: u8 = 2;
const STATUS_PANICKED: u8 = 3;

pub struct Once<T> {
    // safety invariant: This must only ever be one of `UNTOUCHED`, `RUNNING`, `COMPLETE`, or `PANICKED`
    status: AtomicU8,
    data: UnsafeCell<MaybeUninit<T>>,
}

// Once allows for concurrent reads.
unsafe impl<T: Send + Sync> Sync for Once<T> {}
unsafe impl<T: Send> Send for Once<T> {}

impl<T> Drop for Once<T> {
    fn drop(&mut self) {
        if *self.status.get_mut() == STATUS_COMPLETE {
            unsafe {
                self.data.get_mut().assume_init_drop();
            }
        }
    }
}

impl<T> Default for Once<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> Once<T> {
    pub const fn empty() -> Self {
        Self {
            status: AtomicU8::new(STATUS_INIT),
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        if let Some(data) = self.get() {
            data
        } else {
            self.get_or_init_slow(f)
        }
    }

    #[cold]
    fn get_or_init_slow<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        loop {
            let xchg = self.status.compare_exchange(
                STATUS_INIT,
                STATUS_RUNNING,
                Ordering::Acquire,
                Ordering::Acquire,
            );

            if let Err(status) = xchg {
                match status {
                    STATUS_PANICKED => panic!("Once panicked"),
                    STATUS_RUNNING => match self.poll() {
                        Some(data) => return data,
                        None => continue,
                    },
                    STATUS_COMPLETE => unsafe { return self.force_get() },
                    _ => unreachable!(),
                }
            }

            let val = f();

            unsafe {
                // SAFETY:
                // `UnsafeCell`/deref: currently the only accessor, mutably
                // and immutably by cas exclusion.
                // `write`: pointer comes from `MaybeUninit`.
                (*self.data.get()).as_mut_ptr().write(val);
            };

            self.status.store(STATUS_COMPLETE, Ordering::Release);

            return unsafe { self.force_get() };
        }
    }

    pub fn get(&self) -> Option<&T> {
        match self.status.load(Ordering::Acquire) {
            STATUS_COMPLETE => Some(unsafe { self.force_get() }),
            _ => None,
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

    unsafe fn force_get(&self) -> &T {
        // SAFETY:
        // * `UnsafeCell`/inner deref: data never changes again
        // * `MaybeUninit`/outer deref: data was initialized
        &*(*self.data.get()).as_ptr()
    }

    fn poll(&self) -> Option<&T> {
        loop {
            // SAFETY: Acquire is safe here, because if the status is COMPLETE, then we want to make
            // sure that all memory accessed done while initializing that value, are visible when
            // we return a reference to the inner data after this load.
            match self.status.load(Ordering::Acquire) {
                STATUS_INIT => return None,
                STATUS_RUNNING => core::hint::spin_loop(),
                STATUS_COMPLETE => return Some(unsafe { self.force_get() }),
                STATUS_PANICKED => panic!("Once previously poisoned by a panicked"),
                _ => unreachable!(),
            }
        }
    }
}
