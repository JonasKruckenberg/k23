use core::mem::MaybeUninit;
use core::slice;

#[derive(Debug)]
pub struct StackVec<T, const N: usize> {
    inner: MaybeUninit<[T; N]>,
    len: usize,
}

impl<T, const N: usize> Default for StackVec<T, N> {
    fn default() -> Self {
        Self {
            inner: MaybeUninit::uninit(),
            len: 0,
        }
    }
}

impl<T, const N: usize> StackVec<T, N> {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, val: T) {
        let ptr = self.inner.as_mut_ptr() as *mut T;
        let ptr = unsafe { ptr.add(self.len) };
        unsafe { ptr.write(val) };
        self.len += 1;
    }

    pub fn pop(&mut self) {
        self.len -= 1;
    }

    pub fn last(&self) -> Option<&T> {
        self.as_slice().last()
    }

    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.as_mut_slice().last_mut()
    }

    pub fn as_slice(&self) -> &[T] {
        let ptr = self.inner.as_ptr() as *const T;
        unsafe { slice::from_raw_parts(ptr, self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let ptr = self.inner.as_ptr() as *mut T;
        unsafe { slice::from_raw_parts_mut(ptr, self.len) }
    }

    pub unsafe fn truncate(&mut self, len: usize) {
        if len > self.len {
            panic!()
        }
        self.len = len;
    }
}
