use crate::arch;
use crate::mem::{AddressRangeExt, AddressSpace, UserMmap};
use crate::wasm::runtime::{VMContext, VMOffsets};
use alloc::string::ToString;
use core::range::Range;

#[derive(Debug)]
pub struct OwnedVMContext(UserMmap);

impl OwnedVMContext {
    #[expect(clippy::unnecessary_wraps, reason = "TODO")]
    pub fn try_new(
        aspace: &mut AddressSpace,
        offsets: &VMOffsets,
    ) -> crate::Result<OwnedVMContext> {
        let mmap = UserMmap::new_zeroed(
            aspace,
            offsets.size() as usize,
            arch::PAGE_SIZE,
            Some("VMContext".to_string()),
        )
        .unwrap();
        mmap.commit(aspace, Range::from(0..offsets.size() as usize), true)
            .unwrap();

        Ok(Self(mmap))
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
