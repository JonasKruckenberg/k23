#![cfg_attr(not(test), no_std)]
// #![no_std]

use core::borrow::{Borrow, BorrowMut};
use core::error::Error;
use core::hash::{Hash, Hasher};
use core::mem::MaybeUninit;
use core::ops::{Bound, Deref, DerefMut, RangeBounds};
use core::ptr::NonNull;
use core::{cmp, fmt, mem, ptr, slice};

pub struct CapacityError<T>(pub T);

impl<T> Error for CapacityError<T> {}

impl<T> fmt::Display for CapacityError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("insufficient capacity")
    }
}

impl<T> fmt::Debug for CapacityError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CapacityError: insufficient capacity")
    }
}

/// A vector with a fixed capacity.
///
/// The `ArrayVec` is a vector backed by a fixed size array. Elements are stored inline in the vector
/// itself (rather than on the heap) making `ArrayVec` suitable to be allocated on the stack or
/// used in `const` contexts.
///
/// The maximum capacity of the vector is determined by the `CAP` generic parameter, attempting to
/// insert more elements than `CAP` will always fail.
pub struct ArrayVec<T, const CAP: usize> {
    len: usize,
    data: [MaybeUninit<T>; CAP],
}

impl<T, const CAP: usize> Default for ArrayVec<T, CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAP: usize> Drop for ArrayVec<T, CAP> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T, const CAP: usize> ArrayVec<T, CAP> {
    /// Create a new empty `ArrayVec`.
    ///
    /// The maximum capacity is given by the generic parameter `CAP`.
    #[inline]
    pub const fn new() -> Self {
        Self {
            data: [const { MaybeUninit::uninit() }; CAP],
            len: 0,
        }
    }

