// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::Backoff;

pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;

pub struct RawMutex {
    lock: AtomicBool,
}

#[allow(clippy::undocumented_unsafe_blocks, reason = "TODO")]
unsafe impl lock_api::RawMutex for RawMutex {
    type GuardMarker = lock_api::GuardSend;

    const INIT: Self = Self {
        lock: AtomicBool::new(false),
    };

    fn lock(&self) {
        let mut boff = Backoff::default();
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.is_locked() {
                boff.spin();
            }
        }
    }

    fn try_lock(&self) -> bool {
        self.lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    // fn try_lock_weak(&self) -> bool {
    //     self.lock
    //         .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
    //         .is_ok()
    // }

    unsafe fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }
}

// #[cfg(test)]
// mod tests {
//     use core::fmt::Debug;
//     use std::{hint, mem};
//
//     use super::*;
//     use crate::loom::{Arc, AtomicUsize};
//
//     #[derive(Eq, PartialEq, Debug)]
//     struct NonCopy(i32);
//
//     #[derive(Eq, PartialEq, Debug)]
//     struct NonCopyNeedsDrop(i32);
//
//     impl Drop for NonCopyNeedsDrop {
//         fn drop(&mut self) {
//             hint::black_box(());
//         }
//     }
//
//     #[test]
//     fn test_needs_drop() {
//         assert!(!mem::needs_drop::<NonCopy>());
//         assert!(mem::needs_drop::<NonCopyNeedsDrop>());
//     }
//
//     #[test]
//     fn smoke() {
//         let m = Mutex::new(());
//         drop(m.lock());
//         drop(m.lock());
//     }
//
//     #[test]
//     fn try_lock() {
//         let mutex = Mutex::<_>::new(42);
//
//         // First lock succeeds
//         let a = mutex.try_lock();
//         assert_eq!(a.as_ref().map(|r| **r), Some(42));
//
//         // Additional lock fails
//         let b = mutex.try_lock();
//         assert!(b.is_none());
//
//         // After dropping lock, it succeeds again
//         drop(a);
//         let c = mutex.try_lock();
//         assert_eq!(c.as_ref().map(|r| **r), Some(42));
//     }
//
//     #[test]
//     fn test_into_inner() {
//         let m = Mutex::<_>::new(NonCopy(10));
//         assert_eq!(m.into_inner(), NonCopy(10));
//     }
//
//     #[test]
//     fn test_into_inner_drop() {
//         struct Foo(Arc<AtomicUsize>);
//         impl Drop for Foo {
//             fn drop(&mut self) {
//                 self.0.fetch_add(1, Ordering::SeqCst);
//             }
//         }
//         let num_drops = Arc::new(AtomicUsize::new(0));
//         let m = Mutex::<_>::new(Foo(num_drops.clone()));
//         assert_eq!(num_drops.load(Ordering::SeqCst), 0);
//         {
//             let _inner = m.into_inner();
//             assert_eq!(num_drops.load(Ordering::SeqCst), 0);
//         }
//         assert_eq!(num_drops.load(Ordering::SeqCst), 1);
//     }
//
//     #[test]
//     fn test_mutex_unsized() {
//         let mutex: &Mutex<[i32]> = &Mutex::<_>::new([1, 2, 3]);
//         {
//             let b = &mut *mutex.lock();
//             b[0] = 4;
//             b[2] = 5;
//         }
//         let comp: &[i32] = &[4, 2, 5];
//         assert_eq!(&*mutex.lock(), comp);
//     }
//
//     #[test]
//     fn test_mutex_force_lock() {
//         let lock = Mutex::<_>::new(());
//         mem::forget(lock.lock());
//         unsafe {
//             lock.force_unlock();
//         }
//         assert!(lock.try_lock().is_some());
//     }
//
//     #[test]
//     fn test_get_mut() {
//         let mut m = Mutex::new(NonCopy(10));
//         *m.get_mut() = NonCopy(20);
//         assert_eq!(m.into_inner(), NonCopy(20));
//     }
//
//     #[test]
//     fn basic_multi_threaded() {
//         use crate::loom::{self, Arc, thread};
//
//         #[allow(tail_expr_drop_order)]
//         fn incr(lock: &Arc<Mutex<i32>>) -> thread::JoinHandle<()> {
//             let lock = lock.clone();
//             thread::spawn(move || {
//                 let mut lock = lock.lock();
//                 *lock += 1;
//             })
//         }
//
//         loom::model(|| {
//             let lock = Arc::new(Mutex::new(0));
//             let t1 = incr(&lock);
//             let t2 = incr(&lock);
//
//             t1.join().unwrap();
//             t2.join().unwrap();
//
//             thread::spawn(move || {
//                 let lock = lock.lock();
//                 assert_eq!(*lock, 2)
//             });
//         });
//     }
// }
