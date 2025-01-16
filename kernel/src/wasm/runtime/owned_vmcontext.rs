use crate::vm::AddressSpace;
use crate::wasm::runtime::{MmapVec, VMContext, VMOffsets};

#[derive(Debug)]
pub struct OwnedVMContext(MmapVec<u8>);

impl OwnedVMContext {
    pub fn try_new(
        aspace: &mut AddressSpace,
        offsets: &VMOffsets,
    ) -> crate::wasm::Result<OwnedVMContext> {
        let vec = MmapVec::new_zeroed(aspace, offsets.size() as usize)?;
        Ok(Self(vec))
    }
    pub fn as_ptr(&self) -> *const VMContext {
        self.0.as_ptr().cast()
    }
    pub fn as_mut_ptr(&mut self) -> *mut VMContext {
        self.0.as_mut_ptr().cast()
    }
    pub unsafe fn plus_offset<T>(&self, offset: u32) -> *const T {
        // Safety: caller has to ensure offset is valid
        unsafe {
            self.as_ptr()
                .byte_add(usize::try_from(offset).unwrap())
                .cast()
        }
    }
    pub unsafe fn plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        // Safety: caller has to ensure offset is valid
        unsafe {
            self.as_mut_ptr()
                .byte_add(usize::try_from(offset).unwrap())
                .cast()
        }
    }
}
