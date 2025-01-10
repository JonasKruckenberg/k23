use crate::wasm::runtime::{MmapVec, VMContext, VMOffsets};
use crate::wasm::utils::round_usize_up_to_host_pages;

#[derive(Debug)]
pub struct OwnedVMContext(MmapVec<u8>);

impl OwnedVMContext {
    pub fn try_new(offsets: &VMOffsets) -> crate::wasm::Result<OwnedVMContext> {
        let vec = MmapVec::new_zeroed(round_usize_up_to_host_pages(offsets.size() as usize))?;
        Ok(Self(vec))
    }
    pub fn as_ptr(&self) -> *const VMContext {
        self.0.as_ptr().cast()
    }
    pub fn as_mut_ptr(&mut self) -> *mut VMContext {
        self.0.as_mut_ptr().cast()
    }
    pub unsafe fn plus_offset<T>(&self, offset: u32) -> *const T {
        unsafe {
            self.as_ptr()
                .byte_add(usize::try_from(offset).unwrap())
                .cast()
        }
    }
    pub unsafe fn plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        unsafe {
            self.as_mut_ptr()
                .byte_add(usize::try_from(offset).unwrap())
                .cast()
        }
    }
}
