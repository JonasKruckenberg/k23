// Copyright 2017 Amanieu d'Antras
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::iter::FusedIterator;
use core::mem::MaybeUninit;
use core::panic::UnwindSafe;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use core::{fmt, mem, ptr, slice};
pub use thread_local::*;

/// The total number of buckets stored in each thread local.
/// All buckets combined can hold up to `usize::MAX - 1` entries.
const BUCKETS: usize = (usize::BITS - 1) as usize;

/// Thread-local variable wrapper
///
/// See the [module-level documentation](index.html) for more.
pub struct ThreadLocal<T: Send> {
    /// The buckets in the thread local. The nth bucket contains `2^n`
    /// elements. Each bucket is lazily allocated.
    buckets: [AtomicPtr<Entry<T>>; BUCKETS],

    /// The number of values in the thread local. This can be less than the real number of values,
    /// but is never more.
    values: AtomicUsize,
}

struct Entry<T> {
    present: AtomicBool,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Drop for Entry<T> {
    fn drop(&mut self) {
        // Safety: the API ensures we cannot access the TLS value after drop
        unsafe {
            if *self.present.get_mut() {
                ptr::drop_in_place((*self.value.get()).as_mut_ptr());
            }
        }
    }
}

// Safety: ThreadLocal is always Sync, even if T isn't
unsafe impl<T: Send> Sync for ThreadLocal<T> {}

impl<T: Send> Default for ThreadLocal<T> {
    fn default() -> ThreadLocal<T> {
        ThreadLocal::new()
    }
}

impl<T: Send> Drop for ThreadLocal<T> {
    fn drop(&mut self) {
        // Free each non-null bucket
        for (i, bucket) in self.buckets.iter_mut().enumerate() {
            let bucket_ptr = *bucket.get_mut();

            let this_bucket_size = 1 << i;

            if bucket_ptr.is_null() {
                continue;
            }

            // Safety: the API ensures we cannot access the TLS value after drop
            unsafe { deallocate_bucket(bucket_ptr, this_bucket_size) };
        }
    }
}

impl<T: Send> ThreadLocal<T> {
    /// Creates a new empty `ThreadLocal`.
    pub const fn new() -> ThreadLocal<T> {
        let buckets = [ptr::null_mut::<Entry<T>>(); BUCKETS];
        Self {
            // Safety: AtomicPtr has the same representation as a pointer and arrays have the same
            // representation as a sequence of their inner type.
            buckets: unsafe {
                mem::transmute::<[*mut Entry<T>; BUCKETS], [AtomicPtr<Entry<T>>; BUCKETS]>(buckets)
            },
            values: AtomicUsize::new(0),
        }
    }

    /// Creates a new `ThreadLocal` with an initial capacity. If less than the capacity threads
    /// access the thread local it will never reallocate. The capacity may be rounded up to the
    /// nearest power of two.
    pub fn with_capacity(capacity: usize) -> ThreadLocal<T> {
        let allocated_buckets =
            usize::try_from(usize::BITS).unwrap() - (capacity.leading_zeros() as usize);

        let mut buckets = [ptr::null_mut(); BUCKETS];
        for (i, bucket) in buckets[..allocated_buckets].iter_mut().enumerate() {
            *bucket = allocate_bucket::<T>(1 << i);
        }

        Self {
            // Safety: AtomicPtr has the same representation as a pointer and arrays have the same
            // representation as a sequence of their inner type.
            buckets: unsafe {
                mem::transmute::<[*mut Entry<T>; BUCKETS], [AtomicPtr<Entry<T>>; BUCKETS]>(buckets)
            },
            values: AtomicUsize::new(0),
        }
    }

    /// Returns the element for the current thread, if it exists.
    pub fn get(&self) -> Option<&T> {
        let hartid = crate::HART_LOCAL_MACHINE_INFO.with_borrow(|info| info.hartid);
        self.get_inner(Hart::new(hartid))
    }

    /// Returns the element for the current thread, or creates it if it doesn't
    /// exist.
    pub fn get_or<F>(&self, create: F) -> &T
    where
        F: FnOnce() -> T,
    {
        #[expect(tail_expr_drop_order, reason = "")]
        // Safety: value will be initialized by `create` if necessary
        unsafe {
            self.get_or_try(|| Ok::<T, ()>(create())).unwrap_unchecked()
        }
    }

