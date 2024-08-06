mod lazy_lock;
mod once;
mod once_lock;
mod raw_mutex;

use core::num::NonZeroUsize;
use core::ptr::addr_of;
pub use lazy_lock::LazyLock;
use lock_api::GetThreadId;
pub use once::Once;
pub use once_lock::OnceLock;

pub use raw_mutex::RawMutex;

pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;
pub type ReentrantMutex<T> = lock_api::ReentrantMutex<RawMutex, LocalThreadId, T>;
pub type ReentrantMutexGuard<'a, T> = lock_api::ReentrantMutexGuard<'a, RawMutex, LocalThreadId, T>;

pub struct LocalThreadId;

unsafe impl GetThreadId for LocalThreadId {
    const INIT: Self = LocalThreadId;

    fn nonzero_thread_id(&self) -> NonZeroUsize {
        // The address of a thread-local variable is guaranteed to be unique t<o the
        // current thread, and is also guaranteed to be non-zero. The variable has to have a
        // non-zero size to guarantee it has a unique address for each thread.>
        #[thread_local]
        static X: u8 = 0;
        NonZeroUsize::new(addr_of!(X) as usize).expect("thread ID was zero")
    }
}