    /// Returns the number of elements in the `ArrayVec`.
    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the `ArrayVec` is empty, `false` otherwise.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the capacity of the `ArrayVec`.
    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        CAP
    }

    /// Returns `true` if the `ArrayVec` is completely filled to its capacity, `false` otherwise.
    pub const fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }

    /// Returns the capacity left in the `ArrayVec`.
    pub const fn remaining_capacity(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Returns a raw pointer to the vector’s buffer.
    pub const fn as_ptr(&self) -> *const T {
        self.data.as_ptr().cast()
    }

    /// Returns a raw mutable pointer to the vector’s buffer.
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self.data.as_mut_ptr().cast()
    }

    /// Extracts a slice containing the entire vector.
    pub const fn as_slice(&self) -> &[T] {
        // SAFETY: `slice::from_raw_parts` requires
        // 1. pointee is a contiguous, aligned buffer of size `len` containing properly-initialized `T`s.
        // 2. Data must not be mutated for the returned lifetime.
        // 3. Further, `len * size_of::<T>` <= `isize::MAX`, and allocation does not "wrap" through overflowing memory addresses.
        //
        // The ArrayVec API guarantees properly-initialized items within 0..len
        // and the backing store being a Rust array guarantees correct alignment and contiguity.
        // 3. Is also guaranteed by construction (can't express a type that's too large) and we
        // since we borrow self here 2. is upheld as well.
        unsafe { slice::from_raw_parts(self.as_ptr(), self.len) }
    }

    /// Extracts a mutable slice of the entire vector.
    pub const fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: `slice::from_raw_parts` requires
        // 1. pointee is a contiguous, aligned buffer of size `len` containing properly-initialized `T`s.
        // 2. Data must not be mutated for the returned lifetime.
        // 3. Further, `len * size_of::<T>` <= `isize::MAX`, and allocation does not "wrap" through overflowing memory addresses.
        //
        // The ArrayVec API guarantees properly-initialized items within 0..len
        // and the backing store being a Rust array guarantees correct alignment and contiguity.
        // 3. Is also guaranteed by construction (can't express a type that's too large) and we
        // since we borrow self here 2. is upheld as well.
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), self.len) }
    }

    /// Push `element` to the end of the vector.
    ///
    /// # Panics
    ///
    /// Panics if the `ArrayVec` is full.
    pub fn push(&mut self, element: T) {
        self.try_push(element).unwrap();
    }

    /// Push `element` to the end of the vector.
    ///
    /// # Errors
    ///
    /// Returns `Err(CapacityError)` with the element if the `ArrayVec` is full.
    pub const fn try_push(&mut self, element: T) -> Result<(), CapacityError<T>> {
        if self.len() < CAP {
            // Safety: we have checked the capacity above
            unsafe {
                self.push_unchecked(element);
            }
            Ok(())
        } else {
            Err(CapacityError(element))
        }
    }

    /// Push `element` to the end of the vector, without doing bounds checking.
    ///
    /// # Safety
    ///
    /// Calling this method with on an already full vector is *[undefined behavior]*.
    /// The caller has to ensure that `self.len() < self.capacity()`.
    #[track_caller]
    pub const unsafe fn push_unchecked(&mut self, element: T) {
        let len = self.len();
        debug_assert!(len < CAP);
        self.data[len].write(element);
        self.len += 1;
    }

    /// Remove all elements in the vector.
    pub fn clear(&mut self) {
        let len = self.len;
        self.len = 0;
        for elt in self.data.iter_mut().take(len) {
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            unsafe { MaybeUninit::assume_init_drop(elt) };
        }
    }

    /// Retains only the elements specified by the predicate.
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        // The implementation below is taken from std::vec::Vec

        let original_len = self.len();
        self.len = 0;

        struct BackshiftOnDrop<'a, T, const CAP: usize> {
            v: &'a mut ArrayVec<T, CAP>,
            processed_len: usize,
            deleted_cnt: usize,
            original_len: usize,
        }

        impl<T, const CAP: usize> Drop for BackshiftOnDrop<'_, T, CAP> {
            fn drop(&mut self) {
                if self.deleted_cnt > 0 {
                    // Safety: Trailing unchecked items must be valid since we never touch them.
                    unsafe {
                        ptr::copy(
                            self.v.as_ptr().add(self.processed_len),
                            self.v
                                .as_mut_ptr()
                                .add(self.processed_len - self.deleted_cnt),
                            self.original_len - self.processed_len,
                        );
                    }
                }
                self.v.len = self.original_len - self.deleted_cnt;
            }
        }

        let mut g = BackshiftOnDrop {
            v: self,
            processed_len: 0,
            deleted_cnt: 0,
            original_len,
        };

        #[inline(always)]
        fn process_one<F: FnMut(&mut T) -> bool, T, const CAP: usize, const DELETED: bool>(
            f: &mut F,
            g: &mut BackshiftOnDrop<'_, T, CAP>,
        ) -> bool {
            // Safety: Unchecked element must be valid.
            let cur = unsafe { g.v.as_mut_ptr().add(g.processed_len) };
            // Safety: Unchecked element must be valid.
            if !f(unsafe { cur.as_mut().unwrap() }) {
                g.processed_len += 1;
                g.deleted_cnt += 1;
                // Safety: We never touch this element again after dropped.
                unsafe { ptr::drop_in_place(cur) };
                return false;
            }
            if DELETED {
                // Safety: `deleted_cnt` > 0, so the hole slot must not overlap with current element.
                // We use copy for move, and never touch this element again.
                unsafe {
                    let hole_slot = cur.sub(g.deleted_cnt);
                    ptr::copy_nonoverlapping(cur, hole_slot, 1);
                }
            }
            g.processed_len += 1;
            true
        }

        // Stage 1: Nothing was deleted.
        while g.processed_len != original_len {
            if !process_one::<F, T, CAP, false>(&mut f, &mut g) {
                break;
            }
        }

        // Stage 2: Some elements were deleted.
        while g.processed_len != original_len {
            process_one::<F, T, CAP, true>(&mut f, &mut g);
        }

        drop(g);
    }

    /// Removes the subslice indicated by the given range from the vector,
    /// returning a double-ended iterator over the removed subslice.
    ///
    /// If the iterator is dropped before being fully consumed,
    /// it drops the remaining removed elements.
    ///
    /// The returned iterator keeps a mutable borrow on the vector to optimize
    /// its implementation.
    ///
    /// # Panics
    ///
    /// Panics if the starting point is greater than the end point or if
    /// the end point is greater than the length of the vector.
    ///
    /// # Leaking
    ///
    /// If the returned iterator goes out of scope without being dropped (due to
    /// [`mem::forget`], for example), the vector may have lost and leaked
    /// elements arbitrarily, including elements outside the range.
    pub fn drain<R>(&mut self, range: R) -> Drain<'_, T, CAP>
    where
        R: RangeBounds<usize>,
    {
        // Memory safety
        //
        // When the Drain is first created, it shortens the length of
        // the source vector to make sure no uninitialized or moved-from elements
        // are accessible at all if the Drain's destructor never gets to run.
        //
        // Drain will ptr::read out the values to remove.
        // When finished, remaining tail of the vec is copied back to cover
        // the hole, and the vector length is restored to the new length.

        let len = self.len();
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.saturating_add(1),
        };
        let end = match range.end_bound() {
            Bound::Excluded(&j) => j,
            Bound::Included(&j) => j.saturating_add(1),
            Bound::Unbounded => len,
        };

        // set our length to start, to be safe in case Drain is leaked
        self.len = start;
        // Safety: ArrayVec API guarantees properly-initialized items within 0..len
        // Note: we do this unsafe slicing here because we also need to mutably borrow self (for backshifting the tail)
        let range_slice = unsafe { slice::from_raw_parts(self.as_ptr().add(start), end - start) };

        Drain {
            tail_start: end,
            tail_len: len - end,
            iter: range_slice.iter(),
            #[expect(clippy::ref_as_ptr, reason = "passing &mut self to a function would invalidate the slice iterator")]
            // Safety: We have a &mut to self, so creating a pointer from it is always safe.
            vec: unsafe { NonNull::new_unchecked(self as *mut _) },
        }
    }

    /// Shortens the vector, keeping the first `len` elements and dropping
    /// the rest
    pub fn truncate(&mut self, new_len: usize) {
        let len = self.len();
        if new_len < len {
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            // we have checked that new_len is less than len so all elements within 0..new_len must be
            // initialized
            unsafe {
                self.len = new_len;
                let tail = slice::from_raw_parts_mut(self.as_mut_ptr().add(new_len), len - new_len);
                ptr::drop_in_place(tail);
            }
        }
    }

    /// Returns the remaining spare capacity of the vector as a slice of
    /// `MaybeUninit<T>`.
    pub fn spare_capacity_mut(&mut self) -> &mut [MaybeUninit<T>] {
        let len = self.len();
        &mut self.data[len..]
    }

    /// Extend the `ArrayVec` with elements from the provided slice
    ///
    /// # Panics
    ///
    /// Panics if the `ArrayVec` does not have enough capacity to accommodate
    /// the elements.
    pub fn extend_from_slice(&mut self, slice: &[T])
    where
        T: Clone,
    {
        self.try_extend_from_slice(slice).unwrap();
    }

    /// Extend the `ArrayVec` with elements from the provided slice
    ///
    /// # Errors
    ///
    /// Returns a `CapacityError` if the `ArrayVec` does not have enough capacity to accommodate
    /// the elements.
    pub fn try_extend_from_slice(&mut self, other: &[T]) -> Result<(), CapacityError<()>>
    where
        T: Clone,
    {
        if self.remaining_capacity() < other.len() {
            return Err(CapacityError(()));
        }

        for (element, slot) in other.iter().cloned().zip(self.spare_capacity_mut()) {
            slot.write(element);
        }
        self.len += other.len();

        Ok(())
    }
}