    /// Returns the element for the current thread, or creates it if it doesn't
    /// exist. If `create` fails, that error is returned and no element is
    /// added.
    pub fn get_or_try<F, E>(&self, create: F) -> Result<&T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let hartid = crate::HART_LOCAL_MACHINE_INFO.with_borrow(|info| info.hartid);
        let hartid = Hart::new(hartid);

        if let Some(val) = self.get_inner(hartid) {
            return Ok(val);
        }

        #[expect(tail_expr_drop_order, reason = "")]
        Ok(self.insert(hartid, create()?))
    }

    fn get_inner(&self, hart: Hart) -> Option<&T> {
        // Safety: `Hart` constructors ensure correct bucket index
        let bucket_ptr = unsafe { self.buckets.get_unchecked(hart.bucket) }.load(Ordering::Acquire);
        if bucket_ptr.is_null() {
            return None;
        }

        // Safety: bucket ptr is always valid
        unsafe {
            let entry = &*bucket_ptr.add(hart.index);
            if entry.present.load(Ordering::Relaxed) {
                Some(&*(*entry.value.get()).as_ptr())
            } else {
                None
            }
        }
    }

    #[cold]
    fn insert(&self, thread: Hart, data: T) -> &T {
        // Safety: `Hart` constructors ensure correct bucket index
        let bucket_atomic_ptr = unsafe { self.buckets.get_unchecked(thread.bucket) };
        let bucket_ptr: *const _ = bucket_atomic_ptr.load(Ordering::Acquire);

        // If the bucket doesn't already exist, we need to allocate it
        let bucket_ptr = if bucket_ptr.is_null() {
            let new_bucket = allocate_bucket(thread.bucket_size);

            match bucket_atomic_ptr.compare_exchange(
                ptr::null_mut(),
                new_bucket,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => new_bucket,
                // If the bucket value changed (from null), that means
                // another thread stored a new bucket before we could,
                // and we can free our bucket and use that one instead
                Err(bucket_ptr) => {
                    // Safety: bucket will not be read from
                    unsafe { deallocate_bucket(new_bucket, thread.bucket_size) }
                    bucket_ptr
                }
            }
        } else {
            bucket_ptr
        };

        // Insert the new element into the bucket
        // Safety: `Hart` constructors ensure correct index
        let entry = unsafe { &*bucket_ptr.add(thread.index) };
        let value_ptr = entry.value.get();
        // Safety: we just initialized the bucket
        unsafe { value_ptr.write(MaybeUninit::new(data)) };
        entry.present.store(true, Ordering::Release);

        self.values.fetch_add(1, Ordering::Release);

        // Safety: we just initialized the value
        unsafe { &*(*value_ptr).as_ptr() }
    }

    /// Returns an iterator over the local values of all threads in unspecified
    /// order.
    ///
    /// This call can be done safely, as `T` is required to implement [`Sync`].
    pub fn iter(&self) -> Iter<'_, T>
    where
        T: Sync,
    {
        Iter {
            thread_local: self,
            raw: RawIter::new(),
        }
    }

    /// Returns a mutable iterator over the local values of all threads in
    /// unspecified order.
    ///
    /// Since this call borrows the `ThreadLocal` mutably, this operation can
    /// be done safely---the mutable borrow statically guarantees no other
    /// threads are currently accessing their associated values.
    pub fn iter_mut(&mut self) -> IterMut<T> {
        IterMut {
            thread_local: self,
            raw: RawIter::new(),
        }
    }

    /// Removes all thread-specific values from the `ThreadLocal`, effectively
    /// resetting it to its original state.
    ///
    /// Since this call borrows the `ThreadLocal` mutably, this operation can
    /// be done safely---the mutable borrow statically guarantees no other
    /// threads are currently accessing their associated values.
    pub fn clear(&mut self) {
        *self = ThreadLocal::new();
    }
}

impl<T: Send> IntoIterator for ThreadLocal<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter {
            thread_local: self,
            raw: RawIter::new(),
        }
    }
}

impl<'a, T: Send + Sync> IntoIterator for &'a ThreadLocal<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Send> IntoIterator for &'a mut ThreadLocal<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

impl<T: Send + Default> ThreadLocal<T> {
    /// Returns the element for the current thread, or creates a default one if
    /// it doesn't exist.
    pub fn get_or_default(&self) -> &T {
        self.get_or(Default::default)
    }
}

impl<T: Send + fmt::Debug> fmt::Debug for ThreadLocal<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ThreadLocal {{ local_data: {:?} }}", self.get())
    }
}

impl<T: Send + UnwindSafe> UnwindSafe for ThreadLocal<T> {}

