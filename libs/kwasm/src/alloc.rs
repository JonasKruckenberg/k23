// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ptr::NonNull;
use core::range::Range;
use core::{cmp, hint};

use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Permissions: u8 {
        /// Allow reads from the memory region
        const READ = 1 << 0;
        /// Allow writes to the memory region
        const WRITE = 1 << 1;
        /// Allow code execution from the memory region
        const EXECUTE = 1 << 2;
        /// TODO
        const BRANCH_PREDICTION = 1 << 3;
    }
}

unsafe trait AddressSpace {
    fn map(&self, layout: Layout) -> crate::Result<NonNull<[u8]>>;
    fn map_zeroed(&self, layout: Layout) -> crate::Result<NonNull<[u8]>>;

    /// # Safety
    /// - ptr must denote a block of memory currently allocated via this allocator, and
    /// - layout must fit that block of memory.
    unsafe fn unmap(&self, ptr: NonNull<u8>, layout: Layout);

    fn protect(&self, range: Range<NonNull<u8>>, permissions: Permissions) -> crate::Result<()>;

    fn prefetch_read(&self, ptr: NonNull<u8>, layout: Layout) -> crate::Result<()>;

    fn prefetch_write(&self, ptr: NonNull<u8>, layout: Layout) -> crate::Result<()>;

    /// # Safety
    /// - ptr must denote a block of memory currently allocated via this allocator.
    /// - old_layout must fit that block of memory (The new_layout argument need not fit it.).
    /// - new_layout.size() must be greater than or equal to old_layout.size().
    /// Note that new_layout.align() need not be the same as old_layout.align().
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>>;

    /// # Safety
    /// - ptr must denote a block of memory currently allocated via this allocator.
    /// - old_layout must fit that block of memory (The new_layout argument need not fit it.).
    /// - new_layout.size() must be smaller than or equal to old_layout.size().
    /// Note that new_layout.align() need not be the same as old_layout.align().
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>>;
}

// struct Mmap<A> {
//     ptr: NonNull<u8>,
//     cap: usize,
//     aspace: A,
// }
//
// impl<A> Mmap<A>
// where
//     A: AddressSpace,
// {
//     pub fn with_reserve(reserve: usize, aspace: A) -> crate::Result<Mmap<A>> {
//         let layout = Layout::array::<u8>(reserve).unwrap();
//
//         let ptr = aspace.map(layout)?;
//
//         Ok(Self {
//             ptr: ptr.cast(),
//             cap: ptr.len(),
//             aspace,
//         })
//     }
//
//     pub fn with_reserve_zeroed(reserve: usize, aspace: A) -> crate::Result<Mmap<A>> {
//         let layout = Layout::array::<u8>(reserve).unwrap();
//
//         let ptr = aspace.map_zeroed(layout)?;
//
//         Ok(Self {
//             ptr: ptr.cast(),
//             cap: ptr.len(),
//             aspace,
//         })
//     }
//
//     #[inline]
//     #[track_caller]
//     fn reserve(&mut self, len: usize, additional: usize) -> crate::Result<()> {
//         // Callers expect this function to be very cheap when there is already sufficient capacity.
//         // Therefore, we move all the resizing and error-handling logic from grow_amortized and
//         // handle_reserve behind a call, while making sure that this function is likely to be
//         // inlined as just a comparison and a call if the comparison fails.
//         #[cold]
//         fn do_reserve_and_handle<A: AddressSpace>(
//             slf: &mut Mmap<A>,
//             len: usize,
//             additional: usize,
//         ) -> crate::Result<()> {
//             slf.grow_amortized(len, additional)
//         }
//
//         if self.needs_to_grow(len, additional) {
//             do_reserve_and_handle(self, len, additional)?;
//         }
//
//         Ok(())
//     }
//
//     #[inline]
//     fn needs_to_grow(&self, len: usize, additional: usize) -> bool {
//         additional > self.cap.wrapping_sub(len)
//     }
//
//     fn grow_amortized(&mut self, len: usize, additional: usize) -> crate::Result<()> {
//         // This is ensured by the calling contexts.
//         debug_assert!(additional > 0);
//
//         // Nothing we can really do about these checks, sadly.
//         let required_cap = len
//             .checked_add(additional)
//             .ok_or(anyhow::anyhow!("capacity overflow"))?;
//
//         // This guarantees exponential growth. The doubling cannot overflow
//         // because `cap <= isize::MAX` and the type of `cap` is `usize`.
//         let cap = cmp::max(self.cap * 2, required_cap);
//         let cap = cmp::max(8, cap);
//
//         let new_layout = Layout::array::<u8>(cap)?;
//
//         let ptr = finish_grow(
//             new_layout,
//             self.current_memory(),
//             &mut self.aspace,
//         )?;
//         // SAFETY: finish_grow would have resulted in a capacity overflow if we tried to allocate more than `isize::MAX` items
//
//         self.ptr = ptr.cast();
//         self.cap = cap;
//
//         Ok(())
//     }
//
//     #[inline]
//     fn current_memory(&self) -> Option<(NonNull<u8>, Layout)> {
//         if self.cap == 0 {
//             None
//         } else {
//             unsafe {
//                 let layout = Layout::from_size_align_unchecked(self.cap, 1);
//                 Some((self.ptr.into(), layout))
//             }
//         }
//     }
// }
//
// #[cold]
// fn finish_grow<A>(
//     new_layout: Layout,
//     current_memory: Option<(NonNull<u8>, Layout)>,
//     alloc: &mut A,
// ) -> crate::Result<NonNull<[u8]>>
// where
//     A: AddressSpace,
// {
//     if let Some((ptr, old_layout)) = current_memory {
//         debug_assert_eq!(old_layout.align(), new_layout.align());
//         unsafe {
//             // The allocator checks for alignment equality
//             hint::assert_unchecked(old_layout.align() == new_layout.align());
//             alloc.grow(ptr, old_layout, new_layout)
//         }
//     } else {
//         alloc.map(new_layout)
//     }
// }
//
// const fn min_non_zero_cap(size: usize) -> usize {
//     if size == 1 {
//         8
//     } else if size <= 1024 {
//         4
//     } else {
//         1
//     }
// }