impl<T, const CAP: usize> fmt::Debug for ArrayVec<T, CAP>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T, const CAP: usize> Clone for ArrayVec<T, CAP>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        self.iter().cloned().collect()
    }

    fn clone_from(&mut self, rhs: &Self) {
        // recursive case for the common prefix
        let prefix = cmp::min(self.len(), rhs.len());
        self[..prefix].clone_from_slice(&rhs[..prefix]);

        if prefix < self.len() {
            // rhs was shorter
            self.truncate(prefix);
        } else {
            let rhs_elems = &rhs[self.len()..];
            self.extend_from_slice(rhs_elems);
        }
    }
}

impl<T, const CAP: usize> Deref for ArrayVec<T, CAP> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T, const CAP: usize> DerefMut for ArrayVec<T, CAP> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<T, const CAP: usize> Hash for ArrayVec<T, CAP>
where
    T: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&**self, state);
    }
}

impl<T, const CAP: usize> PartialEq for ArrayVec<T, CAP>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T, const CAP: usize> PartialEq<[T]> for ArrayVec<T, CAP>
where
    T: PartialEq,
{
    fn eq(&self, other: &[T]) -> bool {
        **self == *other
    }
}

impl<T, const CAP: usize> Eq for ArrayVec<T, CAP> where T: Eq {}

