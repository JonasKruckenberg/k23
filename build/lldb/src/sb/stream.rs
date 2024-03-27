use cpp::{cpp, cpp_class};
use std::{ffi, slice};

cpp_class!(pub unsafe struct SBStream as "SBStream");

unsafe impl Send for SBStream {}

impl SBStream {
    pub fn new() -> SBStream {
        cpp!(unsafe [] -> SBStream as "SBStream" {
            return SBStream();
        })
    }
    pub fn data(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.as_ptr(), self.len()) }
    }
    pub fn len(&self) -> usize {
        cpp!(unsafe [self as "SBStream*"] -> usize as "size_t" {
            return self->GetSize();
        })
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn as_ptr(&self) -> *const u8 {
        cpp!(unsafe [self as "SBStream*"] -> *const ffi::c_char as "const char*" {
            return self->GetData();
        }) as *const u8
    }
    pub fn clear(&mut self) {
        cpp!(unsafe [self as "SBStream*"]  {
            self->Clear();
        })
    }
}
