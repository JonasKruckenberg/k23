use crate::frame_alloc::{FrameAllocator, FrameUsage};
use crate::{arch, AddressRangeExt, PhysicalAddress, VirtualAddress};
use core::alloc::Layout;
use core::marker::PhantomData;
use core::mem::{offset_of, MaybeUninit};
use core::ops::{Deref, Range};
use core::pin::Pin;
use core::ptr::NonNull;
use core::{array, cmp, fmt, ptr};

const DEFAULT_MAX_ORDER: usize = 11;

pub struct BuddyAllocator<const MAX_ORDER: usize = DEFAULT_MAX_ORDER> {
    free_lists: [linked_list::List<FreeArea>; MAX_ORDER],
    phys_offset: VirtualAddress,
    max_order: usize,
    used: usize,
    total: usize,
}

impl<const MAX_ORDER: usize> Default for BuddyAllocator<MAX_ORDER> {
    fn default() -> Self {
        Self {
            free_lists: array::from_fn(|_| linked_list::List::new()),
            phys_offset: Default::default(),
            max_order: 0,
            used: 0,
            total: 0,
        }
    }
}

impl<const MAX_ORDER: usize> fmt::Debug for BuddyAllocator<MAX_ORDER> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuddyAllocator")
            .field_with("free_lists", |f| {
                let mut f = f.debug_map();
                for (order, l) in self.free_lists.iter().enumerate() {
                    f.key(&order);
                    f.value_with(|f| f.debug_list().entries(l.iter()).finish());
                }
                f.finish()
            })
            .field("phys_offset", &self.phys_offset)
            .field("max_order", &self.max_order)
            .finish()
    }
}

impl<const MAX_ORDER: usize> BuddyAllocator<MAX_ORDER> {
    /// Create a new **empty** buddy allocator.
    ///
    /// Physical memory to manage must be added with `add_range`.
    pub fn new(phys_offset: VirtualAddress) -> Self {
        Self {
            free_lists: array::from_fn(|_| linked_list::List::new()),
            phys_offset,
            max_order: 0,
            used: 0,
            total: 0,
        }
    }

    /// Create a buddy allocator from an iterator of physical memory ranges.
    ///
    /// # Safety
    ///
    /// The caller has to ensure that the ranges are valid physical memory and not already "owned" by other parts of the system.
    pub unsafe fn from_iter<I: IntoIterator<Item = Range<PhysicalAddress>>>(
        iter: I,
        phys_offset: VirtualAddress,
    ) -> Self {
        let mut alloc = BuddyAllocator::new(phys_offset);
        for range in iter {
            alloc.add_range(range)
        }
        alloc
    }

    /// Add a range of physical memory to the buddy allocator.
    ///
    /// # Safety
    ///
    /// The range must be valid physical memory and not already managed by other parts of the system
    /// or the allocator itself.
    pub unsafe fn add_range(&mut self, range: Range<PhysicalAddress>) {
        let aligned = range.align_in(arch::PAGE_SIZE);
        let mut remaining_bytes = aligned.size();
        let mut addr = aligned.start;

        while remaining_bytes > 0 {
            let lowbit = addr.as_raw() & (!addr.as_raw() + 1);

            let size = cmp::min(
                cmp::min(lowbit, prev_power_of_two(remaining_bytes)),
                arch::PAGE_SIZE << (MAX_ORDER - 1),
            );

            let size_pages = size / arch::PAGE_SIZE;
            let order = size_pages.trailing_zeros() as usize;
            self.total += size / arch::PAGE_SIZE;
            self.max_order = cmp::max(self.max_order, order);

            let area = FreeArea::from_addr(addr, self.phys_offset);
            self.free_lists[order].push_back(area);

            addr = addr.add(size);
            remaining_bytes -= size;
        }
    }

    pub fn largest_alignment(&self) -> usize {
        arch::PAGE_SIZE << self.max_order
    }

    fn allocate_inner(&mut self, size: usize) -> Option<PhysicalAddress> {
        let size_pages = size / arch::PAGE_SIZE;
        let order = size_pages.trailing_zeros() as usize;

        for i in order..(self.max_order + 1) {
            if let Some(free_area) = self.free_lists[i].pop_back() {
                for j in (order + 1..i + 1).rev() {
                    // Insert the "upper half" of the block into the free list.
                    let buddy_addr = FreeArea::into_addr(free_area, self.phys_offset)
                        .add(arch::PAGE_SIZE << (j - 1));
                    let buddy = unsafe { FreeArea::from_addr(buddy_addr, self.phys_offset) };
                    self.free_lists[j - 1].push_back(buddy);
                }

                self.used += size_pages;
                return Some(FreeArea::into_addr(free_area, self.phys_offset));
            }
        }

        None
    }