impl<T, const CAP: usize> Borrow<[T]> for ArrayVec<T, CAP> {
    fn borrow(&self) -> &[T] {
        self
    }
}

impl<T, const CAP: usize> BorrowMut<[T]> for ArrayVec<T, CAP> {
    fn borrow_mut(&mut self) -> &mut [T] {
        self
    }
}

impl<T, const CAP: usize> AsRef<[T]> for ArrayVec<T, CAP> {
    fn as_ref(&self) -> &[T] {
        self
    }
}

impl<T, const CAP: usize> AsMut<[T]> for ArrayVec<T, CAP> {
    fn as_mut(&mut self) -> &mut [T] {
        self
    }
}

impl<T, const CAP: usize> PartialOrd for ArrayVec<T, CAP>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        (**self).partial_cmp(other)
    }

    fn lt(&self, other: &Self) -> bool {
        (**self).lt(other)
    }

    fn le(&self, other: &Self) -> bool {
        (**self).le(other)
    }

    fn ge(&self, other: &Self) -> bool {
        (**self).ge(other)
    }

    fn gt(&self, other: &Self) -> bool {
        (**self).gt(other)
    }
}

impl<T, const CAP: usize> Ord for ArrayVec<T, CAP>
where
    T: Ord,
{
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (**self).cmp(other)
    }
}

/// Create an `ArrayVec` from an iterator.
///
/// ***Panics*** if the number of elements in the iterator exceeds the arrayvec's capacity.
impl<T, const CAP: usize> FromIterator<T> for ArrayVec<T, CAP> {
    /// Create an `ArrayVec` from an iterator.
    ///
    /// ***Panics*** if the number of elements in the iterator exceeds the arrayvec's capacity.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut array = ArrayVec::new();
        for element in iter {
            array.push(element);
        }
        array
    }
}

impl<'a, T: 'a, const CAP: usize> IntoIterator for &'a ArrayVec<T, CAP> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: 'a, const CAP: usize> IntoIterator for &'a mut ArrayVec<T, CAP> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T, const CAP: usize> IntoIterator for ArrayVec<T, CAP> {
    type Item = T;
    type IntoIter = IntoIter<T, CAP>;
    fn into_iter(self) -> IntoIter<T, CAP> {
        IntoIter {
            index: 0,
            vec: self,
        }
    }
}

pub struct IntoIter<T, const CAP: usize> {
    index: usize,
    vec: ArrayVec<T, CAP>,
}

impl<T, const CAP: usize> Drop for IntoIter<T, CAP> {
    fn drop(&mut self) {
        let len = self.vec.len();
        self.vec.len = 0;
        for elt in &mut self.vec.data[self.index..len] {
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            unsafe { MaybeUninit::assume_init_drop(elt) };
        }
    }
}