#[derive(Debug)]
struct RawIter {
    yielded: usize,
    bucket: usize,
    bucket_size: usize,
    index: usize,
}
impl RawIter {
    #[inline]
    fn new() -> Self {
        Self {
            yielded: 0,
            bucket: 0,
            bucket_size: 1,
            index: 0,
        }
    }

    fn next<'a, T: Send + Sync>(&mut self, thread_local: &'a ThreadLocal<T>) -> Option<&'a T> {
        while self.bucket < BUCKETS {
            // Safety: `Hart` constructors ensure correct bucket index
            let bucket = unsafe { thread_local.buckets.get_unchecked(self.bucket) };
            let bucket = bucket.load(Ordering::Acquire);

            if !bucket.is_null() {
                while self.index < self.bucket_size {
                    // Safety: `Hart` constructors ensure correct index
                    let entry = unsafe { &*bucket.add(self.index) };
                    self.index += 1;
                    if entry.present.load(Ordering::Acquire) {
                        self.yielded += 1;
                        // Safety: we just ensured the value is valid
                        return Some(unsafe { &*(*entry.value.get()).as_ptr() });
                    }
                }
            }

            self.next_bucket();
        }
        None
    }
    fn next_mut<'a, T: Send>(
        &mut self,
        thread_local: &'a mut ThreadLocal<T>,
    ) -> Option<&'a mut Entry<T>> {
        if *thread_local.values.get_mut() == self.yielded {
            return None;
        }

        loop {
            // Safety: `Hart` constructors ensure correct bucket index
            let bucket = unsafe { thread_local.buckets.get_unchecked_mut(self.bucket) };
            let bucket = *bucket.get_mut();

            if !bucket.is_null() {
                while self.index < self.bucket_size {
                    // Safety: `Hart` constructors ensure correct index
                    let entry = unsafe { &mut *bucket.add(self.index) };
                    self.index += 1;
                    if *entry.present.get_mut() {
                        self.yielded += 1;
                        return Some(entry);
                    }
                }
            }

            self.next_bucket();
        }
    }

    #[inline]
    fn next_bucket(&mut self) {
        self.bucket_size <<= 1_i32;
        self.bucket += 1;
        self.index = 0;
    }

    fn size_hint<T: Send>(&self, thread_local: &ThreadLocal<T>) -> (usize, Option<usize>) {
        let total = thread_local.values.load(Ordering::Acquire);
        (total - self.yielded, None)
    }
    fn size_hint_frozen<T: Send>(&self, thread_local: &ThreadLocal<T>) -> (usize, Option<usize>) {
        // Safety: used as a hint, so racily reading the value is fine
        let total = unsafe { *ptr::from_ref::<AtomicUsize>(&thread_local.values).cast::<usize>() };
        let remaining = total - self.yielded;
        (remaining, Some(remaining))
    }
}

/// Iterator over the contents of a `ThreadLocal`.
#[derive(Debug)]
pub struct Iter<'a, T: Send + Sync> {
    thread_local: &'a ThreadLocal<T>,
    raw: RawIter,
}

impl<'a, T: Send + Sync> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        self.raw.next(self.thread_local)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw.size_hint(self.thread_local)
    }
}
impl<T: Send + Sync> FusedIterator for Iter<'_, T> {}

/// Mutable iterator over the contents of a `ThreadLocal`.
pub struct IterMut<'a, T: Send> {
    thread_local: &'a mut ThreadLocal<T>,
    raw: RawIter,
}

impl<'a, T: Send> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        self.raw
            .next_mut(self.thread_local)
            // Safety: constructor ensures all ptrs are valid
            .map(|entry| unsafe { &mut *(*entry.value.get()).as_mut_ptr() })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw.size_hint_frozen(self.thread_local)
    }
}

impl<T: Send> ExactSizeIterator for IterMut<'_, T> {}
impl<T: Send> FusedIterator for IterMut<'_, T> {}

// Manual impl so we don't call Debug on the ThreadLocal, as doing so would create a reference to
// this thread's value that potentially aliases with a mutable reference we have given out.
impl<T: Send + fmt::Debug> fmt::Debug for IterMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IterMut").field("raw", &self.raw).finish()
    }
}

/// An iterator that moves out of a `ThreadLocal`.
#[derive(Debug)]
pub struct IntoIter<T: Send> {
    thread_local: ThreadLocal<T>,
    raw: RawIter,
}

