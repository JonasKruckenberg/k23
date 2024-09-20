use crate::runtime::guest_memory::GuestAllocator;
use core::alloc::{Allocator, Layout, LayoutError};
use core::ops::Range;
use core::ptr::NonNull;
use core::{hint, mem, ptr, slice};

/// A growable array allocated in the guests address space, with configurable alignment.
///
/// This type is just like a regular `Vec<T>` (in fact much of the code is taken straight from the
/// `alloc` crate) with two major exceptions:
///
/// # GuestAllocator
///
/// Data "owned" by generated code i.e. data that belongs to a [`Store`][crate::runtime::Store] is allocated through a Stores
/// [`GuestAllocator`] that manages a stores virtual memory etc. A [`AlignedVec`] is essentially the
/// guest-owned counterpart to `Vec` in the kernel.
///
/// # Alignment
///
/// Almost all guest allocations require a specific alignment: JIT compiled code must be page-aligned
/// to allow for proper mapping, the `Stack` must be aligned to 16 bytes (on riscv) because of the
/// calling convention. `GuestVec` (contrary to a regular `Vec` and the reason for the code duplication here)
/// allows you to specify a different alignment then what `mem::align_of::<T>()` would normally dictate.
/// There are just two rules:
/// 1. The alignment must be a power of two
/// 2. The alignment must be greater or equal to the alignment of `T`
pub struct AlignedVec<T, const ALIGN: usize> {
    ptr: NonNull<T>,
    cap: usize,
    len: usize,
    alloc: GuestAllocator,
}

impl<T, const ALIGN: usize> AlignedVec<T, ALIGN> {
    const ALIGN_CHECK: bool = ALIGN.is_power_of_two() && ALIGN >= mem::align_of::<T>();
    const MIN_NON_ZERO_CAP: usize = if mem::size_of::<T>() == 1 {
        8
    } else if mem::size_of::<T>() <= 1024 {
        4
    } else {
        1
    };

    pub fn new(alloc: GuestAllocator) -> Self {
        assert!(Self::ALIGN_CHECK);

        let cap = if mem::size_of::<T>() == 0 {
            usize::MAX
        } else {
            0
        };

        unsafe {
            Self {
                // Safety: ALIGN can never be zero
                ptr: NonNull::new_unchecked(ALIGN as _),
                cap,
                len: 0,
                alloc,
            }
        }
    }

    pub fn try_with_capacity(cap: usize, alloc: GuestAllocator) -> Result<Self, ()> {
        assert!(Self::ALIGN_CHECK);

        if cap == 0 || mem::size_of::<T>() == 0 {
            unsafe {
                Ok(Self {
                    // Safety: ALIGN can never be zero
                    ptr: NonNull::new_unchecked(ALIGN as _),
                    cap,
                    len: 0,
                    alloc,
                })
            }
        } else {
            Self::try_allocate(cap, alloc)
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[allow(clippy::unused_self)]
    pub fn alignment(&self) -> usize {
        ALIGN
    }

    pub fn allocator(&self) -> &GuestAllocator {
        &self.alloc
    }

    pub fn as_slice(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }

    pub fn as_ptr_range(&self) -> Range<*const T> {
        self.as_slice().as_ptr_range()
    }

    pub fn as_mut_ptr_range(&mut self) -> Range<*mut T> {
        self.as_mut_slice().as_mut_ptr_range()
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), ()> {
        if additional > self.capacity().wrapping_sub(self.len) {
            unsafe { self.try_grow_amortized(self.len, additional) }
        } else {
            Ok(())
        }
    }

    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        if additional > self.capacity().wrapping_sub(self.len) {
            unsafe { self.try_grow_exact(self.len, additional) }
        } else {
            Ok(())
        }
    }

    pub fn try_resize(&mut self, new_len: usize, value: T) -> Result<(), ()>
    where
        T: Clone,
    {
        let len = self.len();

        if new_len > len {
            self.try_extend_with(new_len - len, value)?;
        } else {
            self.truncate(new_len);
        }

        Ok(())
    }

    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            let old_len = self.len;
            self.len = len;
            unsafe {
                let ptr = self.as_mut_ptr();
                ptr::slice_from_raw_parts_mut(ptr.add(len), old_len - len).drop_in_place();
            }
        }
    }

    pub fn try_extend_from_slice(&mut self, other: &[T]) -> Result<(), ()> {
        let count = other.len();
        self.try_reserve(count)?;
        let len = self.len();
        unsafe { ptr::copy_nonoverlapping(other.as_ptr(), self.as_mut_ptr().add(len), count) };
        self.len += count;

        Ok(())
    }

    fn try_extend_with(&mut self, n: usize, value: T) -> Result<(), ()>
    where
        T: Clone,
    {
        self.try_reserve(n)?;

        unsafe {
            let mut ptr = self.as_mut_ptr().add(self.len());

            // Write all elements except the last one
            for _ in 1..n {
                ptr::write(ptr, value.clone());
                ptr = ptr.add(1);
                // Increment the length in every step in case clone() panics
                self.len += 1;
            }

            if n > 0 {
                // We can write the last element directly without cloning needlessly
                ptr::write(ptr, value);
                self.len += 1;
            }
        }

        Ok(())
    }

    unsafe fn try_grow_amortized(&mut self, len: usize, additional: usize) -> Result<(), ()> {
        debug_assert!(additional > 0);
        if self.cap == 0 {
            *self = Self::try_with_capacity(
                additional.max(Self::MIN_NON_ZERO_CAP),
                self.alloc.clone(),
            )?;
            return Ok(());
        }

        if mem::size_of::<T>() == 0 {
            debug_assert_eq!(self.cap, usize::MAX);
            return Err(());
        }

        let Some(new_cap) = len.checked_add(additional) else {
            return Err(());
        };

        // self.cap * 2 can't overflow because it's less than isize::MAX
        let new_cap = new_cap.max(self.cap * 2);
        let new_cap = new_cap.max(Self::MIN_NON_ZERO_CAP);

        let new_layout = Layout::from_size_align(new_cap, ALIGN);

        let ptr = try_grow_unchecked(new_layout, self.current_memory(), &mut self.alloc)?;

        self.set_ptr_and_cap(ptr, new_cap);
        Ok(())
    }

    unsafe fn try_grow_exact(&mut self, len: usize, additional: usize) -> Result<(), ()> {
        debug_assert!(additional > 0);
        if mem::size_of::<T>() == 0 {
            debug_assert_eq!(self.cap, usize::MAX);
            return Err(());
        }

        if self.cap == 0 {
            *self = Self::try_with_capacity(additional, self.alloc.clone())?;
            return Ok(());
        }

        let new_cap = len.checked_add(additional).ok_or(())?;
        let new_layout = Layout::from_size_align(new_cap, ALIGN);

        let ptr = try_grow_unchecked(new_layout, self.current_memory(), &mut self.alloc)?;

        self.set_ptr_and_cap(ptr, new_cap);
        Ok(())
    }

    fn current_memory(&self) -> Option<(NonNull<u8>, Layout)> {
        if mem::size_of::<T>() == 0 || self.cap == 0 {
            None
        } else {
            // We could use Layout::array here which ensures the absence of isize and usize overflows
            // and could hypothetically handle differences between stride and size, but this memory
            // has already been allocated so we know it can't overflow and currently Rust does not
            // support such types. So we can do better by skipping some checks and avoid an unwrap.
            assert_eq!(mem::size_of::<T>() % mem::align_of::<T>(), 0);
            unsafe {
                let size = mem::size_of::<T>().unchecked_mul(self.cap);
                let layout = Layout::from_size_align_unchecked(size, ALIGN);
                Some((self.ptr.cast(), layout))
            }
        }
    }

    #[inline]
    fn set_ptr_and_cap(&mut self, ptr: NonNull<[u8]>, cap: usize) {
        // Allocators currently return a `NonNull<[u8]>` whose length matches
        // the size requested. If that ever changes, the capacity here should
        // change to `ptr.len() / mem::size_of::<T>()`.
        self.ptr = unsafe { NonNull::new_unchecked(ptr.cast().as_ptr()) };
        self.cap = cap;
    }

    fn try_allocate(cap: usize, alloc: GuestAllocator) -> Result<Self, ()> {
        // We avoid `unwrap_or_else` here because it bloats the amount of
        // LLVM IR generated.
        let Ok(layout) = Layout::from_size_align(cap, ALIGN) else {
            return Err(());
        };

        alloc_guard(layout.size())?;

        let result = alloc.allocate_zeroed(layout);

        let Ok(ptr) = result else {
            return Err(());
        };

        Ok(Self {
            ptr: unsafe { NonNull::new_unchecked(ptr.cast().as_ptr()) },
            cap,
            alloc,
            len: 0,
        })
    }
}