impl<T, const CAP: usize> fmt::Debug for IntoIter<T, CAP>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_list().entries(&self.vec[self.index..]).finish()
    }
}

impl<T, const CAP: usize> Iterator for IntoIter<T, CAP> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.vec.is_empty() || self.index >= self.vec.len {
            None
        } else {
            let elt = mem::replace(&mut self.vec.data[self.index], MaybeUninit::uninit());
            self.index += 1;
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            Some(unsafe { MaybeUninit::assume_init(elt) })
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.vec.len(), Some(self.vec.len()))
    }
}

impl<T, const CAP: usize> DoubleEndedIterator for IntoIter<T, CAP> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.vec.is_empty() || self.index >= self.vec.len {
            None
        } else {
            let elt = mem::replace(&mut self.vec.data[self.vec.len - 1], MaybeUninit::uninit());
            self.vec.len -= 1;
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            Some(unsafe { MaybeUninit::assume_init(elt) })
        }
    }
}

impl<T, const CAP: usize> ExactSizeIterator for IntoIter<T, CAP> {}

/// A draining iterator for `ArrayVec`.
pub struct Drain<'a, T, const CAP: usize> {
    /// Index of tail to preserve
    tail_start: usize,
    /// Length of tail
    tail_len: usize,
    /// Current remaining range to remove
    iter: slice::Iter<'a, T>,
    vec: NonNull<ArrayVec<T, CAP>>,
}

impl<T, const CAP: usize> Iterator for Drain<'_, T, CAP> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|elt| {
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            unsafe { ptr::read(ptr::from_ref(elt)) }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T, const CAP: usize> DoubleEndedIterator for Drain<'_, T, CAP> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().map(|elt| {
            // Safety: ArrayVec API guarantees properly-initialized items within 0..len
            unsafe { ptr::read(ptr::from_ref(elt)) }
        })
    }
}

impl<T, const CAP: usize> ExactSizeIterator for Drain<'_, T, CAP> {}

