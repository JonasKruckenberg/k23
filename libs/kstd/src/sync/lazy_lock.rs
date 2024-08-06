//! Spin-based port of the `std::sync::LazyLock` type.

use super::{once::ExclusiveState, Once};
use core::{
    cell::UnsafeCell,
    fmt,
    mem::ManuallyDrop,
    ops::Deref,
    panic::{RefUnwindSafe, UnwindSafe},
    ptr,
};

union Data<T, F> {
    value: ManuallyDrop<T>,
    f: ManuallyDrop<F>,
}

pub struct LazyLock<T, F = fn() -> T> {
    once: Once,
    data: UnsafeCell<Data<T, F>>,
}

impl<T, F: FnOnce() -> T> LazyLock<T, F> {
    pub const fn new(f: F) -> Self {
        Self {
            once: Once::new(),
            data: UnsafeCell::new(Data {
                f: ManuallyDrop::new(f),
            }),
        }
    }

    /// # Errors
    ///
    /// Returns the initialization closure as the error in case the lock is not yet initialized.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned.
    pub fn into_inner(mut this: Self) -> Result<T, F> {
        let state = this.once.state();
        match state {
            ExclusiveState::Poisoned => panic!("LazyLock instance has previously been poisoned"),
            state => {
                let this = ManuallyDrop::new(this);
                let data = unsafe { ptr::read(&this.data) }.into_inner();
                match state {
                    ExclusiveState::Incomplete => Err(ManuallyDrop::into_inner(unsafe { data.f })),
                    ExclusiveState::Complete => Ok(ManuallyDrop::into_inner(unsafe { data.value })),
                    ExclusiveState::Poisoned => unreachable!(),
                }
            }
        }
    }

    #[inline]
    pub fn force(this: &LazyLock<T, F>) -> &T {
        this.once.call_once(|| {
            // SAFETY: `call_once` only runs this closure once, ever.
            let data = unsafe { &mut *this.data.get() };
            let f = unsafe { ManuallyDrop::take(&mut data.f) };
            let value = f();
            data.value = ManuallyDrop::new(value);
        });

        unsafe { &(*this.data.get()).value }
    }
}

impl<T, F> LazyLock<T, F> {
    /// Get the inner value if it has already been initialized.
    fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            // SAFETY:
            // The closure has been run successfully, so `value` has been initialized
            // and will not be modified again.
            Some(unsafe { &*(*self.data.get()).value })
        } else {
            None
        }
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyLock<T, F> {
    type Target = T;

    /// Dereferences the value.
    ///
    /// This method will block the calling thread if another initialization
    /// routine is currently running.
    ///
    #[inline]
    fn deref(&self) -> &T {
        LazyLock::force(self)
    }
}

impl<T, F> Drop for LazyLock<T, F> {
    fn drop(&mut self) {
        match self.once.state() {
            ExclusiveState::Incomplete => unsafe { ManuallyDrop::drop(&mut self.data.get_mut().f) },
            ExclusiveState::Complete => unsafe {
                ManuallyDrop::drop(&mut self.data.get_mut().value);
            },
            ExclusiveState::Poisoned => {}
        }
    }
}

impl<T: Default> Default for LazyLock<T> {
    /// Creates a new lazy value using `Default` as the initializing function.
    #[inline]
    fn default() -> LazyLock<T> {
        LazyLock::new(T::default)
    }
}

impl<T: fmt::Debug, F> fmt::Debug for LazyLock<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_tuple("LazyLock");
        match self.get() {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}

unsafe impl<T: Sync + Send, F: Send> Sync for LazyLock<T, F> {}

impl<T: RefUnwindSafe + UnwindSafe, F: UnwindSafe> RefUnwindSafe for LazyLock<T, F> {}
impl<T: UnwindSafe, F: UnwindSafe> UnwindSafe for LazyLock<T, F> {}