    fn deallocate_inner(&mut self, addr: PhysicalAddress, size: usize) {
        let size_pages = size / arch::PAGE_SIZE;
        let order = size_pages.trailing_zeros() as usize;

        let mut ptr = addr;
        'outer: for order in order..self.max_order {
            let buddy = VirtualAddress::from_phys(ptr, self.phys_offset).as_raw()
                ^ (arch::PAGE_SIZE << order);

            let mut c = self.free_lists[order].cursor_front_mut();
            while let Some(area) = c.get() {
                let addr = &raw const *area as usize;
                if addr == buddy {
                    c.remove();
                    ptr = cmp::min(ptr, PhysicalAddress::new(buddy - self.phys_offset.as_raw()));
                    continue 'outer;
                }
                c.move_next();
            }

            // TODO this should not use saturating_sub as it ignores double-freeing
            self.used = self.used.saturating_sub(size_pages);

            self.free_lists[order].push_back(unsafe { FreeArea::from_addr(ptr, self.phys_offset) });
            return;
        }

        // panic!("deallocating memory that was not allocated by the buddy allocator");
        log::error!("deallocating memory that was not allocated by the buddy allocator");
    }
}

impl<const MAX_ORDER: usize> FrameAllocator for BuddyAllocator<MAX_ORDER> {
    fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        assert!(layout.align() >= arch::PAGE_SIZE);
        assert!(layout.size() >= arch::PAGE_SIZE);

        let size = cmp::max(layout.size().next_power_of_two(), layout.align());
        self.allocate_inner(size)
    }

    fn deallocate_contiguous(&mut self, addr: PhysicalAddress, layout: Layout) {
        assert!(layout.align() >= arch::PAGE_SIZE);
        assert!(layout.size() >= arch::PAGE_SIZE);

        let size = cmp::max(layout.size().next_power_of_two(), layout.align());
        self.deallocate_inner(addr, size)
    }

    fn allocate_contiguous_zeroed(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        let addr = self.allocate_contiguous(layout)?;
        unsafe {
            ptr::write_bytes(
                self.phys_offset.add(addr.as_raw()).as_raw() as *mut u8,
                0,
                layout.size(),
            )
        }
        Some(addr)
    }

    fn frame_usage(&self) -> FrameUsage {
        FrameUsage {
            used: self.used,
            total: self.total,
        }
    }

    fn allocate_partial(&mut self, layout: Layout) -> Option<(PhysicalAddress, usize)> {
        let size = cmp::min(
            cmp::max(layout.align(), prev_power_of_two(layout.size())),
            arch::PAGE_SIZE << self.max_order,
        );
        debug_assert!(size.is_power_of_two());

        // if the block size we picked is less that the alignment, this means we can't satisfy the alignment
        if size < layout.align() {
            return None;
        }

        let order = (size / arch::PAGE_SIZE).trailing_zeros() as usize;

        // log::trace!(
        //     "align: {} remaining_pot: {} max: {} == {size} (order {order})",
        //     layout.align(),
        //     prev_power_of_two(layout.size()),
        //     arch::PAGE_SIZE << (self.max_order - 1)
        // );

        for i in order..(self.max_order + 1) {
            if let Some(free_area) = self.free_lists[i].pop_back() {
                for j in (order + 1..i + 1).rev() {
                    // Insert the "upper half" of the block into the free list.
                    let buddy_addr = FreeArea::into_addr(free_area, self.phys_offset)
                        .add(arch::PAGE_SIZE << (j - 1));
                    let buddy = unsafe { FreeArea::from_addr(buddy_addr, self.phys_offset) };
                    self.free_lists[j - 1].push_back(buddy);
                }

                if size > layout.size() {
                    log::trace!(
                        "overaligned allocation: {} > {}, wasting {} bytes in the process",
                        size,
                        layout.size(),
                        size - layout.size()
                    );
                }

                let alloc_size = cmp::min(size, layout.size());
                self.used += alloc_size / arch::PAGE_SIZE;

                let addr = FreeArea::into_addr(free_area, self.phys_offset);
                debug_assert!(
                    addr.is_aligned(layout.align()),
                    "addr {addr:?} is not correctly aligned (align {})",
                    layout.align()
                );
                return Some((addr, alloc_size));
            }
        }

        None
    }
}

pub const FREE_AREA_MAGIC: u32 = u32::from_le_bytes(*b"bddy");