impl<T, const CAP: usize> Drop for Drain<'_, T, CAP> {
    fn drop(&mut self) {
        /// Fill the drained range by backshifting the "tail" (elements after the drained range).
        ///
        /// We do this in a drop guard so that no matter what happens, even if T's drop panics
        /// we leave an ArrayVec without uninitialized holes behind.
        struct DropGuard<'r, 'a, T, const CAP: usize>(&'r mut Drain<'a, T, CAP>);

        impl<'r, 'a, T, const CAP: usize> Drop for DropGuard<'r, 'a, T, CAP> {
            fn drop(&mut self) {
                if self.0.tail_len > 0 {
                    // Safety: See ArrayVec::drain comment
                    unsafe {
                        let source_vec = self.0.vec.as_mut();

                        // memmove back untouched tail, update to new length
                        let start = source_vec.len();
                        let tail = self.0.tail_start;
                        if tail != start {
                            // as_mut_ptr creates a &mut, invalidating other pointers.
                            // This pattern avoids calling it with a pointer already present.
                            let ptr = source_vec.as_mut_ptr();
                            let src = ptr.add(tail);
                            let dst = ptr.add(start);
                            ptr::copy(src, dst, self.0.tail_len);
                        }
                        source_vec.len = start + self.0.tail_len;
                    }
                }
            }
        }

        let guard = DropGuard(self);

        // drain the iterator and drop its elements
        for _ in guard.0.by_ref() {}
        // while let Some(_) = guard.0.next() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_vec() {
        let vec: ArrayVec<i32, 10> = ArrayVec::new();
        assert_eq!(vec.len(), 0);
        assert!(vec.is_empty());
        assert_eq!(vec.capacity(), 10);
    }

    #[test]
    fn default_creates_empty_vec() {
        let vec: ArrayVec<i32, 10> = ArrayVec::default();
        assert_eq!(vec.len(), 0);
        assert!(vec.is_empty());
    }

    #[test]
    fn push_increases_length() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        assert_eq!(vec.len(), 1);
        vec.push(2);
        assert_eq!(vec.len(), 2);
    }

    #[test]
    #[should_panic]
    fn push_panics_when_full() {
        let mut vec: ArrayVec<i32, 2> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
    }

    #[test]
    fn try_push_succeeds_when_not_full() {
        let mut vec: ArrayVec<i32, 2> = ArrayVec::new();
        assert!(vec.try_push(1).is_ok());
        assert!(vec.try_push(2).is_ok());
    }

    #[test]
    fn try_push_fails_when_full() {
        let mut vec: ArrayVec<i32, 2> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        let result = vec.try_push(3);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().0, 3);
    }

    #[test]
    fn is_full_returns_true_when_at_capacity() {
        let mut vec: ArrayVec<i32, 2> = ArrayVec::new();
        assert!(!vec.is_full());
        vec.push(1);
        assert!(!vec.is_full());
        vec.push(2);
        assert!(vec.is_full());
    }

    #[test]
    fn as_slice_returns_valid_slice() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        let slice = vec.as_slice();
        assert_eq!(slice, &[1, 2, 3]);
    }

    #[test]
    fn as_mut_slice_allows_modification() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        let slice = vec.as_mut_slice();
        slice[0] = 10;
        assert_eq!(vec.as_slice(), &[10, 2]);
    }
    #[test]
    fn clear_removes_all_elements() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        vec.clear();
        assert_eq!(vec.len(), 0);
        assert!(vec.is_empty());
    }

    #[test]
    fn retain_keeps_matching_elements() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        vec.push(4);
        vec.retain(|x| *x % 2 == 0);
        assert_eq!(vec.as_slice(), &[2, 4]);
    }

    #[test]
    fn clone_from_updates_to_match_source() {
        let mut vec1: ArrayVec<i32, 10> = ArrayVec::new();
        vec1.push(1);
        vec1.push(2);
        let mut vec2: ArrayVec<i32, 10> = ArrayVec::new();
        vec2.push(3);
        vec2.push(4);
        vec2.push(5);
        vec2.clone_from(&vec1);
        assert_eq!(vec2.as_slice(), &[1, 2]);
    }

    #[test]
    fn try_extend_from_slice_succeeds_with_capacity() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        assert!(vec.try_extend_from_slice(&[2, 3]).is_ok());
        assert_eq!(vec.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn try_extend_from_slice_fails_without_capacity() {
        let mut vec: ArrayVec<i32, 3> = ArrayVec::new();
        vec.push(1);
        let result = vec.try_extend_from_slice(&[2, 3, 4]);
        assert!(result.is_err());
    }

    #[test]
    fn extend_from_slice_adds_elements() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        vec.extend_from_slice(&[2, 3, 4]);
        assert_eq!(vec.as_slice(), &[1, 2, 3, 4]);
    }

    #[test]
    #[should_panic]
    fn extend_from_slice_panics_when_insufficient_capacity() {
        let mut vec: ArrayVec<i32, 3> = ArrayVec::new();
        vec.push(1);
        vec.extend_from_slice(&[2, 3, 4]);
    }

    #[test]
    fn truncate_removes_trailing_elements() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        vec.push(4);
        vec.truncate(2);
        assert_eq!(vec.len(), 2);
        assert_eq!(vec.as_slice(), &[1, 2]);
    }

    #[test]
    fn drain_removes_elements_in_range() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3, 4, 5]);
        let drained: Vec<_> = vec.drain(1..3).collect();
        assert_eq!(drained, &[2, 3]);
        assert_eq!(vec.as_slice(), &[1, 4, 5]);
    }

    #[test]
    fn drain_calls_drop_on_remaining_elements() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        struct DropCounter;
        impl Drop for DropCounter {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        {
            let mut vec: ArrayVec<DropCounter, 5> = ArrayVec::new();
            vec.push(DropCounter);
            vec.push(DropCounter);
            vec.push(DropCounter);
            let mut drain = vec.drain(0..2);
            drain.next(); // consume one element
            // Drop drain without consuming all elements
            drop(drain);

            assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 2);
        }
        // All 3 elements should be dropped: 1 consumed, 1 remaining in drain, 1 in vec
        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn drain_restores_tail_on_early_drop() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3, 4, 5]);
        assert_eq!(vec.len(), 5);
        {
            let mut drain = vec.drain(1..3);
            drain.next(); // consume one element (2)
            // drain is dropped here without consuming all elements
        }
        // Tail should be restored: [1, 4, 5]
        assert_eq!(vec.as_slice(), &[1, 4, 5]);
    }

    #[test]
    fn drain_empty_range() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3]);
        assert_eq!(vec.len(), 3);
        let drained: Vec<_> = vec.drain(1..1).collect();
        assert_eq!(drained.len(), 0);
        assert_eq!(vec.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn drain_full_range() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(vec.len(), 4);
        let drained: Vec<_> = vec.drain(..).collect();
        assert_eq!(drained, &[1, 2, 3, 4]);
        assert_eq!(vec.len(), 0);
        assert!(vec.is_empty());
    }

    #[test]
    fn drain_double_ended() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3, 4, 5]);
        assert_eq!(vec.len(), 5);
        let mut drain = vec.drain(1..4);
        assert_eq!(drain.next(), Some(2));
        assert_eq!(drain.next_back(), Some(4));
        assert_eq!(drain.next(), Some(3));
        assert_eq!(drain.next(), None);
        drop(drain);
        assert_eq!(vec.as_slice(), &[1, 5]);
    }

    #[test]
    fn into_iter_consumes_all_elements() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3, 4, 5]);
        let collected: Vec<_> = vec.into_iter().collect();
        assert_eq!(collected, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn into_iter_empty_vec() {
        let vec: ArrayVec<i32, 10> = ArrayVec::new();
        let collected: Vec<_> = vec.into_iter().collect();
        assert_eq!(collected.len(), 0);
    }

    #[test]
    fn into_iter_double_ended() {
        let mut vec: ArrayVec<i32, 10> = ArrayVec::new();
        vec.extend_from_slice(&[1, 2, 3, 4, 5]);
        let mut iter = vec.into_iter();
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next_back(), Some(5));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next_back(), Some(4));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }

    #[test]
    fn into_iter_panic_in_drop() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        enum DropEl {
            Count,
            Panic,
        }

        impl Drop for DropEl {
            fn drop(&mut self) {
                match *self {
                    DropEl::Count => {
                        DROP_COUNT.fetch_add(1, Ordering::SeqCst);
                    }
                    DropEl::Panic => {
                        panic!("Oh no");
                    }
                }
            }
        }

        let mut vec: ArrayVec<DropEl, 5> = ArrayVec::new();
        vec.push(DropEl::Count);
        vec.push(DropEl::Panic);
        vec.push(DropEl::Count);

        let mut iter = vec.into_iter();
        iter.next();
        let _ = std::panic::catch_unwind(|| drop(iter));

        // Note we don't see enough drop counts, essentially every element after the panic is leaked
        // but at least we don't access uninitialized elements and trigger UB
        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drain_panic_in_drop() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        enum DropEl {
            Count,
            Panic,
        }

        impl Drop for DropEl {
            fn drop(&mut self) {
                match *self {
                    DropEl::Count => {
                        DROP_COUNT.fetch_add(1, Ordering::SeqCst);
                    }
                    DropEl::Panic => {
                        panic!("Oh no");
                    }
                }
            }
        }

        let mut vec: ArrayVec<DropEl, 5> = ArrayVec::new();
        vec.push(DropEl::Count);
        vec.push(DropEl::Panic);
        vec.push(DropEl::Count);

        let drain = vec.drain(1..2);
        assert_eq!(drain.len(), 1);
        let _ = std::panic::catch_unwind(|| drop(drain));
        assert_eq!(vec.len(), 2);
        drop(vec);
        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 2);
    }
}
