use core::fmt;
use core::marker::PhantomData;
use core::mem::offset_of;
use core::ops::Deref;
use core::pin::Pin;
use core::ptr::NonNull;
use mmu::PhysicalAddress;
use pin_project_lite::pin_project;

#[derive(Debug)]
pub enum FrameState {
    Free,
    Allocated,
}

pin_project! {
    pub struct Frame {
        pub links: linked_list::Links<Frame>,
        // The physical address of the frame
        pub phys: PhysicalAddress,
        pub state: FrameState,
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("phys", &self.phys)
            .field("state", &self.state)
            .finish()
    }
}

unsafe impl linked_list::Linked for Frame {
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

pub struct Unique<T: ?Sized> {
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