#[repr(C)]
struct FreeArea {
    _magic: u32,
    links: linked_list::Links<FreeArea>,
}

impl fmt::Debug for FreeArea {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FreeArea")
            .field("self", &format_args!("{self:p}"))
            .finish()
    }
}

impl FreeArea {
    pub unsafe fn from_addr(
        addr: PhysicalAddress,
        phys_offset: VirtualAddress,
    ) -> Pin<Unique<Self>> {
        let ptr = &mut *(phys_offset.add(addr.as_raw()).as_raw() as *mut MaybeUninit<Self>);

        let this = if *ptr.as_ptr().cast::<u32>() == FREE_AREA_MAGIC {
            ptr.assume_init_mut()
        } else {
            ptr.write(FreeArea {
                _magic: FREE_AREA_MAGIC,
                links: Default::default(),
            })
        };

        Unique::from(this).into_pin()
    }

    pub fn into_addr(this: Pin<Unique<Self>>, phys_offset: VirtualAddress) -> PhysicalAddress {
        let raw = unsafe { Pin::into_inner_unchecked(this).as_ptr() as usize };

        PhysicalAddress::new(raw - phys_offset.as_raw())
    }
}

unsafe impl linked_list::Linked for FreeArea {
    type Handle = Pin<Unique<Self>>;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { Unique::into_non_null(Pin::into_inner_unchecked(handle)) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the list implementation upholds.
        Pin::new_unchecked(Unique::new_unchecked(ptr.as_ptr()))
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}

struct Unique<T: ?Sized> {
    ptr: NonNull<T>,
    _marker_owning: PhantomData<T>,
}
unsafe impl<T> Send for Unique<T> where T: Send + ?Sized {}
unsafe impl<T> Sync for Unique<T> where T: Sync + ?Sized {}
impl<T: ?Sized> Unique<T> {
    /// Creates a new `Unique`.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null.
    #[inline]
    pub const unsafe fn new_unchecked(ptr: *mut T) -> Self {
        // SAFETY: the caller must guarantee that `ptr` is non-null.
        unsafe {
            Unique {
                ptr: NonNull::new_unchecked(ptr),
                _marker_owning: PhantomData,
            }
        }
    }

    /// Acquires the underlying `*mut` pointer.
    #[must_use = "`self` will be dropped if the result is not used"]
    #[inline]
    pub const fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    #[must_use = "losing the pointer will leak memory"]
    #[inline]
    pub fn into_non_null(b: Unique<T>) -> NonNull<T> {
        b.ptr
    }

    pub const fn into_pin(self) -> Pin<Self> {
        // It's not possible to move or replace the insides of a `Pin<Unique<T>>`
        // when `T: !Unpin`, so it's safe to pin it directly without any
        // additional requirements.
        unsafe { Pin::new_unchecked(self) }
    }
}

impl<T: ?Sized> Clone for Unique<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Unique<T> {}

impl<T: ?Sized> fmt::Debug for Unique<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.as_ptr(), f)
    }
}

impl<T: ?Sized> fmt::Pointer for Unique<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.as_ptr(), f)
    }
}

impl<T: ?Sized> From<&mut T> for Unique<T> {
    /// Converts a `&mut T` to a `Unique<T>`.
    ///
    /// This conversion is infallible since references cannot be null.
    #[inline]
    fn from(reference: &mut T) -> Self {
        Self::from(NonNull::from(reference))
    }
}

impl<T: ?Sized> From<NonNull<T>> for Unique<T> {
    /// Converts a `NonNull<T>` to a `Unique<T>`.
    ///
    /// This conversion is infallible since `NonNull` cannot be null.
    #[inline]
    fn from(ptr: NonNull<T>) -> Self {
        Unique {
            ptr,
            _marker_owning: PhantomData,
        }
    }
}

impl<T: ?Sized> Deref for Unique<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }
}

fn prev_power_of_two(num: usize) -> usize {
    1 << (usize::BITS as usize - num.leading_zeros() as usize - 1)
}

// #[cfg(test)]
// mod tests {
//     use core::slice;
//
//     #[test]
//     fn init() {
//         let mut alloc = crate::frame_alloc::BuddyAllocator::<19>::new(boot_info.physical_memory_offset);
//
//         for region in unsafe { slice::from_raw_parts(boot_info.memory_regions, boot_info.memory_regions_len) } {
//             if region.kind.is_usable() {
//                 log::trace!("adding memory region: {:?}", region);
//                 unsafe { alloc.add_range(region.range.clone()); }
//             }
//         }
//
//         log::trace!("allocator is initialized {alloc:#?}");
//     }
// }