unsafe impl<T: Sync, const ALIGN: usize> Sync for AlignedVec<T, ALIGN> {}
unsafe impl<T: Send, const ALIGN: usize> Send for AlignedVec<T, ALIGN> {}

impl<T, const ALIGN: usize> Drop for AlignedVec<T, ALIGN> {
    #[inline]
    fn drop(&mut self) {
        if let Some((ptr, layout)) = self.current_memory() {
            unsafe { self.alloc.deallocate(ptr, layout) }
        }
    }
}

// We need to guarantee the following:
// * We don't ever allocate `> isize::MAX` byte-size objects.
// * We don't overflow `usize::MAX` and actually allocate too little.
//
// On 64-bit we just need to check for overflow since trying to allocate
// `> isize::MAX` bytes will surely fail. On 32-bit and 16-bit we need to add
// an extra guard for this in case we're running on a platform which can use
// all 4GB in user-space, e.g., PAE or x32.
#[inline]
fn alloc_guard(alloc_size: usize) -> Result<(), ()> {
    if usize::BITS < 64 && alloc_size > isize::MAX as usize {
        Err(())
    } else {
        Ok(())
    }
}

#[inline(never)]
fn try_grow_unchecked(
    new_layout: Result<Layout, LayoutError>,
    current_memory: Option<(NonNull<u8>, Layout)>,
    alloc: &mut GuestAllocator,
) -> Result<NonNull<[u8]>, ()> {
    // Check for the error here to minimize the size of `RawVec::grow_*`.
    let new_layout = new_layout.map_err(|_| ())?;

    alloc_guard(new_layout.size())?;

    let memory = if let Some((ptr, old_layout)) = current_memory {
        debug_assert_eq!(old_layout.align(), new_layout.align());
        unsafe {
            // The allocator checks for alignment equality
            hint::assert_unchecked(old_layout.align() == new_layout.align());
            alloc.grow(ptr, old_layout, new_layout)
        }
    } else {
        alloc.allocate(new_layout)
    };

    memory.map_err(|_| ())
}

impl<const ALIGN: usize> object::write::WritableBuffer for AlignedVec<u8, ALIGN> {
    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn reserve(&mut self, size: usize) -> Result<(), ()> {
        debug_assert!(self.is_empty());
        self.try_reserve(size)
    }

    #[inline]
    fn resize(&mut self, new_len: usize) {
        debug_assert!(new_len >= self.len());
        self.try_resize(new_len, 0).unwrap();
    }

    #[inline]
    fn write_bytes(&mut self, val: &[u8]) {
        debug_assert!(self.len() + val.len() <= self.capacity());
        self.try_extend_from_slice(val).unwrap();
    }
}
