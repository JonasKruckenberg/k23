use crate::vm::{AddressSpace, UserMmap};
use crate::wasm::Error;
use core::marker::PhantomData;
use core::ops::Deref;
use core::range::Range;
use core::slice;

#[derive(Debug)]
pub struct MmapVec<T> {
    mmap: UserMmap,
    len: usize,
    _m: PhantomData<T>,
}

impl<T> MmapVec<T> {
    pub fn new_empty() -> Self {
        Self {
            mmap: UserMmap::new_empty(),
            len: 0,
            _m: PhantomData,
        }
    }

    pub fn new_zeroed(aspace: &mut AddressSpace, len: usize) -> crate::wasm::Result<Self> {
        Ok(Self {
            mmap: UserMmap::new_zeroed(aspace, len).map_err(|_| Error::MmapFailed)?,
            len: 0,
            _m: PhantomData,
        })
    }

    pub fn from_slice(aspace: &mut AddressSpace, slice: &[T]) -> crate::wasm::Result<Self>
    where
        T: Clone,
    {
        if slice.is_empty() {
            Ok(Self::new_empty())
        } else {
            let mut this = Self::new_zeroed(aspace, slice.len())?;
            this.extend_from_slice(slice);

            Ok(this)
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn slice(&self) -> &[T] {
        if self.len == 0 {
            &[]
        } else {
            // Safety: The rest of the code has to ensure that `self.len` is valid.
            unsafe { slice::from_raw_parts(self.as_ptr(), self.len) }
        }
    }

    pub fn slice_mut(&mut self) -> &mut [T] {
        if self.len == 0 {
            &mut []
        } else {
            // Safety: The rest of the code has to ensure that `self.len` is valid.
            unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), self.len) }
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.mmap.as_ptr().cast()
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.mmap.as_mut_ptr().cast()
    }

    pub fn extend_from_slice(&mut self, other: &[T])
    where
        T: Clone,
    {
        // "Transmute" the slice to a byte slice
        // Safety: we're just converting the slice to a byte slice of the same length
        let src = unsafe { slice::from_raw_parts(other.as_ptr().cast::<u8>(), size_of_val(other)) };
        self.mmap
            .copy_to_userspace(
                src,
                Range::from(self.len * size_of::<T>()..(self.len + other.len()) * size_of::<T>()),
            )
            .unwrap();
    }

    pub(crate) fn extend_with(&mut self, count: usize, elem: T)
    where
        T: Clone,
    {
        self.mmap
            .with_user_slice_mut(|dst| {
                // "Transmute" the slice to a byte slice
                // Safety: we're just converting the slice to a byte slice of the same length
                let dst =
                    unsafe { slice::from_raw_parts_mut(dst.as_mut_ptr().cast(), size_of_val(dst)) };

                dst[self.len..self.len + count].fill(elem);
            })
            .unwrap();
    }

    pub(crate) fn into_parts(self) -> (UserMmap, usize) {
        (self.mmap, self.len)
    }
}

impl<T> Deref for MmapVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.slice()
    }
}