impl<T: Send> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.raw.next_mut(&mut self.thread_local).map(|entry| {
            *entry.present.get_mut() = false;
            // Safety: constructor ensures all ptrs are valid
            unsafe { mem::replace(&mut *entry.value.get(), MaybeUninit::uninit()).assume_init() }
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw.size_hint_frozen(&self.thread_local)
    }
}

impl<T: Send> ExactSizeIterator for IntoIter<T> {}
impl<T: Send> FusedIterator for IntoIter<T> {}

/// Data which is unique to the current hart.
#[derive(Clone, Copy)]
struct Hart {
    #[expect(unused, reason = "")]
    id: usize,
    /// The bucket this hart's local storage will be in.
    bucket: usize,
    /// The size of the bucket this hart's local storage will be in.
    bucket_size: usize,
    /// The index into the bucket this hart's local storage is in.
    index: usize,
}
impl Hart {
    fn new(id: usize) -> Self {
        let bucket =
            usize::try_from(usize::BITS).unwrap() - ((id + 1).leading_zeros() as usize) - 1;
        let bucket_size = 1 << bucket;
        let index = id - (bucket_size - 1);

        Self {
            id,
            bucket,
            bucket_size,
            index,
        }
    }
}

fn allocate_bucket<T>(size: usize) -> *mut Entry<T> {
    Box::into_raw(
        (0..size)
            .map(|_| Entry::<T> {
                present: AtomicBool::new(false),
                value: UnsafeCell::new(MaybeUninit::uninit()),
            })
            .collect(),
    )
    .cast()
}

unsafe fn deallocate_bucket<T>(bucket: *mut Entry<T>, size: usize) {
    // Safety: we allocated the entry through `Box::new`
    let _ = unsafe { Box::from_raw(slice::from_raw_parts_mut(bucket, size)) };
}

