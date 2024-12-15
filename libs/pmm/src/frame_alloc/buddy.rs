use crate::arch;
use crate::{PhysicalAddress, VirtualAddress};
use core::alloc::Layout;
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::mem::{offset_of, MaybeUninit};
use core::ops::{Deref, Div, Range};
use core::pin::Pin;
use core::ptr::NonNull;
use core::{array, cmp, fmt};

pub struct BuddyAllocator<const MAX_ORDER: usize = 11> {
    free_lists: [linked_list::List<FreeArea>; MAX_ORDER],
    phys_offset: VirtualAddress,
    max_order: usize,
}

impl<const MAX_ORDER: usize> fmt::Debug for BuddyAllocator<MAX_ORDER> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
    pub fn new(phys_offset: VirtualAddress) -> Self {
        Self {
            free_lists: array::from_fn(|_| linked_list::List::new()),
            phys_offset,
            max_order: 0,
        }
    }

    /// Add a range of physical memory to the buddy allocator.
    ///
    /// # Safety
    ///
    /// The range must be valid physical memory and not already "owned" by other parts of the system.
    pub unsafe fn add_range(&mut self, range: Range<PhysicalAddress>) {
        let mut remaining_bytes = range
            .end
            .align_up(arch::PAGE_SIZE)
            .sub_addr(range.start.align_down(arch::PAGE_SIZE));
        let mut addr = range.start;

        while remaining_bytes > 0 {
            let lowbit = addr.as_raw() & (!addr.as_raw() + 1);

            let size = cmp::min(
                cmp::min(lowbit, prev_power_of_two(remaining_bytes)),
                arch::PAGE_SIZE << (MAX_ORDER - 1),
            );
            let order = size.div(arch::PAGE_SIZE).trailing_zeros() as usize;

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

    pub fn allocate(&mut self, layout: Layout) -> Option<PhysicalAddress> {
        assert!(layout.align() >= arch::PAGE_SIZE);
        let size = cmp::max(layout.size().next_power_of_two(), layout.align());
        self.alloc_power_of_two(size)
    }

    fn alloc_power_of_two(&mut self, size: usize) -> Option<PhysicalAddress> {
        let order = size.div(arch::PAGE_SIZE).trailing_zeros() as usize;

        for i in order..(self.max_order + 1) {
            if let Some(free_area) = self.free_lists[i].pop_back() {
                for j in (order + 1..i + 1).rev() {
                    // Insert the "upper half" of the block into the free list.
                    let buddy_addr = FreeArea::into_addr(free_area, self.phys_offset)
                        .add(arch::PAGE_SIZE << (j - 1));
                    let buddy = unsafe { FreeArea::from_addr(buddy_addr, self.phys_offset) };
                    self.free_lists[j - 1].push_back(buddy);
                }

                return Some(FreeArea::into_addr(free_area, self.phys_offset));
            }
        }

        None
    }

    pub fn deallocate(&mut self, addr: PhysicalAddress, layout: Layout) {
        assert!(layout.align() >= arch::PAGE_SIZE);
        let size = cmp::max(layout.size().next_power_of_two(), layout.align());
        self.dealloc_power_of_two(addr, size)
    }

    fn dealloc_power_of_two(&mut self, addr: PhysicalAddress, size: usize) {
        let order = size.div(arch::PAGE_SIZE).trailing_zeros() as usize;

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

            self.free_lists[order].push_back(unsafe { FreeArea::from_addr(ptr, self.phys_offset) });
            return;
        }

        panic!()
    }
}

// impl<const MAX_ORDER: usize> FrameAllocator for BuddyAllocator<MAX_ORDER> {
//     fn allocate_contiguous(&mut self, frames: NonZeroUsize) -> crate::Result<(PhysicalAddress, NonZeroUsize)> {
//         todo!()
//     }
//
//     fn deallocate(&mut self, base: PhysicalAddress, frames: NonZeroUsize) -> crate::Result<()> {
//         todo!()
//     }
//
//     fn frame_usage(&self) -> FrameUsage {
//         todo!()
//     }
// }

#[derive(Default)]
struct FreeArea {
    links: linked_list::Links<FreeArea>,
}

impl fmt::Debug for FreeArea {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
        let this = ptr.write(FreeArea::default());

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