// #[cfg(test)]
// mod tests {
//     use alloc::string::String;
//     use super::*;
//
//     use core::cell::RefCell;
//     use core::sync::atomic::AtomicUsize;
//     use core::sync::atomic::Ordering::Relaxed;
//     use alloc::sync::Arc;
//
//     // fn make_create() -> Arc<dyn Fn() -> usize + Send + Sync> {
//     //     let count = AtomicUsize::new(0);
//     //     Arc::new(move || count.fetch_add(1, Relaxed))
//     // }
//
//     // #[ktest::test]
//     // fn same_thread() {
//     //     let create = make_create();
//     //     let mut tls = ThreadLocal::new();
//     //     assert_eq!(None, tls.get());
//     //     assert_eq!("ThreadLocal { local_data: None }", format!("{:?}", &tls));
//     //     assert_eq!(0, *tls.get_or(|| create()));
//     //     assert_eq!(Some(&0), tls.get());
//     //     assert_eq!(0, *tls.get_or(|| create()));
//     //     assert_eq!(Some(&0), tls.get());
//     //     assert_eq!(0, *tls.get_or(|| create()));
//     //     assert_eq!(Some(&0), tls.get());
//     //     assert_eq!("ThreadLocal { local_data: Some(0) }", format!("{:?}", &tls));
//     //     tls.clear();
//     //     assert_eq!(None, tls.get());
//     // }
//
//     // #[test]
//     // fn different_thread() {
//     //     let create = make_create();
//     //     let tls = Arc::new(ThreadLocal::new());
//     //     assert_eq!(None, tls.get());
//     //     assert_eq!(0, *tls.get_or(|| create()));
//     //     assert_eq!(Some(&0), tls.get());
//     //
//     //     let tls2 = tls.clone();
//     //     let create2 = create.clone();
//     //     thread::spawn(move || {
//     //         assert_eq!(None, tls2.get());
//     //         assert_eq!(1, *tls2.get_or(|| create2()));
//     //         assert_eq!(Some(&1), tls2.get());
//     //     })
//     //     .join()
//     //     .unwrap();
//     //
//     //     assert_eq!(Some(&0), tls.get());
//     //     assert_eq!(0, *tls.get_or(|| create()));
//     // }
//
//     // #[test]
//     // fn iter() {
//     //     let tls = Arc::new(ThreadLocal::new());
//     //     tls.get_or(|| Box::new(1));
//     //
//     //     let tls2 = tls.clone();
//     //     thread::spawn(move || {
//     //         tls2.get_or(|| Box::new(2));
//     //         let tls3 = tls2.clone();
//     //         thread::spawn(move || {
//     //             tls3.get_or(|| Box::new(3));
//     //         })
//     //         .join()
//     //         .unwrap();
//     //         drop(tls2);
//     //     })
//     //     .join()
//     //     .unwrap();
//     //
//     //     let mut tls = Arc::try_unwrap(tls).unwrap();
//     //
//     //     let mut v = tls.iter().map(|x| **x).collect::<Vec<i32>>();
//     //     v.sort_unstable();
//     //     assert_eq!(vec![1, 2, 3], v);
//     //
//     //     let mut v = tls.iter_mut().map(|x| **x).collect::<Vec<i32>>();
//     //     v.sort_unstable();
//     //     assert_eq!(vec![1, 2, 3], v);
//     //
//     //     let mut v = tls.into_iter().map(|x| *x).collect::<Vec<i32>>();
//     //     v.sort_unstable();
//     //     assert_eq!(vec![1, 2, 3], v);
//     // }
//
//     // #[test]
//     // fn miri_iter_soundness_check() {
//     //     let tls = Arc::new(ThreadLocal::new());
//     //     let _local = tls.get_or(|| Box::new(1));
//     //
//     //     let tls2 = tls.clone();
//     //     let join_1 = thread::spawn(move || {
//     //         let _tls = tls2.get_or(|| Box::new(2));
//     //         let iter = tls2.iter();
//     //         for item in iter {
//     //             println!("{:?}", item);
//     //         }
//     //     });
//     //
//     //     let iter = tls.iter();
//     //     for item in iter {
//     //         println!("{:?}", item);
//     //     }
//     //
//     //     join_1.join().ok();
//     // }
//
//     #[ktest::test]
//     fn test_drop() {
//         let local = ThreadLocal::new();
//         struct Dropped(Arc<AtomicUsize>);
//         impl Drop for Dropped {
//             fn drop(&mut self) {
//                 self.0.fetch_add(1, Relaxed);
//             }
//         }
//
//         let dropped = Arc::new(AtomicUsize::new(0));
//         local.get_or(|| Dropped(dropped.clone()));
//         assert_eq!(dropped.load(Relaxed), 0);
//         drop(local);
//         assert_eq!(dropped.load(Relaxed), 1);
//     }
//
//     #[ktest::test]
//     fn test_earlyreturn_buckets() {
//         struct Dropped(Arc<AtomicUsize>);
//         impl Drop for Dropped {
//             fn drop(&mut self) {
//                 self.0.fetch_add(1, Relaxed);
//             }
//         }
//         let dropped = Arc::new(AtomicUsize::new(0));
//
//         // We use a high `id` here to guarantee that a lazily allocated bucket somewhere in the middle is used.
//         // Neither iteration nor `Drop` must early-return on `null` buckets that are used for lower `buckets`.
//         let thread = Hart::new(1234);
//         assert!(thread.bucket > 1);
//
//         let mut local = ThreadLocal::new();
//         local.insert(thread, Dropped(dropped.clone()));
//
//         let item = local.iter().next().unwrap();
//         assert_eq!(item.0.load(Relaxed), 0);
//         let item = local.iter_mut().next().unwrap();
//         assert_eq!(item.0.load(Relaxed), 0);
//         drop(local);
//         assert_eq!(dropped.load(Relaxed), 1);
//     }
//
//     #[ktest::test]
//     fn is_sync() {
//         fn foo<T: Sync>() {}
//         foo::<ThreadLocal<String>>();
//         foo::<ThreadLocal<RefCell<String>>>();
//     }
//
//     #[ktest::test]
//     fn test_thread() {
//         let thread = Hart::new(0);
//         assert_eq!(thread.id, 0);
//         assert_eq!(thread.bucket, 0);
//         assert_eq!(thread.bucket_size, 1);
//         assert_eq!(thread.index, 0);
//
//         let thread = Hart::new(1);
//         assert_eq!(thread.id, 1);
//         assert_eq!(thread.bucket, 1);
//         assert_eq!(thread.bucket_size, 2);
//         assert_eq!(thread.index, 0);
//
//         let thread = Hart::new(2);
//         assert_eq!(thread.id, 2);
//         assert_eq!(thread.bucket, 1);
//         assert_eq!(thread.bucket_size, 2);
//         assert_eq!(thread.index, 1);
//
//         let thread = Hart::new(3);
//         assert_eq!(thread.id, 3);
//         assert_eq!(thread.bucket, 2);
//         assert_eq!(thread.bucket_size, 4);
//         assert_eq!(thread.index, 0);
//
//         let thread = Hart::new(19);
//         assert_eq!(thread.id, 19);
//         assert_eq!(thread.bucket, 4);
//         assert_eq!(thread.bucket_size, 16);
//         assert_eq!(thread.index, 4);
//     }
// }
